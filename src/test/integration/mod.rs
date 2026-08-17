use crate::db;
use axum_test::{TestServer, TestServerConfig};
use dioxus::prelude::DioxusRouterExt;
use dioxus::server::FullstackState;
use once_cell::sync::Lazy;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use ulid::Ulid;

const MAX_SERVER_FN_BODY_BYTES: usize = 12 * 1024 * 1024;

pub(crate) mod auth;
mod build;
pub(crate) mod exports;
pub(crate) mod fixtures;
pub(crate) mod payments;
pub(crate) mod prices;
mod public_api;
pub(crate) mod settings;
pub(crate) mod transactions;
pub(crate) mod wallets;

static INTEGRATION_TEST_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

pub(crate) struct IntegrationTestServer {
    server: TestServer,
    _runtime: Option<db::TestRuntimeGuard>,
    _no_db_runtime: Option<NoDbTestRuntimeGuard>,
    _serial_guard: Option<MutexGuard<'static, ()>>,
}

impl Deref for IntegrationTestServer {
    type Target = TestServer;

    fn deref(&self) -> &Self::Target {
        &self.server
    }
}

impl IntegrationTestServer {
    pub(crate) fn user_database_path(&self, user_id: crate::models::UserId) -> std::path::PathBuf {
        let runtime = self
            ._runtime
            .as_ref()
            .expect("integration test server should own a runtime");
        let runtime_context = runtime.runtime_context();
        crate::project_paths::user_database_path_from_project_dir(
            runtime_context.project_dir(),
            user_id,
        )
    }
}

struct NoDbTestRuntimeGuard {
    project_dir: PathBuf,
    runtime_context: Arc<crate::runtime_context::RuntimeContext>,
    runtime_context_guard: Option<crate::runtime_context::DefaultRuntimeContextGuard>,
}

impl NoDbTestRuntimeGuard {
    fn acquire() -> Self {
        crate::desktop_session::clear();
        db::enable_test_mode();
        db::enable_user_test_mode();

        let project_dir = std::env::temp_dir().join(format!("bitgarth_test_nodb_{}", Ulid::new()));
        std::fs::create_dir_all(&project_dir)
            .expect("failed to create no-db integration project dir");
        let runtime_context = crate::runtime_context::RuntimeContext::new_test(project_dir.clone());
        let runtime_context_guard =
            crate::runtime_context::push_default_runtime_context(Arc::clone(&runtime_context));

        Self {
            project_dir,
            runtime_context,
            runtime_context_guard: Some(runtime_context_guard),
        }
    }

    fn runtime_context(&self) -> Arc<crate::runtime_context::RuntimeContext> {
        Arc::clone(&self.runtime_context)
    }
}

impl Drop for NoDbTestRuntimeGuard {
    fn drop(&mut self) {
        crate::desktop_session::clear();
        let _ = db::close_user_dbs_for_current_runtime();
        db::reset_test_db();
        let _ = self.runtime_context_guard.take();
        let _ = std::fs::remove_dir_all(&self.project_dir);
    }
}

/// Helper to create a fresh test server with in-memory database
pub(crate) fn setup_test_server() -> IntegrationTestServer {
    // Dioxus server-function tests create a pinned local pool per FullstackState.
    // Running many of these HTTP contract tests concurrently exhausts file descriptors,
    // so keep the integration harness explicitly serial.
    let serial_guard = INTEGRATION_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::sync_control::reset_sync_control_mode_override_for_tests();
    let runtime = db::acquire_test_runtime().expect("Failed to initialize test runtime");
    let runtime_context = runtime.runtime_context();

    build_test_server(
        Some(runtime),
        None,
        Some(runtime_context),
        Some(serial_guard),
        crate::backend::ProxyHeaderTrust::Untrusted,
    )
}

fn setup_test_server_with_proxy_trust(
    proxy_trust: crate::backend::ProxyHeaderTrust,
) -> IntegrationTestServer {
    let serial_guard = INTEGRATION_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::sync_control::reset_sync_control_mode_override_for_tests();
    let runtime = db::acquire_test_runtime().expect("Failed to initialize test runtime");
    let runtime_context = runtime.runtime_context();
    build_test_server(
        Some(runtime),
        None,
        Some(runtime_context),
        Some(serial_guard),
        proxy_trust,
    )
}

/// Helper to create a test server without DB runtime setup.
///
/// Use only for endpoint paths that do not touch DB code (for example:
/// malformed input, missing auth cookie, health checks).
pub(crate) fn setup_test_server_no_db() -> IntegrationTestServer {
    let serial_guard = INTEGRATION_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::sync_control::reset_sync_control_mode_override_for_tests();
    let runtime = NoDbTestRuntimeGuard::acquire();
    let runtime_context = runtime.runtime_context();

    build_test_server(
        None,
        Some(runtime),
        Some(runtime_context),
        Some(serial_guard),
        crate::backend::ProxyHeaderTrust::Untrusted,
    )
}

fn setup_app_test_server_no_db() -> IntegrationTestServer {
    let serial_guard = INTEGRATION_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::sync_control::reset_sync_control_mode_override_for_tests();
    let runtime = NoDbTestRuntimeGuard::acquire();
    let runtime_context = runtime.runtime_context();

    let pairing_store = Arc::new(crate::pairing::PairingStore::new());
    let proxy_trust = crate::backend::ProxyHeaderTrust::Untrusted;
    let router = axum::Router::new()
        .merge(crate::backend::public_api::router(
            Arc::clone(&pairing_store),
            proxy_trust,
        ))
        .serve_api_application(dioxus::server::ServeConfig::new(), crate::App)
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_SERVER_FN_BODY_BYTES,
        ))
        .layer(axum::middleware::from_fn(
            crate::backend::normalize_server_fn_bad_request,
        ))
        .layer(axum::middleware::from_fn(
            crate::backend::public_api::browser_pairing_no_store,
        ))
        .layer(axum::Extension(pairing_store))
        .layer(axum::Extension(proxy_trust))
        .layer(axum::Extension(Arc::clone(&runtime_context)))
        .layer(axum::middleware::from_fn(move |request, next| {
            crate::runtime_context::run_with_runtime_context(
                Arc::clone(&runtime_context),
                request,
                next,
            )
        }))
        .layer(axum::Extension(axum::extract::ConnectInfo(
            "127.0.0.1:45000"
                .parse::<std::net::SocketAddr>()
                .expect("test peer address should parse"),
        )));

    let config = TestServerConfig {
        save_cookies: true,
        ..Default::default()
    };
    let server = TestServer::new_with_config(router, config).expect("Failed to create test server");

    IntegrationTestServer {
        server,
        _runtime: None,
        _no_db_runtime: Some(runtime),
        _serial_guard: Some(serial_guard),
    }
}

fn setup_app_test_server() -> IntegrationTestServer {
    let serial_guard = INTEGRATION_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::sync_control::reset_sync_control_mode_override_for_tests();
    let runtime = db::acquire_test_runtime().expect("Failed to initialize test runtime");
    let runtime_context = runtime.runtime_context();
    let pairing_store = Arc::new(crate::pairing::PairingStore::new());
    let proxy_trust = crate::backend::ProxyHeaderTrust::Untrusted;
    let router = axum::Router::new()
        .merge(crate::backend::public_api::router(
            Arc::clone(&pairing_store),
            proxy_trust,
        ))
        .serve_api_application(dioxus::server::ServeConfig::new(), crate::App)
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_SERVER_FN_BODY_BYTES,
        ))
        .layer(axum::middleware::from_fn(
            crate::backend::normalize_server_fn_bad_request,
        ))
        .layer(axum::middleware::from_fn(
            crate::backend::public_api::browser_pairing_no_store,
        ))
        .layer(axum::Extension(pairing_store))
        .layer(axum::Extension(proxy_trust))
        .layer(axum::Extension(axum::extract::ConnectInfo(
            "127.0.0.1:45000"
                .parse::<std::net::SocketAddr>()
                .expect("test peer address should parse"),
        )))
        .layer(axum::Extension(Arc::clone(&runtime_context)))
        .layer(axum::middleware::from_fn(move |request, next| {
            crate::runtime_context::run_with_runtime_context(
                Arc::clone(&runtime_context),
                request,
                next,
            )
        }));

    let config = TestServerConfig {
        save_cookies: true,
        ..Default::default()
    };
    let server = TestServer::new_with_config(router, config).expect("Failed to create test server");
    IntegrationTestServer {
        server,
        _runtime: Some(runtime),
        _no_db_runtime: None,
        _serial_guard: Some(serial_guard),
    }
}

#[tokio::test]
async fn unknown_app_route_returns_not_found_status() {
    let server = setup_app_test_server_no_db();

    let response = server.get("/abc").await;

    assert_eq!(
        response.status_code(),
        dioxus::fullstack::StatusCode::NOT_FOUND
    );
    assert!(response.text().contains("Page not found"));
}

fn build_test_server(
    runtime: Option<db::TestRuntimeGuard>,
    no_db_runtime: Option<NoDbTestRuntimeGuard>,
    runtime_context: Option<Arc<crate::runtime_context::RuntimeContext>>,
    serial_guard: Option<MutexGuard<'static, ()>>,
    proxy_trust: crate::backend::ProxyHeaderTrust,
) -> IntegrationTestServer {
    let pairing_store = Arc::new(crate::pairing::PairingStore::new());
    let mut router = axum::Router::new()
        .merge(crate::backend::public_api::router(
            Arc::clone(&pairing_store),
            proxy_trust,
        ))
        .route(
            "/_app/user/transactions/sync/events",
            axum::routing::get(crate::backend::transactions_sync_events_sse),
        )
        .route(
            "/_app/user/exports/hledger/download",
            axum::routing::post(crate::backend::download_hledger),
        )
        .route(
            "/api/v1/build",
            axum::routing::get(crate::backend::current_build),
        )
        .register_server_functions()
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_SERVER_FN_BODY_BYTES,
        ))
        .layer(axum::middleware::from_fn(
            crate::backend::normalize_server_fn_bad_request,
        ))
        .layer(axum::middleware::from_fn(
            crate::backend::public_api::browser_pairing_no_store,
        ))
        .layer(axum::Extension(pairing_store))
        .layer(axum::Extension(proxy_trust))
        .layer(axum::Extension(axum::extract::ConnectInfo(
            "127.0.0.1:45000"
                .parse::<std::net::SocketAddr>()
                .expect("test peer address should parse"),
        )))
        .with_state(FullstackState::headless());

    if let Some(runtime_context) = runtime_context {
        router = router
            .layer(axum::Extension(Arc::clone(&runtime_context)))
            .layer(axum::middleware::from_fn(move |request, next| {
                crate::runtime_context::run_with_runtime_context(
                    Arc::clone(&runtime_context),
                    request,
                    next,
                )
            }));
    }

    let config = TestServerConfig {
        save_cookies: true,
        ..Default::default()
    };
    let server = TestServer::new_with_config(router, config).expect("Failed to create test server");

    IntegrationTestServer {
        server,
        _runtime: runtime,
        _no_db_runtime: no_db_runtime,
        _serial_guard: serial_guard,
    }
}

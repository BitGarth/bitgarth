use super::PerfError;
use dioxus::prelude::DioxusRouterExt;
use std::sync::Arc;
use std::thread;
use tokio::net::TcpListener;

pub(super) struct InProcessServer {
    pub(super) base_url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for InProcessServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) fn spawn_with_current_runtime_context<F, T>(f: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(feature = "server")]
    let runtime_context = crate::runtime_context::current_runtime_context();

    thread::spawn(move || {
        #[cfg(feature = "server")]
        let _runtime_context_guard =
            runtime_context.map(crate::runtime_context::push_default_runtime_context);

        f()
    })
}

impl InProcessServer {
    pub(super) async fn start(
        runtime_context: Arc<crate::runtime_context::RuntimeContext>,
    ) -> Result<Self, PerfError> {
        let router = axum::Router::new()
            .register_server_functions()
            .layer(axum::Extension(Arc::clone(&runtime_context)))
            .layer(axum::middleware::from_fn(move |request, next| {
                crate::runtime_context::run_with_runtime_context(
                    Arc::clone(&runtime_context),
                    request,
                    next,
                )
            }))
            .with_state(dioxus::server::FullstackState::headless());

        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|err| {
            PerfError::io(format!("failed to bind in-process perf server: {err}"))
        })?;
        let local_addr = listener
            .local_addr()
            .map_err(|err| PerfError::io(format!("failed to read perf server address: {err}")))?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        Ok(Self {
            base_url: format!("http://{local_addr}/"),
            task,
        })
    }
}

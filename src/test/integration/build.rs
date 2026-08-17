//! Integration test for the version-drift build endpoint.
//!
//! Scope: status, content-type, body shape, no-store cache headers.

use super::setup_test_server_no_db;

#[tokio::test(flavor = "current_thread")]
async fn build_endpoint_returns_plaintext_version_with_no_store() {
    let server = setup_test_server_no_db();
    let response = server.get("/api/v1/build").await;

    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .expect("content-type header present")
        .to_str()
        .expect("content-type is valid ascii");
    assert!(
        content_type.eq_ignore_ascii_case("text/plain; charset=utf-8"),
        "expected text/plain; charset=utf-8, got {content_type}"
    );

    let cache_control = response
        .headers()
        .get("cache-control")
        .expect("cache-control header present")
        .to_str()
        .expect("cache-control is valid ascii");
    let cache_control_lower = cache_control.to_ascii_lowercase();
    assert!(
        cache_control_lower.contains("no-store"),
        "expected no-store, got {cache_control}"
    );
    assert!(
        cache_control_lower.contains("max-age=0"),
        "expected max-age=0, got {cache_control}"
    );

    let pragma = response
        .headers()
        .get("pragma")
        .expect("pragma header present")
        .to_str()
        .expect("pragma is valid ascii");
    assert!(
        pragma.eq_ignore_ascii_case("no-cache"),
        "expected no-cache, got {pragma}"
    );

    let expires = response
        .headers()
        .get("expires")
        .expect("expires header present")
        .to_str()
        .expect("expires is valid ascii");
    assert_eq!(expires, "0", "expected 0, got {expires}");

    let body = response.text();
    assert!(
        body.starts_with(env!("CARGO_PKG_VERSION")),
        "expected body to start with package version, got {body}"
    );
}

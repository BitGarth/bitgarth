//! Integration tests for auth endpoint contracts.
//!
//! Scope:
//! - HTTP status codes
//! - JSON response envelope and shape
//! - Session cookie behavior
//!
//! Detailed auth validation branches are covered by unit tests.

use crate::auth::session::SESSION_COOKIE_NAME;
use crate::backend::ApiErrorEnvelope;
use crate::models::{AuthEntryDecision, AuthEntryMode};
use dioxus::fullstack::StatusCode;
use serde_json::{Value, json};
use ulid::Ulid;

use super::fixtures::legal_acknowledgement_json;
use super::{IntegrationTestServer, setup_test_server, setup_test_server_no_db};

fn unique_username(prefix: &str) -> String {
    format!("{prefix}_{}", Ulid::new())
}

async fn register_user(server: &IntegrationTestServer, username: &str) {
    server
        .post("/_app/auth/register")
        .json(&json!({
            "username": username,
            "password": "SecurePass123",
            "legal_acknowledgement": legal_acknowledgement_json()
        }))
        .await
        .assert_status_ok();
}

async fn assert_malformed_json_returns_bad_request(server: &IntegrationTestServer, path: &str) {
    let response = server
        .post(path)
        .bytes(vec![b'{'].into())
        .content_type("application/json")
        .await;
    let body = response.text();

    assert_eq!(
        response.status_code(),
        StatusCode::BAD_REQUEST,
        "Expected malformed JSON to return 400 for endpoint {path}. Body: {body}",
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_health_endpoint_returns_ok() {
    let server = setup_test_server_no_db();
    let response = server.get("/health").await;
    response.assert_status_ok();
}

#[tokio::test(flavor = "current_thread")]
async fn test_auth_entry_returns_register_when_no_users() {
    let server = setup_test_server();
    let response = server.get("/_app/auth/entry").await;
    response.assert_status_ok();

    let decision: AuthEntryDecision = response.json();
    assert_eq!(decision.mode, AuthEntryMode::Register);
    assert!(decision.banner.is_none());

    let response = server
        .get("/_app/auth/entry")
        .add_header(
            "cookie",
            format!("{SESSION_COOKIE_NAME}=leftover-from-old-install"),
        )
        .await;
    response.assert_status_ok();
    let decision: AuthEntryDecision = response.json();
    assert_eq!(decision.mode, AuthEntryMode::Register);
    assert!(decision.banner.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn test_auth_entry_returns_login_when_users_exist_without_cookie() {
    let server = setup_test_server();
    register_user(&server, &unique_username("entry_existing")).await;

    let response = server.get("/_app/auth/entry").clear_cookies().await;
    response.assert_status_ok();
    let decision: AuthEntryDecision = response.json();
    assert_eq!(decision.mode, AuthEntryMode::Login);
    assert!(decision.banner.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn test_hosted_auth_entry_uses_valid_session_only() {
    if crate::channel::channel() != crate::channel::Channel::Hosted {
        return;
    }

    let server = setup_test_server();
    let response = server
        .get("/_app/auth/entry")
        .add_header("cookie", format!("{SESSION_COOKIE_NAME}=invalid"))
        .await;
    response.assert_status_ok();
    let decision: AuthEntryDecision = response.json();
    assert_eq!(decision.mode, AuthEntryMode::Register);

    register_user(&server, &unique_username("hosted_entry")).await;
    let response = server.get("/_app/auth/entry").await;
    response.assert_status_ok();
    let decision: AuthEntryDecision = response.json();
    assert_eq!(decision.mode, AuthEntryMode::Login);

    let response = server.get("/_app/auth/entry").clear_cookies().await;
    response.assert_status_ok();
    let decision: AuthEntryDecision = response.json();
    assert_eq!(decision.mode, AuthEntryMode::Register);
}

#[tokio::test(flavor = "current_thread")]
async fn test_register_happy_path_sets_session_cookie() {
    let server = setup_test_server();
    let username = unique_username("register_happy");

    let response = server
        .post("/_app/auth/register")
        .json(&json!({
            "username": &username,
            "password": "SecurePass123",
            "legal_acknowledgement": legal_acknowledgement_json()
        }))
        .await;

    response.assert_status_ok();
    assert!(response.maybe_cookie(SESSION_COOKIE_NAME).is_some());

    let body: Value = response.json();
    assert_eq!(body["user"]["username"], username);
    assert!(body["user"]["user_id"].is_string());
    assert!(body["user"]["created_at"].is_string());
    assert!(body["user"]["updated_at"].is_string());
}

#[tokio::test(flavor = "current_thread")]
async fn test_login_happy_path_sets_session_cookie() {
    let server = setup_test_server();
    let username = unique_username("login_happy");
    register_user(&server, &username).await;

    let response = server
        .post("/_app/auth/login")
        .json(&json!({
            "username": &username,
            "password": "SecurePass123"
        }))
        .await;

    response.assert_status_ok();
    assert!(response.maybe_cookie(SESSION_COOKIE_NAME).is_some());
    let body: Value = response.json();
    assert_eq!(body["user"]["username"], username);
}

#[tokio::test(flavor = "current_thread")]
async fn test_logout_happy_path_returns_ok() {
    let server = setup_test_server();
    let username = unique_username("logout_happy");
    register_user(&server, &username).await;

    // Explicit login to ensure session lifecycle behavior is stable.
    server
        .post("/_app/auth/login")
        .json(&json!({
            "username": &username,
            "password": "SecurePass123"
        }))
        .await
        .assert_status_ok();

    let response = server.post("/_app/auth/logout").await;
    response.assert_status_ok();
}

#[tokio::test(flavor = "current_thread")]
async fn test_me_happy_path_returns_user() {
    let server = setup_test_server();
    let username = unique_username("me_happy");
    register_user(&server, &username).await;

    let response = server.get("/_app/auth/me").await;
    response.assert_status_ok();

    let body: Value = response.json();
    assert_eq!(body["user"]["username"], username);
}

#[tokio::test(flavor = "current_thread")]
async fn test_me_without_session_returns_unauthorized() {
    let server = setup_test_server_no_db();
    let response = server.get("/_app/auth/me").await;
    response.assert_status_unauthorized();
}

#[tokio::test(flavor = "current_thread")]
async fn test_register_validation_returns_field_keyed_errors() {
    let server = setup_test_server_no_db();
    let response = server
        .post("/_app/auth/register")
        .json(&json!({
            "username": "",
            "password": "short",
            "legal_acknowledgement": legal_acknowledgement_json()
        }))
        .await;

    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json();
    let auth_error: ApiErrorEnvelope = serde_json::from_value(body["data"].clone())
        .expect("Should parse AuthError from data field");

    assert!(auth_error.is_validation());
    assert!(auth_error.first_field_error("username").is_some());
    assert!(auth_error.first_field_error("password").is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn test_register_duplicate_username_returns_conflict() {
    let server = setup_test_server();
    let username = unique_username("register_duplicate");
    register_user(&server, &username).await;

    let response = server
        .post("/_app/auth/register")
        .json(&json!({
            "username": &username,
            "password": "AnotherPass123",
            "legal_acknowledgement": legal_acknowledgement_json()
        }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
}

#[tokio::test(flavor = "current_thread")]
async fn test_auth_body_endpoints_reject_malformed_json_with_bad_request() {
    let server = setup_test_server_no_db();

    for path in [
        "/_app/auth/register",
        "/_app/auth/login",
        "/_app/auth/change-password",
    ] {
        assert_malformed_json_returns_bad_request(&server, path).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_change_password_wrong_old_password_returns_field_error_and_keeps_session() {
    let server = setup_test_server();
    register_user(&server, &unique_username("change_pw_wrong_old")).await;

    let response = server
        .post("/_app/auth/change-password")
        .json(&json!({
            "old_password": "WrongPass123",
            "new_password": "NewSecurePass123"
        }))
        .await;

    // Not 401: the client treats 401 as an expired session and logs the user out.
    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json();
    let auth_error: ApiErrorEnvelope = serde_json::from_value(body["data"].clone())
        .expect("Should parse AuthError from data field");
    assert!(auth_error.is_validation());
    assert_eq!(
        auth_error
            .first_field_error("old_password")
            .map(String::as_str),
        Some("Incorrect password")
    );

    server.get("/_app/auth/me").await.assert_status_ok();
}

#![cfg(unix)]

use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

const CLIENT_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

const WALLETS: &str = r#"{
  "wallets": [
    {
      "id": "01JC2XAAAAAAAAAAAAAAAAAAAA",
      "label": "Cold storage",
      "accounts": [
        {
          "kind": "native",
          "account_id": "01JC2XBBBBBBBBBBBBBBBBBBBB",
          "native_account_id": "01JC2XBBBBBBBBBBBBBBBBBBBB",
          "account_state": "active",
          "asset": "bitcoin",
          "scheme": "native_segwit",
          "label": "Savings",
          "account_reference_kind": "single_address",
          "account_reference": "bc1qexample",
          "balance": {
            "unit_code": "BTC",
            "balance_reliability": {"kind": "final"},
            "balance_state": {"kind": "known", "amount": {"raw_value": "123450000", "formatted_value": "1.2345"}},
            "current_value": {"price": "60000", "converted_value": "74070.004", "currency": "EUR"}
          },
          "transaction_counts": {"pending": 0, "confirmed": 3, "dropped": 0, "failed": 0, "total": 3},
          "account_mode": "transactions",
          "future_field": true
        }
      ],
      "value_summary": {"priced_total": "74070.004", "currency": "EUR", "priced_asset_count": 1, "total_asset_count": 1}
    }
  ],
  "value_summary": {"priced_total": "74070.004", "currency": "EUR", "priced_asset_count": 1, "total_asset_count": 1, "priced_wallet_count": 1, "total_wallet_count": 1},
  "account_limit": {"active_count": 1, "inactive_count": 0, "active_limit": 5, "hard_cap": 50, "summary": "1 of 5 accounts"},
  "sync_capacity": {}
}"#;

#[test]
fn usage_and_login_problems_have_distinct_exit_codes() {
    let root = TestRoot::new();
    let missing_url = run_cli(&["auth", "login"], Some(&root));
    assert_eq!(missing_url.status.code(), Some(2));
    assert!(stderr(&missing_url).contains("bitgarth auth login <URL>"));

    let missing_http_consent = run_cli(
        &["auth", "login", "http://127.0.0.1:8080/path"],
        Some(&root),
    );
    assert_eq!(missing_http_consent.status.code(), Some(2));
    assert!(stderr(&missing_http_consent).contains("--allow-insecure-http"));

    let not_logged_in = run_cli(&["balance"], Some(&root));
    assert_eq!(not_logged_in.status.code(), Some(4));
    assert!(stderr(&not_logged_in).contains("not logged in"));
    assert!(stderr(&not_logged_in).contains("bitgarth auth login"));

    let origin = "http://127.0.0.1:9/";
    root.write_profiles(&[("zulu", origin, "user-z"), ("alpha", origin, "user-a")]);
    let ambiguous = run_cli(&["balance"], Some(&root));
    assert_eq!(ambiguous.status.code(), Some(4));
    assert!(stderr(&ambiguous).contains("available profiles: alpha, zulu"));

    let unknown = run_cli(&["--profile", "missing", "balance"], Some(&root));
    assert_eq!(unknown.status.code(), Some(4));
    assert!(stderr(&unknown).contains("unknown profile: missing"));
}

#[test]
fn balance_renders_values_and_sends_the_key_only_in_the_header() {
    let (origin, server) = serve(vec![(200, WALLETS.to_owned())]);
    let root = TestRoot::new();
    root.write_profiles(&[("personal", &origin, "remote-user")]);
    let output = run_cli(&["bs"], Some(&root));
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("Cold storage"));
    assert!(stdout.contains("Savings"));
    assert!(stdout.contains("1.2345 BTC"));
    assert!(stdout.contains("74,070.00 EUR"));
    assert!(stdout.contains("Total"));
    assert!(!stdout.contains(CLIENT_KEY));
    assert!(!stderr(&output).contains(CLIENT_KEY));
    let requests = server.join().unwrap_or_default();
    assert!(requests[0].starts_with("GET /api/v1/wallets "));
    assert!(requests[0].contains(&format!("authorization: Bearer {CLIENT_KEY}")));
}

#[test]
fn legacy_balance_logins_still_show_balances() {
    let body = include_str!("../../../tests/fixtures/client_api/wallet-balances.json");
    let (origin, server) = serve(vec![(200, body.to_owned())]);
    let root = TestRoot::new();
    root.write_store(&[("old", &origin, "remote-user")], "balances_read", None);
    let output = run_cli(&["balancesheet"], Some(&root));
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("Alpha"));
    assert!(
        stdout
            .contains("provisional (first successful sync pending, inactive account not syncing)")
    );
    assert!(stdout.contains("1.2345 BTC"));
    assert!(stderr(&output).contains("auth login"));
    let requests = server.join().unwrap_or_default();
    assert!(requests[0].starts_with("GET /api/v1/wallet-balances "));
}

#[test]
fn explicit_manual_asset_id_never_falls_back_to_another_network() {
    let search = r#"{"results":[{"asset_instance_id":{"asset_id":"algorand","network_id":"algorand-mainnet"},"unit_code":"ALGO","asset_name":"Algorand","network_name":"Algorand"}]}"#;
    let (origin, server) = serve(vec![(200, WALLETS.to_owned()), (200, search.to_owned())]);
    let root = TestRoot::new();
    root.write_profiles(&[("mine", &origin, "user-a")]);
    let result = run_cli(
        &["account", "add", "manual", "algorand@ethereum-mainnet"],
        Some(&root),
    );
    assert_eq!(result.status.code(), Some(2));
    assert!(stderr(&result).contains("exact ID"));
    assert_eq!(server.join().unwrap_or_default().len(), 2);
}

#[test]
fn price_history_shows_the_exact_unit_price_and_timestamp() {
    let body = r#"{"prices_enabled":false,"assets":[{"key":"token","name":"Token"}],"selected":"token","currency":"USD","resolved_from":"2026-01-01","resolved_to":"2026-01-01","entries":[{"YourPrice":{"at":"2026-01-01T09:04:53Z","price":"0.00001234","note":"first\nsecond"}}],"page":1,"total_entries":1}"#;
    let (origin, server) = serve(vec![(200, body.to_owned())]);
    let root = TestRoot::new();
    root.write_profiles(&[("mine", &origin, "user-a")]);
    let result = run_cli(&["price", "history", "token"], Some(&root));
    assert!(result.status.success(), "{}", stderr(&result));
    let text = stdout(&result);
    assert!(text.contains("0.00001234 USD"));
    assert!(text.contains("2026-01-01T09:04:53Z"));
    assert!(text.contains(r"first\nsecond"));
    assert!(server.join().is_ok());
}

#[test]
fn transaction_pages_include_later_pending_rows() {
    let body = r#"{"kind":"native","wallet_label":"Cold storage","unit_code":"BTC","address_scheme":"standard","network":"mainnet","account_mode":"transactions","pending":{"end":51,"total":51,"rows":[{"tx_hash":"pendinghash","status":"pending","direction":"incoming","occurred_at":"2026-01-01T00:00:00Z","value":{"formatted_value":"1"}}]},"confirmed":{"end":0,"total":2,"rows":[]}}"#;
    let (origin, server) = serve(vec![(200, WALLETS.to_owned()), (200, body.to_owned())]);
    let root = TestRoot::new();
    root.write_profiles(&[("mine", &origin, "user-a")]);
    let result = run_cli(
        &["account", "transactions", "Savings", "--page", "2"],
        Some(&root),
    );
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(stdout(&result).contains("pendinghash"));
    assert!(!stderr(&result).contains("--page 3"));
    let requests = server.join().unwrap_or_default();
    assert!(requests[1].contains("pending_page=2"));
}

#[test]
fn xpub_with_multiple_active_schemes_never_defaults_to_an_unused_scheme() {
    let inspect = r#"{"suggested_scheme":"legacy","schemes":[{"address_scheme":"legacy","first_address":"one","has_activity":false,"already_linked":false},{"address_scheme":"nested_segwit","first_address":"two","has_activity":true,"already_linked":false},{"address_scheme":"native_segwit","first_address":"three","has_activity":true,"already_linked":false}]}"#;
    let (origin, server) = serve(vec![
        (200, WALLETS.to_owned()),
        (200, inspect.to_owned()),
        (200, "{}".to_owned()),
    ]);
    let root = TestRoot::new();
    root.write_profiles(&[("mine", &origin, "user-a")]);
    let key = format!("xpub{}", "a".repeat(107));
    let result = run_cli(&["account", "add", "bitcoin", &key, "--json"], Some(&root));
    assert!(result.status.success(), "{}", stderr(&result));
    let requests = server.join().unwrap_or_default();
    assert!(requests[2].contains(r#""address_scheme":"nested_segwit""#));
}

#[test]
fn json_output_is_the_server_response_unchanged() {
    let (origin, server) = serve(vec![(200, WALLETS.to_owned())]);
    let root = TestRoot::new();
    root.write_profiles(&[("personal", &origin, "remote-user")]);
    let output = run_cli(&["wallet", "list", "--json"], Some(&root));
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    let expected: serde_json::Value = serde_json::from_str(WALLETS).unwrap_or_default();
    assert_eq!(parsed, expected);
    assert!(server.join().is_ok());
}

#[test]
fn rejected_keys_and_missing_permissions_exit_four_with_guidance() {
    let (origin, server) = serve(vec![(
        401,
        r#"{"code":"unauthorized","message":"Invalid Client Key"}"#.to_owned(),
    )]);
    let root = TestRoot::new();
    root.write_profiles(&[("personal", &origin, "remote-user")]);
    let revoked = run_cli(&["wallet", "list"], Some(&root));
    assert_eq!(revoked.status.code(), Some(4));
    assert!(stderr(&revoked).contains("no longer accepts profile personal"));
    assert!(server.join().is_ok());

    let (origin, server) = serve(vec![
        (200, WALLETS.to_owned()),
        (
            403,
            r#"{"code":"forbidden","message":"This paired client does not have full permission."}"#
                .to_owned(),
        ),
    ]);
    let root = TestRoot::new();
    root.write_store(&[("viewer", &origin, "remote-user")], "read", None);
    let denied = run_cli(&["wallet", "rename", "Cold storage", "Vault"], Some(&root));
    assert_eq!(denied.status.code(), Some(4));
    let message = stderr(&denied);
    assert!(message.contains("does not have full permission"));
    assert!(message.contains("Profile viewer is read-only"));
    assert!(message.contains("--allow-insecure-http"));
    let requests = server.join().unwrap_or_default();
    assert!(requests[1].starts_with("PATCH /api/v1/wallets/01JC2XAAAAAAAAAAAAAAAAAAAA "));
    assert!(requests[1].ends_with(r#"{"label":"Vault"}"#));
}

#[test]
fn deletion_without_a_terminal_requires_yes() {
    let (origin, server) = serve(vec![(200, WALLETS.to_owned())]);
    let root = TestRoot::new();
    root.write_profiles(&[("personal", &origin, "remote-user")]);
    let refused = run_cli(&["wallet", "delete", "cold storage"], Some(&root));
    assert_eq!(refused.status.code(), Some(2));
    assert!(stderr(&refused).contains("pass --yes"));
    let requests = server.join().unwrap_or_default();
    assert_eq!(requests.len(), 1);
}

#[test]
fn wallet_archives_use_private_new_files_and_keep_passwords_out_of_output()
-> Result<(), Box<dyn std::error::Error>> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let pass = "test archive password";
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .with_aes_encryption(zip::AesMode::Aes256, pass);
    zip.start_file("wallet-data.json", options)?;
    zip.write_all(b"{}")?;
    let bytes = zip.finish()?.into_inner();
    let encoded = STANDARD.encode(&bytes);
    let (origin, server) = serve(vec![
        (
            200,
            serde_json::json!({"zip_base64":encoded,"summary":{"encrypted":true}}).to_string(),
        ),
        (200, r#"{"assertions_created":1}"#.to_owned()),
        (
            422,
            r#"{"error":{"code":"validation","message":"Export failed"}}"#.to_owned(),
        ),
    ]);
    let root = TestRoot::new();
    root.write_profiles(&[("personal", &origin, "remote-user")]);
    let path = root.home.join("wallet.zip");
    let file = path.to_str().ok_or("invalid path")?;
    let export = run_cli_with_input(&["export", "wallet-data", file], Some(&root), pass);
    assert!(export.status.success(), "{}", stderr(&export));
    assert_eq!(fs::read(&path)?, bytes);
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o077, 0);
    let again = run_cli(&["export", "wallet-data", file], Some(&root));
    assert!(stderr(&again).contains("already exists"));
    let import = run_cli_with_input(
        &["import", "wallet-data", file, "--json"],
        Some(&root),
        pass,
    );
    assert!(import.status.success(), "{}", stderr(&import));
    assert!(stdout(&import).contains("assertions_created"));
    for output in [&export, &import] {
        assert!(!stdout(output).contains(pass));
        assert!(!stderr(output).contains(pass));
    }
    let failed = root.home.join("failed.zip");
    let error = run_cli_with_input(
        &[
            "export",
            "wallet-data",
            failed.to_str().ok_or("invalid path")?,
        ],
        Some(&root),
        pass,
    );
    assert!(!error.status.success());
    assert!(!failed.exists());
    let requests = server.join().map_err(|_| "test server stopped")?;
    assert!(requests[0].starts_with("POST /api/v1/exports/wallet-data "));
    assert!(requests[1].starts_with("POST /api/v1/imports/wallet-data "));
    assert!(requests[1].contains(&encoded));
    assert!(requests[1].contains(pass));
    Ok(())
}

#[test]
fn secrets_are_read_from_stdin_and_never_echoed() {
    let settings = r#"{"etherscan_api_key":false,"coingecko_api_key":true}"#;
    let (origin, server) = serve(vec![(200, settings.to_owned())]);
    let root = TestRoot::new();
    root.write_profiles(&[("personal", &origin, "remote-user")]);
    let output = run_cli_with_input(
        &["secret", "set", "coingecko-api-key"],
        Some(&root),
        "  CG-secret-value\n",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!stdout(&output).contains("CG-secret-value"));
    assert!(!stderr(&output).contains("CG-secret-value"));
    let requests = server.join().unwrap_or_default();
    assert!(requests[0].starts_with("PUT /api/v1/secrets/coingecko-api-key "));
    assert!(requests[0].ends_with(r#"{"value":"CG-secret-value"}"#));
}

#[test]
fn sync_wait_follows_the_accepted_run_and_reports_failure_to_scripts() {
    let (origin, server) = serve(vec![
        (
            200,
            r#"{"outcome":"queued","sync_run_id":"the-accepted-run"}"#.to_owned(),
        ),
        (
            200,
            r#"{"state":"completed","result":"failure"}"#.to_owned(),
        ),
        (
            200,
            r#"{"overall":{"last_result":"success"},"accounts":[]}"#.to_owned(),
        ),
    ]);
    let root = TestRoot::new();
    root.write_profiles(&[("mine", &origin, "user")]);
    let output = run_cli(&["sync", "run", "--wait", "--json"], Some(&root));
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    assert_eq!(json["sync_run_id"], "the-accepted-run");
    assert_eq!(json["result"], "failure");
    let requests = server.join().unwrap_or_default();
    assert!(requests[0].starts_with("POST /api/v1/sync "));
    assert!(requests[1].starts_with("GET /api/v1/sync/the-accepted-run "));
}

#[test]
fn auth_status_json_uses_effective_permission() {
    let me = serde_json::json!({
        "username": "sample-user", "client": {"name": "personal", "permission": "read", "expires_at": null}, "server_version": "1"
    });
    let (origin, server) = serve(vec![(200, me.to_string())]);
    let root = TestRoot::new();
    root.write_profiles(&[("personal", &origin, "remote-user")]);
    let output = run_cli(&["--json", "auth", "status"], Some(&root));
    assert!(output.status.success(), "{}", stderr(&output));
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    assert_eq!(rows[0]["permission"], "read");
    assert_eq!(server.join().unwrap_or_default().len(), 1);
}

#[test]
fn duplicate_remote_login_revokes_new_key_and_preserves_saved_login() {
    let start = serde_json::json!({
        "pairing_id": CLIENT_KEY, "code": "1111-1111",
        "approval_url": "{origin}pair?code=1111-1111",
        "expires_at": (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    });
    let claim = serde_json::json!({"status":"active", "remote_user_id":"remote-user", "permissions":["full"]});
    let (origin, server) = serve(vec![
        (200, start.to_string()),
        (200, claim.to_string()),
        (200, r#"{"status":"revoked"}"#.into()),
    ]);
    let root = TestRoot::new();
    root.write_profiles(&[("personal", &origin, "remote-user")]);
    let before = fs::read(root.profiles_path()).unwrap_or_default();
    let output = run_cli(
        &[
            "auth",
            "login",
            &origin,
            "--profile",
            "second",
            "--allow-insecure-http",
        ],
        Some(&root),
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("new key was revoked"),
        "{}",
        stderr(&output)
    );
    assert_eq!(before, fs::read(root.profiles_path()).unwrap_or_default());
    let requests = server.join().unwrap_or_default();
    assert!(requests[2].starts_with("DELETE /api/v1/clients/self "));
    assert!(!requests[2].contains(CLIENT_KEY));
}

#[test]
fn switch_sets_the_default_and_rename_keeps_it() {
    let root = TestRoot::new();
    root.write_profiles(&[
        ("personal", "https://my.bitgarth.app/", "user-a"),
        ("work", "https://work.example/", "user-b"),
    ]);
    assert!(
        run_cli(&["auth", "switch", "work"], Some(&root))
            .status
            .success()
    );
    assert!(
        run_cli(&["auth", "rename", "work", "office"], Some(&root))
            .status
            .success()
    );
    let store: serde_json::Value =
        serde_json::from_slice(&fs::read(root.profiles_path()).unwrap_or_default())
            .unwrap_or_default();
    assert_eq!(store["default"], "office");
    assert!(
        !String::from_utf8_lossy(&fs::read(root.profiles_path()).unwrap_or_default())
            .contains("\"work\"")
    );

    let unknown = run_cli(&["auth", "switch", "missing"], Some(&root));
    assert_eq!(unknown.status.code(), Some(2));
    let duplicate = run_cli(&["auth", "rename", "office", "personal"], Some(&root));
    assert_eq!(duplicate.status.code(), Some(2));
    assert!(stderr(&duplicate).contains("already exists"));
}

fn run_cli(args: &[&str], root: Option<&TestRoot>) -> Output {
    run_cli_with_input(args, root, "")
}

fn run_cli_with_input(args: &[&str], root: Option<&TestRoot>, input: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bitgarth"));
    command
        .args(args)
        .env_remove("BITGARTH_PROFILE")
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(root) = root {
        command.env("HOME", &root.home);
        command.env("XDG_CONFIG_HOME", &root.config);
    }
    let Ok(mut child) = command.spawn() else {
        std::process::abort();
    };
    if let Some(mut stdin) = child.stdin.take() {
        assert!(stdin.write_all(input.as_bytes()).is_ok());
    }
    match child.wait_with_output() {
        Ok(output) => output,
        Err(_) => std::process::abort(),
    }
}

/// Serve canned HTTP responses in order and hand back the raw requests.
fn serve(responses: Vec<(u16, String)>) -> (String, thread::JoinHandle<Vec<String>>) {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        std::process::abort();
    };
    let Ok(address) = listener.local_addr() else {
        std::process::abort();
    };
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let body = body.replace("{origin}", &format!("http://{address}/"));
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buffer = vec![0_u8; 65536];
            let mut request = Vec::new();
            while let Ok(read) = stream.read(&mut buffer) {
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                if read == 0 {
                    break;
                }
            }
            requests.push(String::from_utf8_lossy(&request).into_owned());
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            assert!(stream.write_all(response.as_bytes()).is_ok());
        }
        requests
    });
    (format!("http://{address}/"), handle)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

struct TestRoot {
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

impl TestRoot {
    fn new() -> Self {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "bitgarth-cli-process-test-{}-{sequence}",
            std::process::id()
        ));
        let home = root.join("home");
        let config = root.join("config");
        assert!(create_secure_directory(&root).is_ok());
        assert!(create_secure_directory(&home).is_ok());
        assert!(create_secure_directory(&config).is_ok());
        Self { root, home, config }
    }

    fn write_profiles(&self, profiles: &[(&str, &str, &str)]) {
        self.write_store(profiles, "full", None);
    }

    fn write_store(
        &self,
        profiles: &[(&str, &str, &str)],
        permission: &str,
        default: Option<&str>,
    ) {
        let path = self.profiles_path();
        let Some(directory) = path.parent() else {
            std::process::abort();
        };
        assert!(create_secure_tree(directory).is_ok());
        let entries = profiles
            .iter()
            .map(|(name, origin, remote_user_id)| {
                serde_json::json!({
                    "name": name,
                    "canonical_origin": origin,
                    "remote_user_id": remote_user_id,
                    "client_key": CLIENT_KEY,
                    "granted_permissions": [permission],
                    "allow_insecure_http": origin.starts_with("http://")
                })
            })
            .collect::<Vec<_>>();
        let mut store = serde_json::json!({
            "version": 1,
            "profiles": entries
        });
        if let Some(default) = default {
            store["default"] = serde_json::json!(default);
        }
        let bytes = serde_json::to_vec(&store);
        assert!(bytes.is_ok());
        let Ok(bytes) = bytes else { return };
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path);
        assert!(file.is_ok());
        if let Ok(mut file) = file {
            assert!(file.write_all(&bytes).is_ok());
        }
    }

    fn profiles_path(&self) -> PathBuf {
        let directory = if cfg!(target_os = "macos") {
            self.home
                .join("Library")
                .join("Application Support")
                .join("BitGarth")
                .join("cli")
        } else {
            self.config.join("BitGarth").join("cli")
        };
        directory.join("profiles.json")
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        if self.root.starts_with(std::env::temp_dir())
            && self.root.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("bitgarth-cli-process-test-")
            })
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn create_secure_tree(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    let mut current = Some(path);
    while let Some(directory) = current {
        if directory.ends_with("BitGarth") || directory.ends_with("cli") {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        current = directory.parent();
        if current.is_some_and(|parent| parent == std::env::temp_dir()) {
            break;
        }
    }
    Ok(())
}

fn create_secure_directory(path: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)
}

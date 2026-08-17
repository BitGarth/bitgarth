#![cfg(unix)]

use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

const CLIENT_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

#[test]
fn parser_and_selection_failures_exit_two() {
    let missing_url = run_cli(&["pair"], None);
    assert_eq!(missing_url.status.code(), Some(2));
    assert!(stderr(&missing_url).contains("BITGARTH_URL"));

    let missing_profile = run_cli(&["pair", "https://example.com/path"], None);
    assert_eq!(missing_profile.status.code(), Some(2));
    assert!(stderr(&missing_profile).contains("--profile <PROFILE>"));

    let missing_http_consent = run_cli(
        &[
            "--profile",
            "personal",
            "pair",
            "http://127.0.0.1:8080/path",
        ],
        None,
    );
    assert_eq!(missing_http_consent.status.code(), Some(2));
    assert!(stderr(&missing_http_consent).contains("--allow-insecure-http"));

    let help = run_cli(&["pair", "--help"], None);
    assert!(help.status.success());
    assert!(stdout(&help).contains("[BITGARTH_URL]"));
    assert!(!stdout(&help).contains("ORIGIN"));

    let root = TestRoot::new();
    let no_profiles = run_cli(&["balancesheet"], Some(&root));
    assert_eq!(no_profiles.status.code(), Some(2));
    assert!(stderr(&no_profiles).contains("no profiles configured"));
    assert!(stderr(&no_profiles).contains("run `bitgarth pair` first"));
    assert!(!stderr(&no_profiles).contains("ORIGIN"));

    let origin = "http://127.0.0.1:9/";
    root.write_profiles(&[("zulu", origin, "user-z"), ("alpha", origin, "user-a")]);
    let ambiguous = run_cli(&["balancesheet"], Some(&root));
    assert_eq!(ambiguous.status.code(), Some(2));
    assert!(stderr(&ambiguous).contains("available profiles: alpha, zulu"));

    let unknown = run_cli(&["--profile", "missing", "balancesheet"], Some(&root));
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("unknown profile: missing"));
}

#[test]
fn successful_balancesheet_infers_one_profile_and_redacts_key() {
    let listener = TcpListener::bind("127.0.0.1:0");
    assert!(listener.is_ok());
    let Ok(listener) = listener else {
        return;
    };
    let address = listener.local_addr();
    assert!(address.is_ok());
    let Ok(address) = address else {
        return;
    };
    let body = include_str!("../../../tests/fixtures/client_api/wallet-balances.json");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let server = thread::spawn(move || {
        let accepted = listener.accept();
        assert!(accepted.is_ok());
        let Ok((mut stream, _)) = accepted else {
            return;
        };
        let mut request = [0_u8; 4096];
        assert!(stream.read(&mut request).is_ok());
        assert!(stream.write_all(response.as_bytes()).is_ok());
    });

    let root = TestRoot::new();
    root.write_profiles(&[("personal", &format!("http://{address}/"), "remote-user")]);
    let output = run_cli(&["balancesheet"], Some(&root));
    assert!(output.status.success());
    let stdout = stdout(&output);
    let stderr = stderr(&output);
    assert!(stdout.contains("Wallet: Alpha"));
    assert!(stdout.contains("No balances."));
    assert!(stderr.contains("Warning: sending a Client Key over insecure HTTP"));
    assert!(!stdout.contains(CLIENT_KEY));
    assert!(!stderr.contains(CLIENT_KEY));
    assert!(!stderr.contains("Authorization"));
    assert!(server.join().is_ok());
}

#[test]
fn operational_transport_failure_exits_one_without_credentials() {
    let root = TestRoot::new();
    root.write_profiles(&[("personal", "http://127.0.0.1:9/", "remote-user")]);
    let output = run_cli(&["balancesheet"], Some(&root));
    assert_eq!(output.status.code(), Some(1));
    assert!(!stderr(&output).contains(CLIENT_KEY));
}

#[test]
fn profile_list_is_sorted_redacted_and_empty_is_success() {
    let empty_root = TestRoot::new();
    let empty = run_cli(&["profile", "list"], Some(&empty_root));
    assert!(empty.status.success());
    assert_eq!(stdout(&empty), "No profiles configured.\n");

    let root = TestRoot::new();
    root.write_profiles(&[
        ("zulu", "http://127.0.0.1:8080/", "user-z"),
        ("alpha", "https://my.bitgarth.app/", "user-a"),
    ]);
    let listed = run_cli(&["profile", "list"], Some(&root));
    assert!(listed.status.success());
    let output = stdout(&listed);
    assert_eq!(
        output,
        "PROFILE\tBITGARTH URL\nalpha\thttps://my.bitgarth.app/\nzulu\thttp://127.0.0.1:8080/\n"
    );
    assert!(!output.contains(CLIENT_KEY));
    assert!(!output.contains("user-a"));
    assert!(!output.contains("balances_read"));
}

#[test]
fn profile_rename_is_local_and_collision_safe() {
    let root = TestRoot::new();
    root.write_profiles(&[
        ("personal", "https://my.bitgarth.app/", "user-a"),
        ("work", "https://work.example/", "user-b"),
    ]);

    let renamed = run_cli(&["profile", "rename", "personal", "primary"], Some(&root));
    assert!(renamed.status.success());
    let listed = run_cli(&["profile", "list"], Some(&root));
    let output = stdout(&listed);
    assert!(output.contains("primary\thttps://my.bitgarth.app/"));
    assert!(!output.contains("personal\t"));
    let before_failure = fs::read(root.profiles_path());
    assert!(before_failure.is_ok());

    let duplicate = run_cli(&["profile", "rename", "primary", "work"], Some(&root));
    assert_eq!(duplicate.status.code(), Some(2));
    assert!(stderr(&duplicate).contains("profile already exists: work"));

    let unknown = run_cli(&["profile", "rename", "missing", "other"], Some(&root));
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("unknown profile: missing"));
    assert_eq!(fs::read(root.profiles_path()).ok(), before_failure.ok());

    let listed_again = run_cli(&["profile", "list"], Some(&root));
    let output = stdout(&listed_again);
    assert!(output.contains("primary\thttps://my.bitgarth.app/"));
    assert!(output.contains("work\thttps://work.example/"));
    assert!(!output.contains(CLIENT_KEY));
}

fn run_cli(args: &[&str], root: Option<&TestRoot>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bitgarth"));
    command.args(args);
    if let Some(root) = root {
        command.env("HOME", &root.home);
        command.env("XDG_CONFIG_HOME", &root.config);
    }
    let output = command.output();
    assert!(output.is_ok());
    match output {
        Ok(output) => output,
        Err(_) => std::process::abort(),
    }
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
                    "granted_permissions": ["balances_read"],
                    "allow_insecure_http": origin.starts_with("http://")
                })
            })
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "profiles": entries
        }));
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

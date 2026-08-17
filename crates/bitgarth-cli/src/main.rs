mod args;
mod client;
mod output;
mod profiles;

use chrono::Utc;
use clap::{CommandFactory, error::ErrorKind};
use std::io::{BufRead, IsTerminal as _};
use url::Url;

const DEFAULT_BITGARTH_URL: &str = "https://my.bitgarth.app/";

fn main() {
    let args = args::Args::parse();
    if let Err(error) = args.validate() {
        error.exit();
    }
    let exit_code = match run(args) {
        Ok(()) => 0,
        Err(RunError::Cancelled) => 1,
        Err(RunError::Usage(message)) => args::Args::command()
            .error(ErrorKind::InvalidValue, message)
            .exit(),
        Err(RunError::Storage(error)) => {
            eprintln!("{error}");
            1
        }
        Err(RunError::Client(error)) => {
            eprintln!("{error}");
            1
        }
        Err(RunError::Output(error)) => {
            eprintln!("{error}");
            1
        }
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn run(args: args::Args) -> Result<(), RunError> {
    let mut profiles = args.profile.into_iter();
    let profile_name = profiles.next();
    match args.command {
        args::Command::BalanceSheet => balancesheet(profile_name.as_deref()),
        args::Command::Pair {
            bitgarth_url,
            allow_insecure_http,
        } => {
            let stdin = std::io::stdin();
            let interactive = stdin.is_terminal();
            let mut input = stdin.lock();
            let mut error = std::io::stderr();
            let resolved = resolve_pair_args(
                profile_name,
                bitgarth_url,
                allow_insecure_http,
                interactive,
                &mut input,
                &mut error,
            )?;
            pair(
                resolved.profile_name,
                resolved.origin.as_str(),
                resolved.allow_insecure_http,
            )
        }
        args::Command::Profile {
            command: args::ProfileCommand::Remove { name },
        } => {
            if !profiles::ProfileStore::remove_saved(&name).map_err(RunError::Storage)? {
                return Err(RunError::Usage(format!("unknown profile: {name}")));
            }
            Ok(())
        }
        args::Command::Profile {
            command: args::ProfileCommand::List,
        } => profile_list(),
        args::Command::Profile {
            command: args::ProfileCommand::Rename { old, new },
        } => profile_rename(&old, &new),
    }
}

fn profile_list() -> Result<(), RunError> {
    let store = profiles::ProfileStore::load().map_err(RunError::Storage)?;
    if store.profiles().is_empty() {
        println!("No profiles configured.");
        return Ok(());
    }
    let mut profiles: Vec<_> = store.profiles().iter().collect();
    profiles.sort_unstable_by_key(|profile| profile.name());
    println!("PROFILE\tBITGARTH URL");
    for profile in profiles {
        println!("{}\t{}", profile.name(), profile.canonical_origin());
    }
    Ok(())
}

fn profile_rename(old: &str, new: &str) -> Result<(), RunError> {
    match profiles::ProfileStore::rename_saved(old, new).map_err(RunError::Storage)? {
        profiles::RenameResult::Renamed => Ok(()),
        profiles::RenameResult::Unknown => Err(RunError::Usage(format!("unknown profile: {old}"))),
        profiles::RenameResult::Duplicate => {
            Err(RunError::Usage(format!("profile already exists: {new}")))
        }
    }
}

fn balancesheet(profile_name: Option<&str>) -> Result<(), RunError> {
    let store = profiles::ProfileStore::load().map_err(RunError::Storage)?;
    let profile = store
        .select(profile_name)
        .map_err(|error| RunError::Usage(error.to_string()))?;
    let origin = client::ServerOrigin::parse(
        profile.canonical_origin().as_str(),
        profile.allows_insecure_http(),
    )
    .map_err(RunError::Client)?;
    if origin.allows_insecure_http() {
        eprintln!(
            "Warning: sending a Client Key over insecure HTTP to {}",
            origin.url()
        );
    }
    let client = client::BitGarthClient::new(origin).map_err(RunError::Client)?;
    let balances = client
        .wallet_balances(profile.client_key())
        .map_err(RunError::Client)?;
    let rendered = balances.render().map_err(RunError::Output)?;
    print!("{rendered}");
    Ok(())
}

fn pair(profile_name: String, origin: &str, allow_insecure_http: bool) -> Result<(), RunError> {
    let origin =
        client::ServerOrigin::parse(origin, allow_insecure_http).map_err(RunError::Client)?;
    let store = profiles::ProfileStore::load().map_err(RunError::Storage)?;
    if store.profile(&profile_name).is_some() {
        return Err(RunError::Usage(format!(
            "profile already exists: {profile_name}"
        )));
    }

    let client = client::BitGarthClient::new(origin).map_err(RunError::Client)?;
    let key_bytes = client::generate_client_key().map_err(RunError::Client)?;
    let verifier = client::key_verifier(&key_bytes);
    let client_key = profiles::SecretClientKey::from_bytes(&key_bytes);
    let started = client
        .start_pairing(&profile_name, verifier.as_str())
        .map_err(RunError::Client)?;
    let (expires_at, approval_url) = started
        .validate(client.origin())
        .map_err(RunError::Client)?;
    eprintln!("Approve this pairing at: {approval_url}");
    eprintln!("Pairing code: {}", started.code);

    let remote_user_id = loop {
        if Utc::now() >= expires_at {
            return Err(RunError::Client(client::ClientError::PairingExpired));
        }
        match client
            .claim_pairing(&started.pairing_id, &client_key)
            .map_err(RunError::Client)?
        {
            client::PairingClaim::Active { remote_user_id } => break remote_user_id,
            client::PairingClaim::Pending { retry_after } => {
                let remaining = (expires_at - Utc::now()).to_std().unwrap_or_default();
                if remaining <= retry_after {
                    return Err(RunError::Client(client::ClientError::PairingExpired));
                }
                std::thread::sleep(retry_after);
            }
        }
    };

    let profile = profiles::Profile::new(
        profile_name.clone(),
        client.origin().clone(),
        remote_user_id,
        client_key,
        allow_insecure_http,
    )
    .map_err(RunError::Storage)?;
    profiles::ProfileStore::insert_saved(profile).map_err(RunError::Storage)?;
    println!("Pairing successful. Profile '{profile_name}' is ready.");
    Ok(())
}

enum RunError {
    Usage(String),
    Cancelled,
    Storage(profiles::ProfileError),
    Client(client::ClientError),
    Output(output::OutputError),
}

struct ResolvedPairArgs {
    profile_name: String,
    origin: Url,
    allow_insecure_http: bool,
}

fn prompt_line<R: BufRead, W: std::io::Write>(
    input: &mut R,
    error: &mut W,
    prompt: &str,
) -> Result<Option<String>, RunError> {
    write!(error, "{prompt}")
        .and_then(|()| error.flush())
        .map_err(|_| RunError::Cancelled)?;
    let mut value = String::new();
    match input.read_line(&mut value) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(_) => {
            let _ = writeln!(error, "Pairing cancelled.");
            return Err(RunError::Cancelled);
        }
    }
    Ok(Some(value.trim_end_matches(['\r', '\n']).to_owned()))
}

fn cancel_pairing(error: &mut impl std::io::Write) -> RunError {
    let _ = writeln!(error, "Pairing cancelled.");
    RunError::Cancelled
}

fn resolve_pair_args<R: BufRead, W: std::io::Write>(
    profile_name: Option<String>,
    bitgarth_url: Option<String>,
    allow_insecure_http: bool,
    interactive: bool,
    input: &mut R,
    error: &mut W,
) -> Result<ResolvedPairArgs, RunError> {
    let origin = if let Some(input) = bitgarth_url {
        profiles::canonicalize_origin(&input).map_err(|error| RunError::Usage(error.to_string()))?
    } else {
        if !interactive {
            return Err(RunError::Usage(
                "BITGARTH_URL is required when input is not interactive".to_owned(),
            ));
        }
        loop {
            let Some(value) =
                prompt_line(input, error, "BitGarth URL [https://my.bitgarth.app/]: ")?
            else {
                return Err(cancel_pairing(error));
            };
            let value = if value.is_empty() {
                DEFAULT_BITGARTH_URL
            } else {
                value.as_str()
            };
            match profiles::canonicalize_origin(value) {
                Ok(origin) => break origin,
                Err(validation) => {
                    writeln!(error, "{validation}").map_err(|_| RunError::Cancelled)?;
                }
            }
        }
    };

    let allow_insecure_http = match (origin.scheme(), allow_insecure_http) {
        ("https", false) => false,
        ("https", true) => {
            return Err(RunError::Usage(
                "--allow-insecure-http is only valid with an HTTP BitGarth URL".to_owned(),
            ));
        }
        ("http", true) => {
            writeln!(
                error,
                "Warning: sending a Client Key over insecure HTTP to {origin}"
            )
            .map_err(|_| RunError::Cancelled)?;
            true
        }
        ("http", false) => {
            if !interactive {
                return Err(RunError::Usage(
                    "HTTP requires --allow-insecure-http when input is not interactive".to_owned(),
                ));
            }
            writeln!(
                error,
                "Warning: {origin} will receive the Client Key without transport encryption. Continue only on a network you trust."
            )
            .map_err(|_| RunError::Cancelled)?;
            let Some(answer) = prompt_line(input, error, "Type yes to continue: ")? else {
                return Err(cancel_pairing(error));
            };
            if !answer.trim().eq_ignore_ascii_case("yes") {
                return Err(cancel_pairing(error));
            }
            true
        }
        _ => return Err(RunError::Usage("invalid BitGarth URL".to_owned())),
    };

    let profile_name = if let Some(profile_name) = profile_name {
        profile_name
    } else {
        if !interactive {
            return Err(RunError::Usage(
                "--profile <PROFILE> is required when input is not interactive".to_owned(),
            ));
        }
        loop {
            let Some(value) = prompt_line(input, error, "Profile name: ")? else {
                return Err(cancel_pairing(error));
            };
            match profiles::validate_profile_name(&value) {
                Ok(()) => break value,
                Err(validation) => {
                    writeln!(error, "{validation}").map_err(|_| RunError::Cancelled)?;
                }
            }
        }
    };

    Ok(ResolvedPairArgs {
        profile_name,
        origin,
        allow_insecure_http,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{RunError, resolve_pair_args};

    #[test]
    fn interactive_pair_resolution_defaults_retries_and_normalizes() {
        let mut input = Cursor::new(b"not a URL\n\n bad\npersonal\n");
        let mut error = Vec::new();
        let resolved = resolve_pair_args(None, None, false, true, &mut input, &mut error);
        assert!(resolved.is_ok());
        let Ok(resolved) = resolved else { return };
        assert_eq!(resolved.origin.as_str(), "https://my.bitgarth.app/");
        assert_eq!(resolved.profile_name, "personal");
        assert!(!resolved.allow_insecure_http);
        let prompts = String::from_utf8_lossy(&error);
        assert!(prompts.contains("BitGarth URL [https://my.bitgarth.app/]:"));
        assert!(prompts.contains("invalid server origin"));
        assert!(prompts.contains("Profile name:"));
        assert!(prompts.contains("profile name contains forbidden"));
    }

    #[test]
    fn interactive_http_pairing_prompts_for_url_consent_then_profile() {
        let mut input = Cursor::new(b"http://127.0.0.1:8080/reports\nyes\npersonal\n");
        let mut error = Vec::new();

        let resolved = resolve_pair_args(None, None, false, true, &mut input, &mut error);

        assert!(resolved.is_ok());
        let Ok(resolved) = resolved else { return };
        assert_eq!(resolved.origin.as_str(), "http://127.0.0.1:8080/");
        assert_eq!(resolved.profile_name, "personal");
        let prompts = String::from_utf8_lossy(&error);
        let url = prompts.find("BitGarth URL [https://my.bitgarth.app/]:");
        let warning = prompts.find("Warning: http://127.0.0.1:8080/ will receive");
        let consent = prompts.find("Type yes to continue:");
        let profile = prompts.find("Profile name:");
        assert!(url < warning && warning < consent && consent < profile);
    }

    #[test]
    fn insecure_http_requires_full_yes_and_cancels_before_pairing() {
        for refused in [b"\n".as_slice(), b"y\n".as_slice(), b"no\n".as_slice()] {
            let mut input = Cursor::new(refused);
            let mut error = Vec::new();
            let result = resolve_pair_args(
                Some("personal".to_owned()),
                Some("http://127.0.0.1:8080/reports?month=8".to_owned()),
                false,
                true,
                &mut input,
                &mut error,
            );
            assert!(matches!(result, Err(RunError::Cancelled)));
            let output = String::from_utf8_lossy(&error);
            assert!(output.contains("without transport encryption"));
            assert!(output.contains("Pairing cancelled."));
        }

        let mut input = Cursor::new(b" YES \n");
        let mut error = Vec::new();
        let accepted = resolve_pair_args(
            Some("personal".to_owned()),
            Some("http://127.0.0.1:8080/reports?month=8".to_owned()),
            false,
            true,
            &mut input,
            &mut error,
        );
        assert!(accepted.as_ref().is_ok_and(|resolved| {
            resolved.origin.as_str() == "http://127.0.0.1:8080/" && resolved.allow_insecure_http
        }));
    }

    #[test]
    fn noninteractive_pair_resolution_requires_explicit_arguments_and_consent() {
        let mut input = Cursor::new(Vec::<u8>::new());
        let mut error = Vec::new();
        assert!(matches!(
            resolve_pair_args(None, None, false, false, &mut input, &mut error),
            Err(RunError::Usage(_))
        ));

        let mut input = Cursor::new(Vec::<u8>::new());
        let mut error = Vec::new();
        assert!(matches!(
            resolve_pair_args(
                Some("personal".to_owned()),
                Some("http://127.0.0.1:8080/".to_owned()),
                false,
                false,
                &mut input,
                &mut error,
            ),
            Err(RunError::Usage(_))
        ));

        let mut input = Cursor::new(Vec::<u8>::new());
        let mut error = Vec::new();
        let explicit = resolve_pair_args(
            Some("personal".to_owned()),
            Some("http://127.0.0.1:8080/path".to_owned()),
            true,
            false,
            &mut input,
            &mut error,
        );
        assert!(explicit.as_ref().is_ok_and(|resolved| {
            resolved.origin.as_str() == "http://127.0.0.1:8080/" && resolved.allow_insecure_http
        }));
    }

    struct FailingInput;

    impl std::io::Read for FailingInput {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("test read failure"))
        }
    }

    impl std::io::BufRead for FailingInput {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Err(std::io::Error::other("test read failure"))
        }

        fn consume(&mut self, _amount: usize) {}
    }

    #[test]
    fn prompt_read_failures_cancel_with_the_safe_message() {
        let mut input = FailingInput;
        let mut error = Vec::new();
        let result = resolve_pair_args(None, None, false, true, &mut input, &mut error);
        assert!(matches!(result, Err(RunError::Cancelled)));
        assert!(String::from_utf8_lossy(&error).contains("Pairing cancelled."));
    }
}

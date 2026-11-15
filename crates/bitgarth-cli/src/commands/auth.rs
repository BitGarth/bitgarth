use chrono::Utc;
use serde_json::json;
use url::Url;

use crate::args::AuthCommand;
use crate::client::{self, BitGarthClient, PairingClaim, ServerOrigin};
use crate::context::{CliError, Context, Session, decode};
use crate::model::Me;
use crate::profiles::{self, Profile, ProfileStore, RenameResult};
use crate::ui::{self, Style};

const DEFAULT_URL: &str = "https://my.bitgarth.app/";

pub(super) fn run(context: &Context, command: AuthCommand) -> Result<(), CliError> {
    match command {
        AuthCommand::Login {
            url,
            read_only,
            allow_insecure_http,
        } => login(context, url, read_only, allow_insecure_http),
        AuthCommand::Logout { local_only } => logout(context, local_only),
        AuthCommand::Status => status(context),
        AuthCommand::Switch { name } => switch(context, &name),
        AuthCommand::Rename { old, new } => rename(context, &old, &new),
    }
}

fn permission_label(permission: &str) -> &'static str {
    match permission {
        "full" => "full access",
        "read" => "read-only access",
        _ => "balances-only access",
    }
}

// ============ login ============

struct LoginTarget {
    origin: Url,
    allow_insecure_http: bool,
    profile_name: String,
}

fn resolve_login_target(
    profile_name: Option<&str>,
    url: Option<String>,
    allow_insecure_http: bool,
    interactive: bool,
) -> Result<LoginTarget, CliError> {
    let origin = match url {
        Some(url) => profiles::canonicalize_origin(&url).map_err(|_| {
            CliError::Usage(format!(
                "\"{url}\" is not a valid BitGarth address; use a URL like https://my.bitgarth.app"
            ))
        })?,
        None if interactive => loop {
            let Some(answer) = ui::prompt(&format!("BitGarth address [{DEFAULT_URL}]: "))? else {
                return Err(CliError::Cancelled);
            };
            let answer = if answer.trim().is_empty() {
                DEFAULT_URL
            } else {
                answer.trim()
            };
            match profiles::canonicalize_origin(answer) {
                Ok(origin) => break origin,
                Err(_) => {
                    ui::warn("That is not a valid address; use a URL like https://my.bitgarth.app")
                }
            }
        },
        None => {
            return Err(CliError::Usage(
                "the BitGarth address is required when input is not a terminal: bitgarth auth login <URL>".to_owned(),
            ));
        }
    };

    let allow_insecure_http = match (origin.scheme(), allow_insecure_http) {
        ("https", false) => false,
        ("https", true) => {
            return Err(CliError::Usage(
                "--allow-insecure-http only applies to http:// addresses".to_owned(),
            ));
        }
        ("http", true) => true,
        ("http", false) if interactive => {
            ui::warn(&format!(
                "{origin} is not encrypted. Your Client Key and financial data will travel in plain text."
            ));
            if !ui::confirm("Continue only on a network you trust. Continue?")? {
                return Err(CliError::Cancelled);
            }
            true
        }
        _ => {
            return Err(CliError::Usage(format!(
                "{origin} is not encrypted; pass --allow-insecure-http to use it anyway"
            )));
        }
    };

    let suggested = default_profile_name(&origin);
    let profile_name = match profile_name {
        Some(name) => name.to_owned(),
        None if interactive => loop {
            let Some(answer) = ui::prompt(&format!("Name for this login [{suggested}]: "))? else {
                return Err(CliError::Cancelled);
            };
            let name = if answer.trim().is_empty() {
                suggested.clone()
            } else {
                answer
            };
            match profiles::validate_profile_name(&name) {
                Ok(()) => break name,
                Err(error) => ui::warn(&error.to_string()),
            }
        },
        None => suggested,
    };
    profiles::validate_profile_name(&profile_name)?;
    Ok(LoginTarget {
        origin,
        allow_insecure_http,
        profile_name,
    })
}

/// `my.bitgarth.app`, or `127.0.0.1:8080` when a port is given.
fn default_profile_name(origin: &Url) -> String {
    let host = origin.host_str().unwrap_or("bitgarth");
    match origin.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

fn login(
    context: &Context,
    url: Option<String>,
    read_only: bool,
    allow_insecure_http: bool,
) -> Result<(), CliError> {
    let target = resolve_login_target(
        context.profile_name(),
        url,
        allow_insecure_http,
        ui::stdin_is_interactive(),
    )?;
    ProfileStore::prepare_saved()?;
    let store = ProfileStore::load()?;
    if store.profile(&target.profile_name).is_some() {
        return Err(CliError::Usage(format!(
            "a login named \"{}\" already exists; choose another with --profile <NAME>, or run `bitgarth auth logout --profile {}` first",
            target.profile_name, target.profile_name
        )));
    }
    let permission = if read_only { "read" } else { "full" };

    let origin = ServerOrigin::parse(target.origin.as_str(), target.allow_insecure_http)?;
    let client = BitGarthClient::new(origin)?;
    let key_bytes = client::generate_client_key()?;
    let verifier = client::key_verifier(&key_bytes);
    let client_key = profiles::SecretClientKey::from_bytes(&key_bytes);
    let started = client.start_pairing(&target.profile_name, verifier.as_str(), permission)?;
    let (expires_at, approval_url) = started.validate(client.origin())?;

    eprintln!();
    eprintln!(
        "  {} {}",
        ui::epaint(Style::Dim, "One-time code:"),
        ui::epaint(Style::Bold, &started.code)
    );
    eprintln!(
        "  {} {}",
        ui::epaint(Style::Dim, "Approve at:   "),
        ui::epaint(Style::Cyan, approval_url.as_str())
    );
    eprintln!();
    ui::info(&format!(
        "Open the link while signed in to BitGarth, check that the code matches, and approve {} access.",
        if read_only { "read-only" } else { "full" }
    ));
    ui::hint(&format!(
        "Waiting for approval until {}…",
        expires_at.with_timezone(&chrono::Local).format("%H:%M")
    ));

    let remote_user_id = loop {
        if Utc::now() >= expires_at {
            return Err(CliError::Failed(
                "the pairing request expired before it was approved; run the command again"
                    .to_owned(),
            ));
        }
        match client.claim_pairing(&started.pairing_id, &client_key, permission) {
            Ok(PairingClaim::Active { remote_user_id }) => break remote_user_id,
            Ok(PairingClaim::Pending { retry_after }) => {
                let remaining = (expires_at - Utc::now()).to_std().unwrap_or_default();
                std::thread::sleep(retry_after.min(remaining));
            }
            Err(error) if error.status() == Some(403) => {
                return Err(CliError::Failed(
                    "the pairing request was denied in the browser".to_owned(),
                ));
            }
            Err(error) => return Err(error.into()),
        }
    };

    let profile = Profile::new(
        target.profile_name.clone(),
        client.origin().clone(),
        remote_user_id,
        profiles::SecretClientKey::from_bytes(&key_bytes),
        permission,
        target.allow_insecure_http,
    );
    if let Err(error) = profile.and_then(ProfileStore::insert_saved) {
        let revoked = client.request(
            &client_key,
            reqwest::Method::DELETE,
            &["clients", "self"],
            &[],
            None,
        );
        let cleanup = match revoked {
            Ok(_) => "The new key was revoked.".to_owned(),
            Err(error) if error.status() == Some(401) => {
                "The new key is no longer active.".to_owned()
            }
            Err(_) => format!(
                "Revoke the newly approved '{}' key under Settings → Paired Clients.",
                target.profile_name
            ),
        };
        return Err(CliError::Failed(format!("{error}. {cleanup}")));
    }

    let session = Session::connect(
        ProfileStore::load()?
            .into_selected(Some(&target.profile_name))
            .map_err(|error| CliError::Failed(error.to_string()))?,
    )?;
    let username = session
        .get(&["me"], &[])
        .ok()
        .and_then(|me| decode::<Me>(me).ok())
        .and_then(|me| me.username);

    if context.json() {
        return context.print_json(&json!({
            "profile": target.profile_name,
            "origin": target.origin.as_str(),
            "permission": permission,
            "username": username,
            "default": true,
        }));
    }
    let who = username.map_or_else(String::new, |name| format!(" as {name}"));
    ui::success(&format!(
        "Logged in to {}{who} with {}",
        target.origin,
        permission_label(permission)
    ));
    ui::success(&format!(
        "Saved as profile {} and made it the default",
        target.profile_name
    ));
    ui::hint(
        "Next: `bitgarth balance` to see your portfolio, or `bitgarth account add --help` to add accounts.",
    );
    Ok(())
}

// ============ logout ============

fn logout(context: &Context, local_only: bool) -> Result<(), CliError> {
    let session = context.session()?;
    let name = session.profile.name().to_owned();
    let origin = session.origin().to_owned();
    let mut revoked = false;
    if !local_only {
        match session.delete(&["clients", "self"]) {
            Ok(_) => revoked = true,
            Err(CliError::Unauthorized { .. }) => {
                ui::warn("The server had already revoked this login.");
            }
            Err(error) => {
                ui::warn(&format!("Could not revoke access on the server: {error}"));
                ui::hint(
                    "Nothing was changed. Retry, or forget it here only with --local-only and revoke it in the web app under Settings > Paired Clients.",
                );
                return Err(CliError::Failed("logout incomplete".to_owned()));
            }
        }
    }
    ProfileStore::remove_saved(&name)?;
    if context.json() {
        return context
            .print_json(&json!({ "profile": name, "origin": origin, "revoked": revoked }));
    }
    if revoked {
        ui::success(&format!("Revoked this computer's access to {origin}"));
    }
    ui::success(&format!("Removed profile {name}"));
    if local_only {
        ui::hint(
            "The server still accepts this Client Key until you revoke it in the web app under Settings > Paired Clients.",
        );
    }
    Ok(())
}

// ============ status ============

fn status(context: &Context) -> Result<(), CliError> {
    let store = ProfileStore::load()?;
    let selected = store
        .select(context.profile_name())
        .ok()
        .map(|profile| profile.name().to_owned());
    let default = store.default_name().map(str::to_owned);
    let mut names: Vec<String> = store
        .profiles()
        .iter()
        .map(|profile| profile.name().to_owned())
        .collect();
    names.sort();

    let mut reports = Vec::new();
    for name in &names {
        let Ok(profile) = ProfileStore::load()?.into_selected(Some(name)) else {
            continue;
        };
        let origin = profile.canonical_origin().to_string();
        let permission = profile.permission().to_owned();
        let check = Session::connect(profile)
            .and_then(|session| session.get(&["me"], &[]))
            .and_then(decode::<Me>);
        reports.push((name.clone(), origin, permission, check));
    }

    if context.json() {
        let rows: Vec<_> = reports
            .iter()
            .map(|(name, origin, _, check)| {
                json!({
                    "profile": name,
                    "origin": origin,
                    "permission": check.as_ref().ok().map(|me| &me.client.permission),
                    "active": selected.as_deref() == Some(name.as_str()),
                    "default": default.as_deref() == Some(name.as_str()),
                    "ok": check.is_ok(),
                    "username": check.as_ref().ok().map(|me| me.username.clone()),
                    "error": check.as_ref().err().map(ToString::to_string),
                })
            })
            .collect();
        return context.print_json(&json!(rows));
    }

    if reports.is_empty() {
        println!("Not logged in to any BitGarth server.");
        ui::hint("Run `bitgarth auth login <URL>` to pair this computer with your account.");
        return Ok(());
    }
    for (index, (name, origin, permission, check)) in reports.iter().enumerate() {
        if index != 0 {
            println!();
        }
        let marker = if selected.as_deref() == Some(name.as_str()) {
            ui::paint(Style::Green, " (active)")
        } else {
            String::new()
        };
        println!("{}{marker}", ui::paint(Style::Bold, name));
        match check {
            Ok(me) => {
                let who = me.username.as_deref().map_or_else(String::new, |name| {
                    format!(" as {}", ui::paint(Style::Bold, name))
                });
                println!(
                    "  {} Logged in to {origin}{who}",
                    ui::paint(Style::Green, "✓")
                );
                println!(
                    "  {} {}",
                    ui::paint(Style::Dim, "•"),
                    permission_label(&me.client.permission)
                );
                let expiry = me.client.expires_at.as_deref().map_or_else(
                    || "never expires".to_owned(),
                    |at| format!("expires {}", ui::terminal_text(&super::local_time(at))),
                );
                println!(
                    "  {} Shown in the web app as \"{}\", {expiry}",
                    ui::paint(Style::Dim, "•"),
                    ui::terminal_text(&me.client.name)
                );
                println!(
                    "  {} Server version {}",
                    ui::paint(Style::Dim, "•"),
                    ui::terminal_text(&me.server_version)
                );
            }
            Err(error) => {
                println!(
                    "  {} {origin}: {}",
                    ui::paint(Style::Red, "✗"),
                    ui::terminal_text(&error.to_string())
                );
                println!(
                    "  {} {}",
                    ui::paint(Style::Dim, "•"),
                    permission_label(permission)
                );
            }
        }
    }
    if selected.is_none() && reports.len() > 1 {
        println!();
        ui::hint("No login is active. Pick one with `bitgarth auth switch <NAME>`.");
    }
    Ok(())
}

fn switch(context: &Context, name: &str) -> Result<(), CliError> {
    if !ProfileStore::set_default_saved(name)? {
        return Err(CliError::Usage(format!(
            "unknown profile: {name}; see `bitgarth auth status`"
        )));
    }
    if context.json() {
        return context.print_json(&json!({ "default": name }));
    }
    ui::success(&format!("Now using {} by default", name));
    Ok(())
}

fn rename(context: &Context, old: &str, new: &str) -> Result<(), CliError> {
    match ProfileStore::rename_saved(old, new)? {
        RenameResult::Renamed => {
            if context.json() {
                return context.print_json(&json!({ "profile": new, "previous": old }));
            }
            ui::success(&format!("Renamed profile {old} to {}", new));
            ui::hint("The web app keeps showing the name this computer paired with.");
            Ok(())
        }
        RenameResult::Unknown => Err(CliError::Usage(format!("unknown profile: {old}"))),
        RenameResult::Duplicate => Err(CliError::Usage(format!(
            "a profile named {new} already exists"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{default_profile_name, resolve_login_target};

    #[test]
    fn noninteractive_login_needs_an_address_and_explicit_http_consent() {
        assert!(resolve_login_target(None, None, false, false).is_err());
        assert!(
            resolve_login_target(None, Some("http://127.0.0.1:8080".into()), false, false).is_err()
        );
        assert!(
            resolve_login_target(None, Some("https://example.com".into()), true, false).is_err()
        );
        let target =
            resolve_login_target(None, Some("http://127.0.0.1:8080/x".into()), true, false);
        assert!(target.is_ok_and(|target| target.allow_insecure_http
            && target.profile_name == "127.0.0.1:8080"
            && target.origin.as_str() == "http://127.0.0.1:8080/"));
    }

    #[test]
    fn profile_name_defaults_to_the_server_host() {
        let target = resolve_login_target(
            Some("home"),
            Some("https://my.bitgarth.app/pair".into()),
            false,
            false,
        );
        assert!(target.is_ok_and(|target| target.profile_name == "home"));
        let origin = url::Url::parse("https://my.bitgarth.app/");
        assert!(origin.is_ok_and(|origin| default_profile_name(&origin) == "my.bitgarth.app"));
    }
}

use std::time::{Duration, Instant};

use reqwest::Method;
use serde_json::json;

use super::{find_account, load_wallets, local_time};
use crate::args::SyncCommand;
use crate::context::{CliError, Context, Session, decode};
use crate::model::{Account, SyncStarted, SyncStatus, Wallets};
use crate::ui::{self, Align, Cell, Style, Table};

const WAIT_LIMIT: Duration = Duration::from_secs(15 * 60);
const POLL_EVERY: Duration = Duration::from_secs(3);

pub(super) fn run(context: &Context, command: SyncCommand) -> Result<(), CliError> {
    match command {
        SyncCommand::Status => {
            let session = context.session()?;
            let raw = session.get(&["sync"], &[])?;
            if context.json() {
                return context.print_json(&raw);
            }
            let (_, wallets) = load_wallets(&session)?;
            render_status(&decode(raw)?, &wallets);
            Ok(())
        }
        SyncCommand::Run { account, wait } => start(context, account.as_deref(), wait),
    }
}

fn account_label<'a>(wallets: &'a Wallets, native_account_id: &'a str) -> &'a str {
    wallets
        .wallets
        .iter()
        .flat_map(|wallet| wallet.accounts.iter())
        .find_map(|account| match account {
            Account::Native(native) if native.native_account_id == native_account_id => {
                Some(native.label.as_str())
            }
            _ => None,
        })
        .unwrap_or(native_account_id)
}

fn render_status(status: &SyncStatus, wallets: &Wallets) {
    let overall = &status.overall;
    if overall.is_running {
        println!("{} Sync is running", ui::paint(Style::Cyan, "⟳"));
    } else {
        let when = overall
            .last_completed_at
            .as_deref()
            .map_or_else(|| "never".to_owned(), local_time);
        let (mark, style) = match overall.last_result.as_deref() {
            Some("success") => ("✓", Style::Green),
            Some("partial") => ("!", Style::Yellow),
            Some(_) => ("✗", Style::Red),
            None => ("•", Style::Dim),
        };
        println!("{} Last sync finished {when}", ui::paint(style, mark));
    }
    if let Some(error) = &overall.last_error {
        println!("  {}", ui::paint(Style::Red, error));
    }
    if status.accounts.is_empty() {
        println!();
        println!("No accounts sync from a blockchain yet.");
        return;
    }
    println!();
    let mut table = Table::new(&[
        ("ACCOUNT", Align::Left),
        ("STATE", Align::Left),
        ("ADDRESSES", Align::Right),
        ("LAST SUCCESS", Align::Left),
        ("PROBLEM", Align::Left),
    ]);
    for account in &status.accounts {
        let (state, style) = if account.addresses_in_progress > 0 {
            ("syncing", Style::Cyan)
        } else {
            match account.last_result.as_deref() {
                Some("success") => ("ok", Style::Green),
                Some("partial") => ("partial", Style::Yellow),
                Some("failure") => ("failed", Style::Red),
                Some(other) => (other, Style::Dim),
                None => ("waiting", Style::Dim),
            }
        };
        let failed = if account.addresses_failed > 0 {
            format!(" ({} failed)", account.addresses_failed)
        } else {
            String::new()
        };
        table.row(vec![
            Cell::styled(account_label(wallets, &account.account_id), Style::Bold),
            Cell::styled(state, style),
            Cell::new(format!(
                "{}/{}{failed}",
                account.addresses_synced, account.addresses_total
            )),
            Cell::new(
                account
                    .last_success_at
                    .as_deref()
                    .map_or_else(|| "never".to_owned(), local_time),
            ),
            Cell::styled(account.last_error.clone().unwrap_or_default(), Style::Red),
        ]);
    }
    print!("{}", table.render());
    let needs_etherscan_key = status.accounts.iter().any(|account| {
        account
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("Etherscan API key"))
    });
    if needs_etherscan_key {
        ui::hint(
            "Store your key with `bitgarth secret set etherscan-api-key`, then `bitgarth sync run`.",
        );
    }
}

fn start(context: &Context, account: Option<&str>, wait: bool) -> Result<(), CliError> {
    let session = context.session()?;
    let scope = match account {
        Some(query) => {
            let (_, wallets) = load_wallets(&session)?;
            let (_, account) = find_account(&wallets, query)?;
            let Account::Native(native) = account else {
                return Err(CliError::Usage(format!(
                    "\"{}\" is a manual account; it has nothing to sync",
                    account.label()
                )));
            };
            json!({ "kind": "account", "account_id": native.native_account_id })
        }
        None => json!({ "kind": "user" }),
    };
    let raw = session.send(Method::POST, &["sync"], &json!({ "scope": scope }))?;
    let started: SyncStarted = decode(raw.clone())?;
    if !wait {
        if context.json() {
            return context.print_json(&raw);
        }
        ui::success(match started.outcome.as_str() {
            "queued" => "Sync queued behind the one already running",
            _ => "Sync started",
        });
        ui::hint("Follow along with `bitgarth sync status`, or rerun with --wait.");
        return Ok(());
    }
    if !context.json() {
        ui::info("Syncing…");
    }
    let result = wait_for_sync(&session, &started.sync_run_id)?;
    let mut snapshot = session.get(&["sync"], &[])?;
    if context.json() {
        let fields = snapshot
            .as_object_mut()
            .ok_or_else(|| CliError::Failed("Unexpected sync status response".to_owned()))?;
        fields.insert("sync_run_id".to_owned(), json!(started.sync_run_id));
        fields.insert("result".to_owned(), json!(result));
        context.print_json(&snapshot)?;
    } else {
        if matches!(result, SyncResult::Success) {
            ui::success("Sync finished");
        }
        let (_, wallets) = load_wallets(&session)?;
        render_status(&decode(snapshot)?, &wallets);
    }
    match result {
        SyncResult::Success => Ok(()),
        SyncResult::Partial => Err(CliError::Failed(
            "Sync finished with partial results".to_owned(),
        )),
        SyncResult::Failure => Err(CliError::Failed("Sync failed".to_owned())),
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum RunStatus {
    Queued,
    Running,
    Completed { result: SyncResult },
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum SyncResult {
    Success,
    Partial,
    Failure,
}

fn wait_for_sync(session: &Session, run_id: &str) -> Result<SyncResult, CliError> {
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        let raw = session.get(&["sync", run_id], &[])?;
        if let RunStatus::Completed { result } = decode(raw)? {
            return Ok(result);
        }
        if Instant::now() >= deadline {
            return Err(CliError::Failed(format!(
                "sync {run_id} has not finished after 15 minutes"
            )));
        }
        std::thread::sleep(POLL_EVERY);
    }
}

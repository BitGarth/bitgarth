use reqwest::Method;
use serde_json::json;

use super::{confirm_destructive, find_wallet, load_wallets};
use crate::args::WalletCommand;
use crate::context::{CliError, Context, Session, decode};
use crate::model::{AccountSummary, LegacyBalances, ValueSummary, Wallet, Wallets};
use crate::ui::{self, Align, Cell, Style, Table};

pub(super) fn run(context: &Context, command: WalletCommand) -> Result<(), CliError> {
    match command {
        WalletCommand::List => list(context),
        WalletCommand::View { wallet } => balance(context, Some(&wallet)),
        WalletCommand::Rename { wallet, name } => rename(context, &wallet, &name),
        WalletCommand::Delete { wallet, yes } => delete(context, &wallet, yes),
    }
}

fn value_text(summary: Option<&ValueSummary>) -> String {
    match summary {
        Some(summary) if summary.priced_asset_count > 0 => {
            ui::fiat(&summary.priced_total, &summary.currency)
        }
        _ => "—".to_owned(),
    }
}

fn list(context: &Context) -> Result<(), CliError> {
    let session = context.session()?;
    let (raw, wallets) = load_wallets(&session)?;
    if context.json() {
        return context.print_json(&raw);
    }
    if wallets.wallets.is_empty() {
        println!("No wallets yet.");
        ui::hint("Add one with `bitgarth account add bitcoin <ADDRESS> --new-wallet <NAME>`.");
        return Ok(());
    }
    let mut table = Table::new(&[
        ("WALLET", Align::Left),
        ("ACCOUNTS", Align::Right),
        ("VALUE", Align::Right),
        ("ID", Align::Left),
    ]);
    for wallet in &wallets.wallets {
        table.row(vec![
            Cell::styled(&wallet.label, Style::Bold),
            Cell::new(wallet.accounts.len().to_string()),
            Cell::new(value_text(wallet.value_summary.as_ref())),
            Cell::styled(&wallet.id, Style::Dim),
        ]);
    }
    if let Some(total) = &wallets.value_summary {
        table.footer(vec![
            Cell::styled("Total", Style::Bold),
            Cell::new(
                wallets
                    .wallets
                    .iter()
                    .map(|wallet| wallet.accounts.len())
                    .sum::<usize>()
                    .to_string(),
            ),
            Cell::styled(value_text(Some(total)), Style::Bold),
            Cell::new(""),
        ]);
    }
    print!("{}", table.render());
    footnotes(&wallets);
    Ok(())
}

/// Balance-sheet view: every wallet with its accounts, balances and values.
pub(super) fn balance(context: &Context, only: Option<&str>) -> Result<(), CliError> {
    let session = context.session()?;
    if session.profile.permission() == "balances_read" && only.is_none() {
        return legacy_balance(context, &session);
    }
    let (raw, wallets) = load_wallets(&session)?;
    let selected: Vec<&Wallet> = match only {
        Some(query) => vec![find_wallet(&wallets, query)?],
        None => wallets.wallets.iter().collect(),
    };
    if context.json() {
        return match only {
            Some(_) => {
                let id = selected
                    .first()
                    .map(|wallet| wallet.id.as_str())
                    .unwrap_or_default();
                let wallet = raw["wallets"]
                    .as_array()
                    .and_then(|all| all.iter().find(|wallet| wallet["id"] == id))
                    .cloned()
                    .unwrap_or_default();
                context.print_json(&wallet)
            }
            None => context.print_json(&raw),
        };
    }
    if selected.is_empty() {
        println!("No wallets yet.");
        ui::hint("Add one with `bitgarth account add bitcoin <ADDRESS> --new-wallet <NAME>`.");
        return Ok(());
    }

    let mut table = Table::new(&[
        ("", Align::Left),
        ("TYPE", Align::Left),
        ("BALANCE", Align::Right),
        ("VALUE", Align::Right),
    ]);
    let mut provisional = false;
    for wallet in &selected {
        table.row(vec![
            Cell::styled(&wallet.label, Style::Bold),
            Cell::styled(
                match wallet.accounts.len() {
                    1 => "1 account".to_owned(),
                    count => format!("{count} accounts"),
                },
                Style::Dim,
            ),
            Cell::new(""),
            Cell::styled(value_text(wallet.value_summary.as_ref()), Style::Bold),
        ]);
        for account in &wallet.accounts {
            let summary = account.summary();
            provisional |= summary.provisional;
            table.row(account_cells(&summary));
        }
        if wallet.accounts.is_empty() {
            table.row(vec![Cell::styled("  (no accounts)", Style::Dim)]);
        }
    }
    if let (None, Some(total)) = (only, &wallets.value_summary) {
        table.footer(vec![
            Cell::styled("Total", Style::Bold),
            Cell::styled(
                format!(
                    "{} of {} assets priced",
                    total.priced_asset_count, total.total_asset_count
                ),
                Style::Dim,
            ),
            Cell::new(""),
            Cell::styled(value_text(Some(total)), Style::Bold),
        ]);
    }
    print!("{}", table.render());
    if provisional {
        ui::hint(
            "~ provisional and ? unknown balances wait on a sync; see `bitgarth sync status`.",
        );
    }
    footnotes(&wallets);
    Ok(())
}

pub(super) fn account_cells(summary: &AccountSummary<'_>) -> Vec<Cell> {
    let label = if summary.inactive {
        format!("  {} (inactive)", summary.label)
    } else {
        format!("  {}", summary.label)
    };
    let balance = match summary.balance {
        Some(balance) if summary.provisional => format!("~{}", ui::amount(balance, summary.unit)),
        Some(balance) => ui::amount(balance, summary.unit),
        None => format!("? {}", summary.unit),
    };
    vec![
        Cell::new(label),
        Cell::styled(summary.kind.clone(), Style::Dim),
        Cell::styled(
            balance,
            if summary.provisional || summary.balance.is_none() {
                Style::Yellow
            } else {
                Style::Plain
            },
        ),
        Cell::new(summary.value.map_or_else(
            || "—".to_owned(),
            |value| ui::fiat(&value.converted_value, &value.currency),
        )),
    ]
}

/// Balances for logins paired before read/full permissions existed.
fn legacy_balance(context: &Context, session: &Session) -> Result<(), CliError> {
    let raw = session.get(&["wallet-balances"], &[])?;
    if context.json() {
        return context.print_json(&raw);
    }
    let balances: LegacyBalances = decode(raw)?;
    let mut table = Table::new(&[
        ("", Align::Left),
        ("BALANCE", Align::Right),
        ("RELIABILITY", Align::Left),
    ]);
    for wallet in &balances.wallets {
        table.row(vec![Cell::styled(wallet.name.as_str(), Style::Bold)]);
        for balance in &wallet.balances {
            let text = balance.amount.as_deref().map_or_else(
                || format!("? {}", balance.unit),
                |amount| ui::amount(amount, &balance.unit),
            );
            let style = if balance.status == "final" {
                Style::Plain
            } else {
                Style::Yellow
            };
            let reliability = if balance.reasons.is_empty() {
                balance.status.clone()
            } else {
                format!(
                    "{} ({})",
                    balance.status,
                    balance.reasons.join(", ").replace('_', " ")
                )
            };
            table.row(vec![
                Cell::new(""),
                Cell::styled(text, style),
                Cell::new(reliability),
            ]);
        }
    }
    print!("{}", table.render());
    ui::hint(
        "This login predates values and accounts. Run `bitgarth auth login` again for full access.",
    );
    Ok(())
}

fn footnotes(wallets: &Wallets) {
    let has_values = wallets.value_summary.is_some()
        || wallets
            .wallets
            .iter()
            .any(|wallet| wallet.value_summary.is_some());
    let has_accounts = wallets
        .wallets
        .iter()
        .any(|wallet| !wallet.accounts.is_empty());
    if has_accounts && !has_values {
        ui::hint("Values need prices: `bitgarth settings set price-fetching on`.");
    }
    if let Some(upgrade) = &wallets.account_limit.upgrade_call_to_action {
        ui::hint(&format!("{} {upgrade}", wallets.account_limit.summary));
    }
}

fn rename(context: &Context, query: &str, name: &str) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let wallet = find_wallet(&wallets, query)?;
    let response = session.send(
        Method::PATCH,
        &["wallets", &wallet.id],
        &json!({ "label": name }),
    )?;
    if context.json() {
        return context.print_json(&json!({ "id": wallet.id, "label": name, "result": response }));
    }
    ui::success(&format!("Renamed wallet {} to {}", wallet.label, name));
    Ok(())
}

fn delete(context: &Context, query: &str, yes: bool) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let wallet = find_wallet(&wallets, query)?;
    let accounts = wallet.accounts.len();
    confirm_destructive(
        &format!(
            "Delete wallet \"{}\" and its {accounts} account(s), including their history?",
            wallet.label
        ),
        yes,
    )?;
    session.delete(&["wallets", &wallet.id])?;
    if context.json() {
        return context.print_json(&json!({ "deleted": wallet.id, "accounts_deleted": accounts }));
    }
    ui::success(&format!(
        "Deleted wallet {} and {accounts} account(s)",
        wallet.label
    ));
    Ok(())
}

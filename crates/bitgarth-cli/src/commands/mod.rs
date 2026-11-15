mod account;
mod api;
mod auth;
mod prices;
mod reports;
mod settings;
mod sync;
mod wallet;
mod wallet_data;

use crate::args::Command;
use crate::context::{CliError, Context, Session, decode};
use crate::model::{Account, Wallet, Wallets};
use crate::ui;

pub(crate) fn run(context: &Context, command: Command) -> Result<(), CliError> {
    match command {
        Command::Auth(command) => auth::run(context, command),
        Command::Balance { wallet } => wallet::balance(context, wallet.as_deref()),
        Command::Wallet(command) => wallet::run(context, command),
        Command::Account(command) => account::run(context, command),
        Command::Report(command) => reports::run(context, command),
        Command::Sync(command) => sync::run(context, command),
        Command::Price(command) => prices::run(context, command),
        Command::Asset(command) => account::asset(context, command),
        Command::Settings(command) => settings::run(context, command),
        Command::Secret(command) => settings::secret(context, command),
        Command::Export(crate::args::WalletDataCommand::WalletData { file }) => {
            wallet_data::export(context, &file)
        }
        Command::Import(crate::args::WalletDataCommand::WalletData { file }) => {
            wallet_data::import(context, &file)
        }
        Command::Api(args) => api::run(context, args),
    }
}

pub(crate) fn load_wallets(session: &Session) -> Result<(serde_json::Value, Wallets), CliError> {
    let raw = session.get(&["wallets"], &[])?;
    let wallets = decode(raw.clone())?;
    Ok((raw, wallets))
}

/// Pick one item by exact ID, unique ID prefix, or exact name (any case).
fn resolve<'a, T>(
    items: Vec<(&'a str, &'a str, T)>,
    query: &str,
    noun: &str,
    list_command: &str,
) -> Result<T, CliError> {
    let query = query.trim();
    let lower = query.to_lowercase();
    let mut items = items;
    if let Some(index) = items
        .iter()
        .position(|(id, _, _)| id.eq_ignore_ascii_case(query))
    {
        return Ok(items.swap_remove(index).2);
    }
    let by_name: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, (_, label, _))| label.to_lowercase() == lower)
        .map(|(index, _)| index)
        .collect();
    let by_prefix: Vec<usize> = if query.len() >= 4 {
        items
            .iter()
            .enumerate()
            .filter(|(_, (id, _, _))| id.to_lowercase().starts_with(&lower))
            .map(|(index, _)| index)
            .collect()
    } else {
        Vec::new()
    };
    let matches = if by_name.is_empty() {
        by_prefix
    } else {
        by_name
    };
    match matches.as_slice() {
        [index] => Ok(items.swap_remove(*index).2),
        [] => Err(CliError::Usage(format!(
            "no {noun} matches \"{query}\"; see `{list_command}`"
        ))),
        many => {
            let candidates: Vec<String> = many
                .iter()
                .map(|index| format!("{} ({})", items[*index].1, items[*index].0))
                .collect();
            Err(CliError::Usage(format!(
                "\"{query}\" matches several {noun}s: {}; use the ID instead",
                candidates.join(", ")
            )))
        }
    }
}

pub(crate) fn find_wallet<'a>(wallets: &'a Wallets, query: &str) -> Result<&'a Wallet, CliError> {
    resolve(
        wallets
            .wallets
            .iter()
            .map(|wallet| (wallet.id.as_str(), wallet.label.as_str(), wallet))
            .collect(),
        query,
        "wallet",
        "bitgarth wallet list",
    )
}

pub(crate) fn find_account<'a>(
    wallets: &'a Wallets,
    query: &str,
) -> Result<(&'a Wallet, &'a Account), CliError> {
    resolve(
        wallets
            .wallets
            .iter()
            .flat_map(|wallet| {
                wallet
                    .accounts
                    .iter()
                    .map(move |account| (account.id(), account.label(), (wallet, account)))
            })
            .collect(),
        query,
        "account",
        "bitgarth account list",
    )
}

/// Ask before a destructive action. Without a terminal, require `--yes`.
pub(crate) fn confirm_destructive(question: &str, yes: bool) -> Result<(), CliError> {
    if yes {
        return Ok(());
    }
    if !ui::stdin_is_interactive() {
        return Err(CliError::Usage(
            "refusing to delete without confirmation; pass --yes".to_owned(),
        ));
    }
    if ui::confirm(question)? {
        Ok(())
    } else {
        Err(CliError::Cancelled)
    }
}

/// Validate a YYYY-MM-DD date before sending it.
pub(crate) fn parse_date(value: &str, flag: &str) -> Result<String, CliError> {
    chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map(|date| date.format("%Y-%m-%d").to_string())
        .map_err(|_| CliError::Usage(format!("{flag} must be a date like 2026-01-31")))
}

pub(crate) fn date_range_query(
    range: &crate::args::DateRange,
) -> Result<Vec<(&'static str, String)>, CliError> {
    let mut query = Vec::new();
    if let Some(from) = &range.from {
        query.push(("from", parse_date(from, "--from")?));
    }
    if let Some(to) = &range.to {
        query.push(("to", parse_date(to, "--to")?));
    }
    Ok(query)
}

/// Format an RFC 3339 timestamp as local `YYYY-MM-DD HH:MM`.
pub(crate) fn local_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| ui::terminal_text(value))
}

/// Shorten a long identifier such as an xpub or hash for tables.
pub(crate) fn ellipsize(value: &str, keep: usize) -> String {
    let count = value.chars().count();
    if count <= keep * 2 + 1 {
        return value.to_owned();
    }
    let head: String = value.chars().take(keep).collect();
    let tail: String = value.chars().skip(count - keep).collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::{ellipsize, resolve};

    fn items() -> Vec<(&'static str, &'static str, u8)> {
        vec![
            ("01JC2XAAAA", "Cold storage", 1),
            ("01JC2XBBBB", "Hot", 2),
            ("01JD9ZCCCC", "hot", 3),
        ]
    }

    #[test]
    fn resolves_by_id_name_or_unique_prefix() {
        assert_eq!(resolve(items(), "01jc2xbbbb", "wallet", "x").ok(), Some(2));
        assert_eq!(
            resolve(items(), "cold STORAGE", "wallet", "x").ok(),
            Some(1)
        );
        assert_eq!(resolve(items(), "01JD", "wallet", "x").ok(), Some(3));
    }

    #[test]
    fn ambiguous_or_missing_matches_explain_themselves() {
        let ambiguous = resolve(items(), "HOT", "wallet", "x")
            .err()
            .map(|e| e.to_string());
        assert!(
            ambiguous.is_some_and(|message| message.contains("matches several wallets")
                && message.contains("01JC2XBBBB"))
        );
        let prefix = resolve(items(), "01JC2X", "wallet", "x")
            .err()
            .map(|e| e.to_string());
        assert!(prefix.is_some_and(|message| message.contains("several")));
        let missing = resolve(items(), "nope", "wallet", "bitgarth wallet list")
            .err()
            .map(|e| e.to_string());
        assert!(missing.is_some_and(|message| message.contains("bitgarth wallet list")));
        assert!(resolve(items(), "01J", "wallet", "x").is_err());
    }

    #[test]
    fn ellipsizes_only_long_values() {
        assert_eq!(ellipsize("abcdef", 3), "abcdef");
        assert_eq!(ellipsize("abcdefghij", 3), "abc…hij");
    }
}

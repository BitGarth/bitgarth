use std::io::Read as _;

use reqwest::Method;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::args::{SecretCommand, SecretName, SettingKey, SettingsCommand};
use crate::context::{CliError, Context, decode};
use crate::model::{Secrets, Settings};
use crate::ui::{self, Align, Cell, Style, Table};

pub(super) fn run(context: &Context, command: SettingsCommand) -> Result<(), CliError> {
    let session = context.session()?;
    match command {
        SettingsCommand::List => {
            let raw = session.get(&["settings"], &[])?;
            if context.json() {
                return context.print_json(&raw);
            }
            print!("{}", settings_table(&decode(raw)?).render());
            ui::hint(
                "Change one with `bitgarth settings set <KEY> <VALUE>`; API keys live under `bitgarth secret`.",
            );
            Ok(())
        }
        SettingsCommand::Get { key } => {
            let raw = session.get(&["settings"], &[])?;
            let settings: Settings = decode(raw)?;
            let (value, _) = setting_value(&settings, key);
            if context.json() {
                return context.print_json(&json!({ "key": key.api_name(), "value": value }));
            }
            println!(
                "{}",
                ui::terminal_text(
                    &value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned)
                )
            );
            Ok(())
        }
        SettingsCommand::Set { key, value } => {
            let value = setting_input(key, &value)?;
            let raw = session.send(
                Method::PUT,
                &["settings", key.api_name()],
                &json!({ "value": value }),
            )?;
            if context.json() {
                return context.print_json(&raw);
            }
            let settings: Settings = decode(raw)?;
            let (shown, _) = setting_value(&settings, key);
            ui::success(&format!(
                "Set {} to {}",
                key.api_name(),
                shown.as_str().unwrap_or(&shown.to_string())
            ));
            if key == SettingKey::PriceFetching && value == Value::Bool(true) {
                ui::hint(
                    "Prices download in the background; values appear in `bitgarth balance` shortly.",
                );
            }
            Ok(())
        }
        SettingsCommand::Unset { key } => {
            if !matches!(
                key,
                SettingKey::MempoolUrl | SettingKey::EtherscanUrl | SettingKey::HledgerPrefix
            ) {
                return Err(CliError::Usage(format!(
                    "{} always has a value; change it with `bitgarth settings set {} <VALUE>`",
                    key.api_name(),
                    key.api_name()
                )));
            }
            let raw = session.send(
                Method::PUT,
                &["settings", key.api_name()],
                &json!({ "value": Value::Null }),
            )?;
            if context.json() {
                return context.print_json(&raw);
            }
            let settings: Settings = decode(raw)?;
            let (shown, _) = setting_value(&settings, key);
            ui::success(&format!(
                "Reset {} to the default ({})",
                key.api_name(),
                shown.as_str().unwrap_or("none")
            ));
            Ok(())
        }
    }
}

/// Turn CLI text into the JSON value the server expects for `key`.
fn setting_input(key: SettingKey, value: &str) -> Result<Value, CliError> {
    let value = value.trim();
    if key == SettingKey::PriceFetching {
        return match value.to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" | "enable" | "enabled" => Ok(Value::Bool(true)),
            "off" | "false" | "no" | "0" | "disable" | "disabled" => Ok(Value::Bool(false)),
            _ => Err(CliError::Usage(
                "price-fetching must be on or off".to_owned(),
            )),
        };
    }
    Ok(Value::String(value.to_owned()))
}

/// The displayed value of `key` and whether it is a default.
fn setting_value(settings: &Settings, key: SettingKey) -> (Value, bool) {
    let explicit = |value: &Option<String>| match value {
        Some(value) => (json!(value), false),
        None => (Value::Null, true),
    };
    match key {
        SettingKey::Currency => settings
            .currency
            .as_ref()
            .map_or((json!("USD"), true), |value| (json!(value), false)),
        SettingKey::Timezone => explicit(&settings.timezone),
        SettingKey::NumberFormat => settings
            .number_format
            .as_ref()
            .map_or((json!("dot_comma"), true), |value| (json!(value), false)),
        SettingKey::DateFormat => settings
            .date_format
            .as_ref()
            .map_or((json!("mdy_12"), true), |value| (json!(value), false)),
        SettingKey::PriceFetching => (
            json!(if settings.price_fetching { "on" } else { "off" }),
            false,
        ),
        SettingKey::MempoolUrl => match &settings.mempool_url {
            Some(url) => (json!(url), false),
            None => (json!(settings.effective_mempool_url), true),
        },
        SettingKey::EtherscanUrl => match &settings.etherscan_url {
            Some(url) => (json!(url), false),
            None => (json!(settings.effective_etherscan_url), true),
        },
        SettingKey::HledgerPrefix => match &settings.hledger_prefix {
            Some(prefix) => (json!(prefix), false),
            None => (json!(settings.default_hledger_prefix), true),
        },
    }
}

fn settings_table(settings: &Settings) -> Table {
    let mut table = Table::new(&[
        ("SETTING", Align::Left),
        ("VALUE", Align::Left),
        ("", Align::Left),
    ]);
    for key in [
        SettingKey::Currency,
        SettingKey::Timezone,
        SettingKey::NumberFormat,
        SettingKey::DateFormat,
        SettingKey::PriceFetching,
        SettingKey::MempoolUrl,
        SettingKey::EtherscanUrl,
        SettingKey::HledgerPrefix,
    ] {
        let (value, is_default) = setting_value(settings, key);
        let shown = match &value {
            Value::String(text) => text.clone(),
            Value::Null => "not set (web: your browser's; CLI reports: UTC)".to_owned(),
            other => other.to_string(),
        };
        table.row(vec![
            Cell::styled(key.api_name(), Style::Bold),
            Cell::new(shown),
            Cell::styled(if is_default { "default" } else { "" }, Style::Dim),
        ]);
    }
    for (name, configured) in [
        ("etherscan-api-key", settings.secrets.etherscan_api_key),
        ("coingecko-api-key", settings.secrets.coingecko_api_key),
    ] {
        table.row(vec![
            Cell::styled(name, Style::Bold),
            secret_cell(configured),
            Cell::styled("secret", Style::Dim),
        ]);
    }
    table
}

fn secret_cell(configured: bool) -> Cell {
    if configured {
        Cell::styled("••••••• (stored)", Style::Green)
    } else {
        Cell::styled("not set", Style::Dim)
    }
}

pub(super) fn secret(context: &Context, command: SecretCommand) -> Result<(), CliError> {
    let session = context.session()?;
    match command {
        SecretCommand::List => {
            let settings = session.get(&["settings"], &[])?;
            let raw = settings.get("secrets").cloned().unwrap_or_default();
            if context.json() {
                return context.print_json(&raw);
            }
            let secrets: Secrets = decode(raw)?;
            let mut table = Table::new(&[("SECRET", Align::Left), ("STATUS", Align::Left)]);
            table.row(vec![
                Cell::styled("etherscan-api-key", Style::Bold),
                secret_cell(secrets.etherscan_api_key),
            ]);
            table.row(vec![
                Cell::styled("coingecko-api-key", Style::Bold),
                secret_cell(secrets.coingecko_api_key),
            ]);
            print!("{}", table.render());
            ui::hint("Stored values are never shown or sent back to clients.");
            Ok(())
        }
        SecretCommand::Set { name } => {
            let value = read_secret(name)?;
            if name == SecretName::EtherscanApiKey && !context.json() {
                ui::info("Checking the key with Etherscan…");
            }
            let raw = session.send(
                Method::PUT,
                &["secrets", name.api_name()],
                &json!({ "value": value.as_str() }),
            )?;
            if context.json() {
                return context.print_json(&raw);
            }
            ui::success(&format!("Stored {}", name.api_name()));
            Ok(())
        }
        SecretCommand::Delete { name } => {
            let raw = session.delete(&["secrets", name.api_name()])?;
            if context.json() {
                return context.print_json(&raw);
            }
            ui::success(&format!("Removed {}", name.api_name()));
            Ok(())
        }
    }
}

/// Prompt without echo on a terminal; otherwise read all of stdin.
fn read_secret(name: SecretName) -> Result<Zeroizing<String>, CliError> {
    let value = if ui::stdin_is_interactive() {
        Zeroizing::new(
            ui::prompt_secret(&format!("Paste your {}: ", name.api_name()))?
                .ok_or(CliError::Cancelled)?,
        )
    } else {
        let mut input = Zeroizing::new(String::new());
        std::io::stdin()
            .take(64 * 1024)
            .read_to_string(&mut input)
            .map_err(|_| CliError::Usage("could not read the secret from stdin".to_owned()))?;
        input
    };
    let trimmed = Zeroizing::new(value.trim().to_owned());
    if trimmed.is_empty() {
        return Err(CliError::Usage(format!(
            "no {} given; pipe it on stdin or run this in a terminal",
            name.api_name()
        )));
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::setting_input;
    use crate::args::SettingKey;
    use serde_json::json;

    #[test]
    fn price_fetching_accepts_friendly_switch_words() {
        for on in ["on", "TRUE", "yes", "enable"] {
            assert_eq!(
                setting_input(SettingKey::PriceFetching, on).ok(),
                Some(json!(true))
            );
        }
        assert_eq!(
            setting_input(SettingKey::PriceFetching, "off").ok(),
            Some(json!(false))
        );
        assert!(setting_input(SettingKey::PriceFetching, "maybe").is_err());
        assert_eq!(
            setting_input(SettingKey::Currency, " eur ").ok(),
            Some(json!("eur"))
        );
    }
}

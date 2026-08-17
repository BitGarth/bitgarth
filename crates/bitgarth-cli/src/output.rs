use std::fmt;

use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct WalletBalances {
    wallets: Vec<WalletBalance>,
}

#[derive(Deserialize)]
struct WalletBalance {
    id: String,
    name: String,
    balances: Vec<AssetBalance>,
}

#[derive(Deserialize)]
struct AssetBalance {
    asset_id: String,
    network_id: String,
    unit: String,
    #[serde(deserialize_with = "deserialize_nullable_string")]
    amount: Option<String>,
    status: BalanceStatus,
    reasons: Vec<BalanceReason>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum BalanceStatus {
    Final,
    Provisional,
    Unknown,
}

impl BalanceStatus {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Final => "final",
            Self::Provisional => "provisional",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Deserialize, Ord, PartialOrd, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum BalanceReason {
    FirstSuccessfulSyncPending,
    InactiveAccountNotSyncing,
}

impl BalanceReason {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::FirstSuccessfulSyncPending => "first_successful_sync_pending",
            Self::InactiveAccountNotSyncing => "inactive_account_not_syncing",
        }
    }
}

impl WalletBalances {
    pub(crate) fn validate(&self) -> Result<(), OutputError> {
        for wallet in &self.wallets {
            validate_text(&wallet.id, "wallet id")?;
            validate_text(&wallet.name, "wallet name")?;
            for balance in &wallet.balances {
                validate_text(&balance.asset_id, "asset id")?;
                validate_text(&balance.network_id, "network id")?;
                validate_text(&balance.unit, "asset unit")?;
                if matches!(balance.status, BalanceStatus::Unknown) != balance.amount.is_none() {
                    return Err(OutputError("balance amount does not match reliability"));
                }
                if balance
                    .amount
                    .as_deref()
                    .is_some_and(|value| !valid_decimal(value))
                {
                    return Err(OutputError("invalid balance amount"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn render(mut self) -> Result<String, OutputError> {
        self.validate()?;
        self.wallets
            .sort_by(|left, right| (&left.name, &left.id).cmp(&(&right.name, &right.id)));
        if self.wallets.is_empty() {
            return Ok("No wallets found.\n".to_owned());
        }

        let mut rendered = String::new();
        for (wallet_index, wallet) in self.wallets.iter_mut().enumerate() {
            if wallet_index != 0 {
                rendered.push('\n');
            }
            rendered.push_str("Wallet: ");
            rendered.push_str(&wallet.name);
            rendered.push_str(" [");
            rendered.push_str(&wallet.id);
            rendered.push_str("]\n");
            if wallet.balances.is_empty() {
                rendered.push_str("No balances.\n");
                continue;
            }

            wallet.balances.sort_by(|left, right| {
                (&left.asset_id, &left.network_id, &left.unit).cmp(&(
                    &right.asset_id,
                    &right.network_id,
                    &right.unit,
                ))
            });
            rendered.push_str("Asset\tNetwork\tUnit\tAmount\tReliability\tReasons\n");
            for balance in &mut wallet.balances {
                balance.reasons.sort();
                let reasons = if balance.reasons.is_empty() {
                    "-".to_owned()
                } else {
                    balance
                        .reasons
                        .iter()
                        .map(BalanceReason::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                };
                rendered.push_str(&balance.asset_id);
                rendered.push('\t');
                rendered.push_str(&balance.network_id);
                rendered.push('\t');
                rendered.push_str(&balance.unit);
                rendered.push('\t');
                rendered.push_str(balance.amount.as_deref().unwrap_or("unknown"));
                rendered.push('\t');
                rendered.push_str(balance.status.as_str());
                rendered.push('\t');
                rendered.push_str(&reasons);
                rendered.push('\n');
            }
        }
        Ok(rendered)
    }
}

fn deserialize_nullable_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}

fn validate_text(value: &str, field: &'static str) -> Result<(), OutputError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(OutputError(field));
    }
    Ok(())
}

fn valid_decimal(value: &str) -> bool {
    let mut parts = value.split('.');
    let Some(integer) = parts.next() else {
        return false;
    };
    if integer.is_empty() || !integer.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    match (parts.next(), parts.next()) {
        (None, None) => true,
        (Some(fraction), None) => {
            !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit())
        }
        _ => false,
    }
}

pub(crate) struct OutputError(&'static str);

impl fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "incompatible wallet balance response: {}",
            self.0
        )
    }
}

impl fmt::Debug for OutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for OutputError {}

#[cfg(test)]
mod tests {
    use super::WalletBalances;

    #[test]
    fn shared_fixture_renders_every_reliability_state_and_empty_wallet() {
        let balances: Result<WalletBalances, _> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/client_api/wallet-balances.json"
        ));
        assert!(balances.is_ok());
        let Ok(balances) = balances else { return };
        let rendered = balances.render();
        assert!(rendered.is_ok());
        let Ok(rendered) = rendered else { return };

        assert!(rendered.contains("Wallet: Alpha [01ARZ3NDEKTSV4RRFFQ69G5FAV]"));
        assert!(rendered.contains("bitcoin\tbitcoin-mainnet\tBTC\t1.2345\tfinal\t-"));
        assert!(rendered.contains(
            "ethereum\tethereum-mainnet\tETH\t2\tprovisional\tfirst_successful_sync_pending,inactive_account_not_syncing"
        ));
        assert!(rendered.contains("usd-coin\tpolygon-mainnet\tUSDC\tunknown\tunknown\t-"));
        assert!(rendered.contains("Wallet: Empty [01ARZ3NDEKTSV4RRFFQ69G5FAW]\nNo balances."));
        assert!(rendered.contains("Wallet: Manual [01ARZ3NDEKTSV4RRFFQ69G5FAX]"));
    }

    #[test]
    fn empty_response_is_successful_and_unknown_fields_are_tolerated() {
        let balances: Result<WalletBalances, _> =
            serde_json::from_str(r#"{"wallets":[],"future_field":true}"#);
        assert!(balances.is_ok());
        let Ok(balances) = balances else { return };
        assert_eq!(
            balances.render().ok().as_deref(),
            Some("No wallets found.\n")
        );
    }

    #[test]
    fn missing_or_incompatible_required_fields_are_rejected() {
        for json in [
            r#"{"wallets":[{"id":"w","name":"Wallet","balances":[{"asset_id":"a","network_id":"n","unit":"U","status":"final","reasons":[]}]}]}"#,
            r#"{"wallets":[{"id":"w","name":"Wallet","balances":[{"asset_id":"a","network_id":"n","unit":"U","amount":"1","status":"future","reasons":[]}]}]}"#,
            r#"{"wallets":[{"id":"w","name":"Wallet","balances":[{"asset_id":"a","network_id":"n","unit":"U","amount":"1","status":"final","reasons":["future_reason"]}]}]}"#,
            r#"{"wallets":[{"id":"w","name":"Wallet","balances":[{"asset_id":"a","network_id":"n","unit":"U","amount":null,"status":"final","reasons":[]}]}]}"#,
        ] {
            let rejected = match serde_json::from_str::<WalletBalances>(json) {
                Ok(balances) => balances.validate().is_err(),
                Err(_) => true,
            };
            assert!(rejected);
        }
    }

    #[test]
    fn rendering_sorts_wallets_balances_and_reasons() {
        let json = r#"{
            "wallets": [
                {"id":"2","name":"Zulu","balances":[]},
                {"id":"1","name":"Alpha","balances":[
                    {"asset_id":"z","network_id":"other-testnet","unit":"Z","amount":"2.00","status":"provisional","reasons":["inactive_account_not_syncing","first_successful_sync_pending"]},
                    {"asset_id":"manual-asset","network_id":"polygon-mainnet","unit":"MAN","amount":null,"status":"unknown","reasons":[]}
                ]}
            ]
        }"#;
        let balances: Result<WalletBalances, _> = serde_json::from_str(json);
        assert!(balances.is_ok());
        let Ok(balances) = balances else { return };
        let rendered = balances.render();
        assert!(rendered.is_ok());
        let Ok(rendered) = rendered else { return };
        assert!(rendered.find("Wallet: Alpha").is_some_and(|alpha| {
            rendered
                .find("Wallet: Zulu")
                .is_some_and(|zulu| alpha < zulu)
        }));
        assert!(rendered.find("manual-asset").is_some_and(|manual| {
            rendered
                .find("z\tother-testnet")
                .is_some_and(|z| manual < z)
        }));
        assert!(rendered.contains("first_successful_sync_pending,inactive_account_not_syncing"));
        assert!(rendered.contains("manual-asset\tpolygon-mainnet\tMAN\tunknown\tunknown"));
    }
}

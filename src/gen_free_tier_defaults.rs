//! Developer-run generator for assets/entitlements/free_tier_defaults.json.
//! Run: cargo run --features server -- gen-free-tier-defaults [--out PATH]

#![cfg(feature = "server")]

use crate::models::UserId;
use crate::payments::free_tier::free_observation_from_product_options;
use chrono::Utc;

const DEFAULT_OUT: &str = "assets/entitlements/free_tier_defaults.json";

pub(crate) fn maybe_run_from_args() -> Result<bool, String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("gen-free-tier-defaults") => {}
        _ => return Ok(false),
    }
    let parsed = parse_args(args)?;

    let client = crate::payments::client::BitGarthCentralClient::new(UserId::new())
        .map_err(|err| format!("central client: {err}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("tokio runtime: {err}"))?;
    let options = runtime
        .block_on(client.payment_product_options())
        .map_err(|err| format!("product options: {err}"))?;
    let captured_at = Utc::now();
    let observation = free_observation_from_product_options(&options, captured_at)
        .ok_or_else(|| "product options did not include a valid v3 free tier".to_string())?;
    let snapshot = serde_json::json!({
        "captured_at": observation.observed_at,
        "capability_schema_version": observation.capability_schema_version,
        "capabilities": observation.capabilities,
    });
    let mut json = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| format!("serialize free tier defaults: {err}"))?;
    json.push('\n');
    std::fs::write(&parsed.out, json).map_err(|err| format!("write {}: {err}", parsed.out))?;
    eprintln!("wrote {}", parsed.out);
    Ok(true)
}

#[derive(Debug, PartialEq, Eq)]
struct FreeTierDefaultsArgs {
    out: String,
}

fn usage() -> String {
    "usage: gen-free-tier-defaults [--out PATH]".to_string()
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<FreeTierDefaultsArgs, String> {
    let mut out = None;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--out requires a path".to_string())?;
                if out.replace(path).is_some() {
                    return Err("--out may only be supplied once".to_string());
                }
            }
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    Ok(FreeTierDefaultsArgs {
        out: out.unwrap_or_else(|| DEFAULT_OUT.to_string()),
    })
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    #[test]
    fn default_output_path_is_entitlements_asset() {
        assert_eq!(
            super::DEFAULT_OUT,
            "assets/entitlements/free_tier_defaults.json"
        );
    }

    #[test]
    fn parse_args_uses_default_output_path() {
        let parsed = super::parse_args(Vec::<String>::new().into_iter()).expect("parse args");

        assert_eq!(parsed.out, super::DEFAULT_OUT);
    }

    #[test]
    fn parse_args_accepts_custom_output_path() {
        let parsed = super::parse_args(
            vec![
                "--out".to_string(),
                "tmp/free_tier_defaults.json".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        assert_eq!(parsed.out, "tmp/free_tier_defaults.json");
    }
}

use reqwest::Method;
use serde_json::json;

use super::date_range_query;
use crate::args::PriceCommand;
use crate::context::{CliError, Context, Session, decode};
use crate::model::{PriceEntry, PriceHistory, Settings};
use crate::ui::{self, Align, Cell, Style, Table};

const PAGE_SIZE: u32 = 50;

pub(super) fn run(context: &Context, command: PriceCommand) -> Result<(), CliError> {
    match command {
        PriceCommand::History {
            asset,
            range,
            page,
            oldest_first,
            source,
        } => {
            let mut query = date_range_query(&range)?;
            if let Some(asset) = asset {
                query.push(("asset", asset));
            }
            query.push(("page", page.to_string()));
            query.push((
                "sort",
                if oldest_first {
                    "OldestFirst"
                } else {
                    "NewestFirst"
                }
                .to_owned(),
            ));
            query.push(("source", source.api_name().to_owned()));
            let session = context.session()?;
            let raw = session.get(&["prices", "history"], &query)?;
            if context.json() {
                return context.print_json(&raw);
            }
            history(&decode(raw)?);
            Ok(())
        }
        PriceCommand::Set {
            asset,
            price,
            at,
            currency,
            note,
        } => {
            let session = context.session()?;
            let currency = quote_currency(&session, currency)?;
            let body = json!({
                "subject": { "kind": "catalog_asset", "id": asset },
                "quote_currency": currency,
                "price_time_local": local_timestamp(&at)?,
                "price": price.trim(),
                "source_note": note,
            });
            let response = session.send(Method::PUT, &["prices", "overrides"], &body)?;
            if context.json() {
                return context.print_json(&response);
            }
            ui::success(&format!(
                "Saved your price for {asset}: {} at {at}",
                ui::amount(price.trim(), &currency)
            ));
            Ok(())
        }
        PriceCommand::Delete {
            asset,
            at,
            currency,
        } => {
            let session = context.session()?;
            let currency = quote_currency(&session, currency)?;
            let body = json!({
                "subject": { "kind": "catalog_asset", "id": asset },
                "quote_currency": currency,
                "price_time_local": local_timestamp(&at)?,
            });
            session.call(Method::DELETE, &["prices", "overrides"], &[], Some(&body))?;
            if context.json() {
                return context.print_json(&json!({ "deleted": body }));
            }
            ui::success(&format!(
                "Removed your {currency} price for {asset} at {at}"
            ));
            Ok(())
        }
    }
}

/// Use the given currency, else the account's reporting currency.
fn quote_currency(session: &Session, currency: Option<String>) -> Result<String, CliError> {
    if let Some(currency) = currency {
        return Ok(currency.trim().to_ascii_uppercase());
    }
    let settings: Settings = decode(session.get(&["settings"], &[])?)?;
    Ok(settings.currency.unwrap_or_else(|| "USD".to_owned()))
}

/// Accept `YYYY-MM-DD` or `YYYY-MM-DD HH:MM[:SS]` and produce the server's
/// local timestamp format.
fn local_timestamp(value: &str) -> Result<String, CliError> {
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(value.trim()) {
        return Ok(time.to_rfc3339());
    }
    let value = value.trim().replace('T', " ");
    let parsed = chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M"))
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d")
                .map(|date| date.and_time(chrono::NaiveTime::MIN))
        })
        .map_err(|_| {
            CliError::Usage("--at must look like 2026-01-31 or 2026-01-31 17:00".to_owned())
        })?;
    Ok(parsed.format("%Y-%m-%dT%H:%M:%S").to_string())
}

fn history(history: &PriceHistory) {
    let Some(selected) = &history.selected else {
        println!("You do not hold any assets with prices yet.");
        return;
    };
    let name = history
        .assets
        .iter()
        .find(|asset| &asset.key == selected)
        .map_or(selected.as_str(), |asset| asset.name.as_str());
    print!(
        "{}",
        ui::heading(
            name,
            Some(&format!(
                "in {}, {} → {}",
                history.currency, history.resolved_from, history.resolved_to
            ))
        )
    );
    let mut table = Table::new(&[
        ("DATE", Align::Left),
        ("PRICE", Align::Right),
        ("SOURCE", Align::Left),
    ]);
    for entry in &history.entries {
        match entry {
            PriceEntry::Daily { date_utc, price } => table.row(vec![
                Cell::new(date_utc.as_str()),
                Cell::new(ui::amount(price, &history.currency)),
                Cell::styled("CoinGecko", Style::Dim),
            ]),
            PriceEntry::YourPrice { at, price, note } => table.row(vec![
                Cell::new(at.as_str()),
                Cell::styled(ui::amount(price, &history.currency), Style::Bold),
                Cell::styled(
                    match note {
                        Some(note) => format!("yours · {note}"),
                        None => "yours".to_owned(),
                    },
                    Style::Cyan,
                ),
            ]),
            PriceEntry::Gap { from, to } => table.row(vec![
                Cell::styled(
                    if from == to {
                        from.clone()
                    } else {
                        format!("{from} → {to}")
                    },
                    Style::Dim,
                ),
                Cell::styled("—", Style::Dim),
                Cell::styled("no price", Style::Yellow),
            ]),
        }
    }
    if table.is_empty() {
        println!("No prices in this period.");
    } else {
        print!("{}", table.render());
    }
    let pages = history.total_entries.div_ceil(PAGE_SIZE);
    if history.page < pages {
        ui::hint(&format!(
            "Page {} of {pages}. Next: add --page {}",
            history.page,
            history.page + 1
        ));
    }
    if !history.prices_enabled {
        ui::hint(
            "CoinGecko prices are off; turn them on with `bitgarth settings set price-fetching on`.",
        );
    }
    if history.assets.len() > 1 {
        let others: Vec<&str> = history
            .assets
            .iter()
            .filter(|asset| &asset.key != selected)
            .map(|asset| asset.key.as_str())
            .collect();
        ui::hint(&format!("Other assets: {}", others.join(", ")));
    }
}

#[cfg(test)]
mod tests {
    use super::local_timestamp;

    #[test]
    fn accepts_dates_and_times_in_the_server_format() {
        assert_eq!(
            local_timestamp("2026-01-31").ok().as_deref(),
            Some("2026-01-31T00:00:00")
        );
        assert_eq!(
            local_timestamp("2026-01-31 17:05").ok().as_deref(),
            Some("2026-01-31T17:05:00")
        );
        assert_eq!(
            local_timestamp("2026-01-31T17:05:09").ok().as_deref(),
            Some("2026-01-31T17:05:09")
        );
        assert_eq!(
            local_timestamp("2026-01-31T17:05:09+03:00").ok().as_deref(),
            Some("2026-01-31T17:05:09+03:00")
        );
        assert!(local_timestamp("31/01/2026").is_err());
    }
}

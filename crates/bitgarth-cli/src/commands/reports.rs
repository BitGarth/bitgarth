use super::{date_range_query, find_wallet, load_wallets};
use crate::args::ReportCommand;
use crate::context::{CliError, Context, decode};
use crate::model::{HoldingsReport, ReportAccess, WalletReport};
use crate::ui::{self, Align, Cell, Style, Table};

pub(super) fn run(context: &Context, command: ReportCommand) -> Result<(), CliError> {
    match command {
        ReportCommand::Holdings { range } => {
            let query = date_range_query(&range)?;
            let session = context.session()?;
            let raw = session.get(&["reports", "holdings"], &query)?;
            if context.json() {
                return context.print_json(&raw);
            }
            let settings: crate::model::Settings = decode(session.get(&["settings"], &[])?)?;
            holdings(decode(raw)?, settings.currency.as_deref().unwrap_or("USD"));
            Ok(())
        }
        ReportCommand::Wallet { wallet, range } => {
            let query = date_range_query(&range)?;
            let session = context.session()?;
            let (_, wallets) = load_wallets(&session)?;
            let wallet = find_wallet(&wallets, &wallet)?;
            let raw = session.get(&["wallets", &wallet.id, "report"], &query)?;
            if context.json() {
                return context.print_json(&raw);
            }
            wallet_report(decode(raw)?);
            Ok(())
        }
    }
}

fn clamp_notice(access: &ReportAccess) {
    if access.range_clamped {
        ui::warn(&format!(
            "Your plan limits reports to dates from {}; the period was shortened.",
            access.effective_from
        ));
    }
}

fn fiat_cell(amount: Option<&crate::model::Amount>) -> Cell {
    amount.map_or_else(
        || Cell::styled("needs price", Style::Yellow),
        |amount| Cell::new(ui::group_thousands(&amount.formatted_value)),
    )
}

fn holdings(report: HoldingsReport, currency: &str) {
    print!(
        "{}",
        ui::heading(
            "Holdings",
            Some(&format!(
                "in {currency}, {} → {}",
                report.resolved_from, report.resolved_to
            ))
        )
    );
    clamp_notice(&report.access);
    if report.wallets.is_empty() {
        println!("No wallets yet.");
        return;
    }
    let mut table = Table::new(&[
        ("WALLET", Align::Left),
        ("OPENING", Align::Right),
        ("CLOSING", Align::Right),
        ("CHANGE", Align::Right),
        ("", Align::Right),
    ]);
    let mut missing = false;
    for row in &report.wallets {
        missing |= row.opening_fiat.is_none() || row.closing_fiat.is_none();
        let change_style = match row
            .change_fiat
            .as_ref()
            .map(|change| change.formatted_value.starts_with('-'))
        {
            Some(true) => Style::Red,
            Some(false) => Style::Green,
            None => Style::Dim,
        };
        table.row(vec![
            Cell::styled(row.wallet_label.as_str(), Style::Bold),
            fiat_cell(row.opening_fiat.as_ref()),
            fiat_cell(row.closing_fiat.as_ref()),
            Cell::styled(
                row.change_fiat.as_ref().map_or_else(String::new, |change| {
                    ui::group_thousands(&change.formatted_value)
                }),
                change_style,
            ),
            Cell::styled(
                row.change_percent
                    .as_deref()
                    .map_or_else(String::new, |percent| format!("{percent}%")),
                change_style,
            ),
        ]);
    }
    print!("{}", table.render());
    if missing {
        ui::hint(
            "Some values need a price: turn on `bitgarth settings set price-fetching on` or add your own with `bitgarth price set`.",
        );
    }
}

fn wallet_report(report: WalletReport) {
    print!(
        "{}",
        ui::heading(
            &report.wallet_label,
            Some(&format!(
                "{} → {}",
                report.resolved_from, report.resolved_to
            ))
        )
    );
    clamp_notice(&report.access);
    let mut table = Table::new(&[
        ("ACCOUNT", Align::Left),
        ("OPENING", Align::Right),
        ("CLOSING", Align::Right),
    ]);
    for row in &report.accounts {
        let balance = |amount: Option<&crate::model::Amount>| {
            amount.map_or_else(
                || Cell::styled("unknown", Style::Yellow),
                |amount| Cell::new(ui::amount(&amount.formatted_value, &row.unit_code)),
            )
        };
        table.row(vec![
            Cell::styled(row.account_label.as_str(), Style::Bold),
            balance(row.opening_balance.as_ref()),
            balance(row.closing_balance.as_ref()),
        ]);
    }
    if table.is_empty() {
        println!("This wallet has no accounts.");
    } else {
        print!("{}", table.render());
    }
}

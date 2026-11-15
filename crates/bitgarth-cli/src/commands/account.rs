use reqwest::Method;
use serde_json::{Value, json};

use super::wallet::account_cells;
use super::{
    confirm_destructive, ellipsize, find_account, find_wallet, load_wallets, local_time, parse_date,
};
use crate::args::{AccountCommand, AddAccountCommand, AssetCommand, Destination, WalletChoice};
use crate::context::{CliError, Context, Session, decode};
use crate::model::{
    Account, Addresses, AssetSearch, AssetSearchRow, Created, History, Wallets, XpubInspection,
    title_case,
};
use crate::ui::{self, Align, Cell, Details, Style, Table};

pub(super) fn run(context: &Context, command: AccountCommand) -> Result<(), CliError> {
    match command {
        AccountCommand::List { wallet } => list(context, wallet.as_deref()),
        AccountCommand::View { account } => view(context, &account),
        AccountCommand::Add(command) => add(context, command),
        AccountCommand::Rename { account, name } => rename(context, &account, &name),
        AccountCommand::Move {
            account,
            destination,
        } => move_account(context, &account, &destination),
        AccountCommand::Delete { account, yes } => delete(context, &account, yes),
        AccountCommand::Addresses { account, page } => addresses(context, &account, page),
        AccountCommand::Transactions {
            account,
            page,
            oldest_first,
            from,
            to,
        } => transactions(context, &account, page, oldest_first, from, to),
        AccountCommand::SetBalance {
            account,
            amount,
            date,
            note,
        } => set_balance(context, &account, &amount, date, note),
        AccountCommand::UnsetBalance { account, date } => unset_balance(context, &account, &date),
    }
}

fn list(context: &Context, wallet: Option<&str>) -> Result<(), CliError> {
    let session = context.session()?;
    let (raw, wallets) = load_wallets(&session)?;
    let only = wallet
        .map(|query| find_wallet(&wallets, query))
        .transpose()?;
    let rows: Vec<_> = wallets
        .wallets
        .iter()
        .filter(|candidate| only.is_none_or(|only| only.id == candidate.id))
        .flat_map(|wallet| wallet.accounts.iter().map(move |account| (wallet, account)))
        .collect();
    if context.json() {
        let accounts: Vec<Value> = raw["wallets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|candidate| only.is_none_or(|only| candidate["id"] == only.id.as_str()))
            .flat_map(|wallet| {
                wallet["accounts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|account| {
                        let mut account = account.clone();
                        account["wallet_id"] = wallet["id"].clone();
                        account["wallet_label"] = wallet["label"].clone();
                        account
                    })
            })
            .collect();
        return context.print_json(&json!(accounts));
    }
    if rows.is_empty() {
        println!("No accounts yet.");
        ui::hint("Add one with `bitgarth account add --help`.");
        return Ok(());
    }
    let mut table = Table::new(&[
        ("ACCOUNT", Align::Left),
        ("TYPE", Align::Left),
        ("BALANCE", Align::Right),
        ("VALUE", Align::Right),
        ("WALLET", Align::Left),
        ("ID", Align::Left),
    ]);
    for (wallet, account) in rows {
        let summary = account.summary();
        let mut cells = account_cells(&summary);
        if let Some(first) = cells.first_mut() {
            *first = Cell::styled(summary.label, Style::Bold);
        }
        cells.push(Cell::new(wallet.label.as_str()));
        cells.push(Cell::styled(summary.id, Style::Dim));
        table.row(cells);
    }
    print!("{}", table.render());
    Ok(())
}

fn view(context: &Context, query: &str) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let (wallet, account) = find_account(&wallets, query)?;
    let history_raw = session.get(&["accounts", account.id(), "transactions"], &[])?;
    if context.json() {
        let account_raw = session
            .get(&["wallets"], &[])?
            .get("wallets")
            .and_then(Value::as_array)
            .and_then(|all| {
                all.iter()
                    .flat_map(|wallet| wallet["accounts"].as_array().into_iter().flatten())
                    .find(|candidate| candidate["account_id"] == account.id())
                    .cloned()
            })
            .unwrap_or_default();
        return context.print_json(&json!({
            "wallet": { "id": wallet.id, "label": wallet.label },
            "account": account_raw,
            "history": history_raw,
        }));
    }
    let history: History = decode(history_raw)?;
    let summary = account.summary();
    print!(
        "{}",
        ui::heading(
            summary.label,
            Some(&format!("{} in {}", summary.kind, wallet.label))
        )
    );
    let mut details = Details::new();
    details.row("ID", summary.id);
    let balance = summary.balance.map_or_else(
        || "unknown".to_owned(),
        |balance| ui::amount(balance, summary.unit),
    );
    details.row(
        "Balance",
        if summary.provisional {
            format!("{balance} (provisional)")
        } else {
            balance
        },
    );
    if let Some(value) = summary.value {
        details.row(
            "Value",
            format!(
                "{} @ {} per unit",
                ui::fiat(&value.converted_value, &value.currency),
                ui::amount(&value.price, &value.currency)
            ),
        );
    }
    match account {
        Account::Native(native) => {
            let reference = if native.account_reference_kind == "extended_pubkey" {
                "Xpub"
            } else {
                "Address"
            };
            details.row(reference, native.account_reference.as_str());
            if let History::Native(history) = &history {
                details.row(
                    "Network",
                    format!(
                        "{} {}",
                        title_case(&native.asset),
                        title_case(&history.network)
                    ),
                );
                if history.address_scheme != "standard" {
                    details.row("Address type", title_case(&history.address_scheme));
                }
            }
            details.row("Sync", sync_mode(&native.account_mode));
            details.row("Transactions", native.transaction_counts.total.to_string());
        }
        Account::Manual(manual) => {
            details.row(
                "Asset",
                format!(
                    "{} ({})",
                    manual.asset_name,
                    manual.asset_instance_id.display()
                ),
            );
        }
        Account::Custom(_) => {}
    }
    if summary.inactive {
        details.row("Status", "inactive: over your plan's account limit");
    }
    print!("{}", details.render());
    println!();
    match history {
        History::Native(history) => {
            let recent = history
                .pending
                .rows
                .iter()
                .chain(history.confirmed.rows.iter())
                .take(10);
            let mut table = transaction_table();
            for row in recent {
                table.row(transaction_cells(row, &history.unit_code));
            }
            if table.is_empty() {
                println!("{}", ui::paint(Style::Dim, "No transactions yet."));
            } else {
                print!("{}", ui::heading("Recent transactions", None));
                print!("{}", table.render());
                ui::hint(&format!(
                    "All {} transactions: `bitgarth account transactions {}`",
                    history.pending.total + history.confirmed.total,
                    summary.id
                ));
            }
        }
        History::Custom(history) => {
            if history.assertions.rows.is_empty() {
                println!("{}", ui::paint(Style::Dim, "No balance recorded yet."));
                ui::hint(&format!(
                    "Record one: `bitgarth account set-balance {} <AMOUNT>`",
                    summary.id
                ));
            } else {
                print!("{}", ui::heading("Recorded balances", None));
                print!(
                    "{}",
                    assertion_table(&history.assertions.rows, &history.unit_code).render()
                );
            }
        }
    }
    Ok(())
}

fn sync_mode(mode: &str) -> String {
    match mode {
        "transactions" => "balance and transaction history".to_owned(),
        "balance_only" => "balance only (transaction history needs a paid plan)".to_owned(),
        "inactive" => "paused (over your plan's account limit)".to_owned(),
        other => title_case(other),
    }
}

fn transaction_table() -> Table {
    Table::new(&[
        ("DATE", Align::Left),
        ("", Align::Left),
        ("AMOUNT", Align::Right),
        ("FEE", Align::Right),
        ("BALANCE", Align::Right),
        ("STATUS", Align::Left),
        ("TRANSACTION", Align::Left),
    ])
}

fn transaction_cells(row: &crate::model::TransactionRow, unit: &str) -> Vec<Cell> {
    let (arrow, style) = match row.direction.as_str() {
        "incoming" => ("↓ in", Style::Green),
        "outgoing" => ("↑ out", Style::Red),
        _ => ("↔ self", Style::Dim),
    };
    let status_style = match row.status.as_str() {
        "confirmed" => Style::Dim,
        "pending" => Style::Yellow,
        _ => Style::Red,
    };
    vec![
        Cell::new(local_time(&row.occurred_at)),
        Cell::styled(arrow, style),
        Cell::styled(ui::amount(&row.value.formatted_value, unit), style),
        Cell::styled(
            row.fee
                .as_ref()
                .map_or_else(String::new, |fee| group(&fee.formatted_value)),
            Style::Dim,
        ),
        Cell::new(
            row.closing_balance
                .as_ref()
                .map_or_else(String::new, |balance| group(&balance.formatted_value)),
        ),
        Cell::styled(row.status.as_str(), status_style),
        Cell::styled(ellipsize(&row.tx_hash, 8), Style::Dim),
    ]
}

fn group(value: &str) -> String {
    ui::group_thousands(value)
}

fn assertion_table(rows: &[crate::model::AssertionRow], unit: &str) -> Table {
    let mut table = Table::new(&[
        ("DATE", Align::Left),
        ("BALANCE", Align::Right),
        ("NOTE", Align::Left),
    ]);
    for row in rows {
        table.row(vec![
            Cell::new(row.asserted_on.as_str()),
            Cell::new(ui::amount(&row.asserted_balance.formatted_value, unit)),
            Cell::styled(row.note.clone().unwrap_or_default(), Style::Dim),
        ]);
    }
    table
}

fn transactions(
    context: &Context,
    query: &str,
    page: u32,
    oldest_first: bool,
    from: Option<String>,
    to: Option<String>,
) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let (_, account) = find_account(&wallets, query)?;
    let mut params = vec![
        ("confirmed_page", page.to_string()),
        ("pending_page", page.to_string()),
        ("sort", if oldest_first { "asc" } else { "desc" }.to_owned()),
    ];
    if from.is_some() || to.is_some() {
        let manual = !matches!(account, Account::Native(_));
        let filters = json!({
            "status": Value::Null,
            "from_date": from.as_deref().map(|date| day_boundary(date, "--from", false, manual)).transpose()?,
            "to_date": to.as_deref().map(|date| day_boundary(date, "--to", true, manual)).transpose()?,
        });
        params.push(("filters", filters.to_string()));
    }
    let raw = session.get(&["accounts", account.id(), "transactions"], &params)?;
    if context.json() {
        return context.print_json(&raw);
    }
    match decode::<History>(raw)? {
        History::Native(history) => {
            let mut table = transaction_table();
            for row in history.pending.rows.iter().chain(&history.confirmed.rows) {
                table.row(transaction_cells(row, &history.unit_code));
            }
            if table.is_empty() {
                println!(
                    "No transactions{}.",
                    if page > 1 { " on this page" } else { "" }
                );
                if history.account_mode != "transactions" {
                    ui::hint("Transaction history is not synced for this account on your plan.");
                }
                return Ok(());
            }
            print!(
                "{}",
                ui::heading(
                    account.label(),
                    Some(&format!("in {}", history.wallet_label))
                )
            );
            print!("{}", table.render());
            if [&history.pending, &history.confirmed]
                .into_iter()
                .any(|table| table.end > 0 && table.end < table.total)
            {
                ui::hint(&format!("More transactions: add --page {}", page + 1));
            }
        }
        History::Custom(history) => {
            print!(
                "{}",
                ui::heading(
                    account.label(),
                    Some(&format!(
                        "{} in {}",
                        history.asset_name.as_deref().unwrap_or(&history.unit_code),
                        history.wallet_label
                    ))
                )
            );
            if history.assertions.rows.is_empty() {
                println!("No balances recorded yet.");
                return Ok(());
            }
            print!(
                "{}",
                assertion_table(&history.assertions.rows, &history.unit_code).render()
            );
            pagination(
                history.assertions.start,
                history.assertions.end,
                history.assertions.total,
                history.assertions.page,
                &format!("bitgarth account transactions {}", account.id()),
            );
        }
    }
    Ok(())
}

/// Start or end of a calendar day in this computer's timezone, as RFC 3339.
fn day_boundary(
    date: &str,
    flag: &str,
    end_of_day: bool,
    manual: bool,
) -> Result<String, CliError> {
    let day = chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
        .map_err(|_| CliError::Usage(format!("{flag} must be a date like 2026-01-31")))?;
    let time = if end_of_day {
        chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(chrono::NaiveTime::MIN)
    } else {
        chrono::NaiveTime::MIN
    };
    if manual {
        // Assertion dates are calendar days, independent of the workstation timezone.
        return Ok(day.and_time(time).and_utc().to_rfc3339());
    }
    day.and_time(time)
        .and_local_timezone(chrono::Local)
        .earliest()
        .map(|moment| moment.to_rfc3339())
        .ok_or_else(|| CliError::Usage(format!("{flag} is not a valid local date")))
}

fn pagination(start: u32, end: u32, total: u32, page: u32, command: &str) {
    if total > end {
        ui::hint(&format!(
            "Showing {start}–{end} of {total}. Next page: `{command} --page {}`",
            page + 1
        ));
    }
}

// ============ add ============

fn add(context: &Context, command: AddAccountCommand) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let (kind, created) = match command {
        AddAccountCommand::Bitcoin {
            address_or_xpub,
            wallet,
            name,
            scheme,
            network,
        } => {
            let target = wallet_target(&wallets, &wallet, name.as_deref(), "Bitcoin")?;
            let value = address_or_xpub.trim();
            if is_extended_key(value) {
                let scheme = choose_xpub_scheme(context, &session, value, scheme)?;
                let mut body = json!({
                    "extended_pubkey": value,
                    "address_scheme": scheme,
                    "account_label": name,
                });
                merge(&mut body, target);
                (
                    "Bitcoin xpub",
                    session.send(Method::POST, &["accounts", "xpub"], &body)?,
                )
            } else {
                let mut body = json!({
                    "address": value,
                    "network": network.api_name(),
                    "account_label": name,
                });
                merge(&mut body, target);
                (
                    "Bitcoin address",
                    session.send(Method::POST, &["accounts", "bitcoin-address"], &body)?,
                )
            }
        }
        AddAccountCommand::Ethereum {
            address,
            wallet,
            name,
        } => {
            let target = wallet_target(&wallets, &wallet, name.as_deref(), "Ethereum")?;
            let mut body = json!({
                "address": address.trim(),
                "network": "mainnet",
                "account_label": name,
            });
            merge(&mut body, target);
            (
                "Ethereum address",
                session.send(Method::POST, &["accounts", "ethereum-address"], &body)?,
            )
        }
        AddAccountCommand::Manual {
            asset,
            wallet,
            name,
        } => {
            let instance = resolve_catalog_asset(&session, &asset)?;
            let default_wallet = instance.unit_code.clone();
            let target = wallet_target(&wallets, &wallet, name.as_deref(), &default_wallet)?;
            let Some(asset_instance_id) = &instance.asset_instance_id else {
                return Err(CliError::Usage(format!(
                    "{} is only listed on CoinGecko; add it in the web app for now",
                    instance.asset_name
                )));
            };
            let mut body = json!({
                "asset": {
                    "source": "bit_garth_catalog",
                    "asset_instance_id": {
                        "asset_id": asset_instance_id.asset_id,
                        "network_id": asset_instance_id.network_id,
                    }
                },
                "account_label": name,
            });
            merge(&mut body, target);
            (
                "Manual",
                session.send(Method::POST, &["accounts", "manual"], &body)?,
            )
        }
    };
    report_created(context, &session, kind, created)
}

fn merge(body: &mut Value, extra: Value) {
    if let (Some(body), Value::Object(extra)) = (body.as_object_mut(), extra) {
        body.extend(extra);
    }
}

/// `wallet_id` for an existing wallet, else `wallet_label` for a new one.
fn wallet_target(
    wallets: &Wallets,
    choice: &WalletChoice,
    account_name: Option<&str>,
    fallback: &str,
) -> Result<Value, CliError> {
    if let Some(query) = &choice.wallet {
        let wallet = find_wallet(wallets, query)?;
        return Ok(json!({ "wallet_id": wallet.id }));
    }
    let label = choice
        .new_wallet
        .clone()
        .or_else(|| account_name.map(str::to_owned))
        .unwrap_or_else(|| fallback.to_owned());
    Ok(json!({ "wallet_label": label }))
}

fn is_extended_key(value: &str) -> bool {
    [
        "xpub", "ypub", "zpub", "Ypub", "Zpub", "tpub", "upub", "vpub", "Upub", "Vpub",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
        && value.len() > 100
}

fn choose_xpub_scheme(
    context: &Context,
    session: &Session,
    xpub: &str,
    requested: Option<crate::args::BitcoinScheme>,
) -> Result<String, CliError> {
    let inspection: XpubInspection = decode(session.send(
        Method::POST,
        &["xpubs", "inspect"],
        &json!({ "extended_pubkey": xpub }),
    )?)?;
    if let Some(linked) = &inspection.already_linked {
        return Err(CliError::Usage(format!(
            "this key is already tracked in wallet \"{}\" for every address type",
            linked.wallet_label
        )));
    }
    if let Some(requested) = requested {
        return Ok(requested.api_name().to_owned());
    }
    let active: Vec<_> = inspection
        .schemes
        .iter()
        .filter(|scheme| scheme.has_activity == Some(true) && !scheme.already_linked)
        .collect();
    let available = inspection
        .schemes
        .iter()
        .filter(|scheme| !scheme.already_linked);
    let chosen = active
        .first()
        .copied()
        .or_else(|| {
            available
                .clone()
                .find(|scheme| scheme.address_scheme == inspection.suggested_scheme)
        })
        .or_else(|| available.into_iter().next())
        .ok_or_else(|| {
            CliError::Usage("All address types for this key are already tracked".to_owned())
        })?
        .address_scheme
        .clone();
    if !context.json() {
        for scheme in &inspection.schemes {
            let marker = if scheme.address_scheme == chosen {
                "›"
            } else {
                " "
            };
            let activity = match (scheme.already_linked, scheme.has_activity) {
                (true, _) => "already tracked",
                (_, Some(true)) => "has activity",
                (_, Some(false)) => "no activity",
                (_, None) => "activity unknown",
            };
            eprintln!(
                "  {} {:<14} {}  {}",
                ui::epaint(Style::Cyan, marker),
                ui::terminal_text(&title_case(&scheme.address_scheme)),
                ui::epaint(Style::Dim, &ellipsize(&scheme.first_address, 10)),
                ui::epaint(Style::Dim, activity)
            );
        }
        if active.len() > 1 {
            ui::warn(&format!(
                "Several address types have activity; using {}. Add the others with --scheme.",
                title_case(&chosen)
            ));
        }
    }
    Ok(chosen)
}

fn report_created(
    context: &Context,
    session: &Session,
    kind: &str,
    raw: Value,
) -> Result<(), CliError> {
    if context.json() {
        return context.print_json(&raw);
    }
    let created: Created = decode(raw)?;
    let (_, wallets) = load_wallets(session)?;
    let wallet = wallets
        .wallets
        .iter()
        .find(|wallet| wallet.id == created.wallet_id);
    let account = wallet.and_then(|wallet| {
        wallet
            .accounts
            .iter()
            .find(|account| account.id() == created.account_id)
    });
    ui::success(&format!(
        "Added {kind} account {} to wallet {}",
        account.map_or(created.account_id.as_str(), Account::label),
        wallet.map_or(created.wallet_id.as_str(), |wallet| wallet.label.as_str())
    ));
    ui::hint(&format!("ID {}", created.account_id));
    if let Some(notice) = created.account_limit_notice {
        ui::warn(&notice.message);
    } else if created.account_state == "active" && kind != "Manual" {
        ui::hint("Syncing in the background; follow along with `bitgarth sync status`.");
    } else if kind == "Manual" {
        ui::hint(&format!(
            "Record what you hold: `bitgarth account set-balance {} <AMOUNT>`",
            created.account_id
        ));
    }
    Ok(())
}

fn resolve_catalog_asset(session: &Session, query: &str) -> Result<AssetSearchRow, CliError> {
    let search_text = query.split('@').next().unwrap_or(query);
    let search: AssetSearch =
        decode(session.get(&["assets", "search"], &[("query", search_text.to_owned())])?)?;
    let exact = search.results.iter().position(|row| {
        row.asset_instance_id
            .as_ref()
            .is_some_and(|id| id.display().eq_ignore_ascii_case(query.trim()))
    });
    let mut results = search.results;
    if let Some(index) = exact {
        return Ok(results.swap_remove(index));
    }
    if query.contains('@') {
        return Err(CliError::Usage(format!(
            "no catalog asset matches the exact ID \"{query}\""
        )));
    }
    let catalog: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, row)| row.asset_instance_id.is_some())
        .map(|(index, _)| index)
        .collect();
    match catalog.as_slice() {
        [index] => Ok(results.swap_remove(*index)),
        [] => Err(CliError::Usage(format!(
            "no catalog asset matches \"{query}\"; try `bitgarth asset search {query}`"
        ))),
        many => Err(CliError::Usage(format!(
            "\"{query}\" matches {} assets; pass an exact ID such as {} (see `bitgarth asset search {search_text}`)",
            many.len(),
            results[many[0]]
                .asset_instance_id
                .as_ref()
                .map(crate::model::AssetInstanceId::display)
                .unwrap_or_default()
        ))),
    }
}

pub(super) fn asset(context: &Context, command: AssetCommand) -> Result<(), CliError> {
    let AssetCommand::Search { query } = command;
    let query = query.join(" ");
    let session = context.session()?;
    let raw = session.get(&["assets", "search"], &[("query", query.clone())])?;
    if context.json() {
        return context.print_json(&raw);
    }
    let search: AssetSearch = decode(raw)?;
    if search.results.is_empty() {
        println!("No assets match \"{}\".", ui::terminal_text(&query));
        return Ok(());
    }
    let mut table = Table::new(&[
        ("ASSET", Align::Left),
        ("UNIT", Align::Left),
        ("NETWORK", Align::Left),
        ("ID", Align::Left),
    ]);
    for row in &search.results {
        let id = match (&row.asset_instance_id, &row.coingecko_id) {
            (Some(id), _) => Cell::styled(id.display(), Style::Cyan),
            (None, Some(coingecko)) => {
                Cell::styled(format!("coingecko:{coingecko} (web app only)"), Style::Dim)
            }
            (None, None) => Cell::new("—"),
        };
        table.row(vec![
            Cell::styled(row.asset_name.as_str(), Style::Bold),
            Cell::new(row.unit_code.as_str()),
            Cell::styled(row.network_name.as_str(), Style::Dim),
            id,
        ]);
    }
    print!("{}", table.render());
    if search.total_match_count > search.results.len() {
        ui::hint(&format!(
            "Showing {} of {} matches; refine your search.",
            search.results.len(),
            search.total_match_count
        ));
    }
    ui::hint("Add one with `bitgarth account add manual <ID> --new-wallet <NAME>`.");
    Ok(())
}

// ============ edit ============

fn rename(context: &Context, query: &str, name: &str) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let (_, account) = find_account(&wallets, query)?;
    session.send(
        Method::PATCH,
        &["accounts", account.id()],
        &json!({ "label": name }),
    )?;
    if context.json() {
        return context.print_json(&json!({ "id": account.id(), "label": name }));
    }
    ui::success(&format!("Renamed account {} to {}", account.label(), name));
    Ok(())
}

fn move_account(context: &Context, query: &str, destination: &Destination) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let (from, account) = find_account(&wallets, query)?;
    let (body, target_name) = match (&destination.to, &destination.new_wallet) {
        (Some(to), _) => {
            let wallet = find_wallet(&wallets, to)?;
            (
                json!({ "kind": "existing_wallet", "wallet_id": wallet.id }),
                wallet.label.clone(),
            )
        }
        (None, Some(label)) => (
            json!({ "kind": "new_wallet", "label": label }),
            label.clone(),
        ),
        (None, None) => {
            return Err(CliError::Usage(
                "pass --to <WALLET> or --new-wallet <NAME>".to_owned(),
            ));
        }
    };
    let response = session.send(
        Method::POST,
        &["accounts", account.id(), "move"],
        &json!({ "destination": body }),
    )?;
    if context.json() {
        return context.print_json(&response);
    }
    ui::success(&format!(
        "Moved {} from {} to {}",
        account.label(),
        from.label,
        target_name
    ));
    Ok(())
}

fn delete(context: &Context, query: &str, yes: bool) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let (wallet, account) = find_account(&wallets, query)?;
    confirm_destructive(
        &format!(
            "Delete account \"{}\" from wallet \"{}\", including its history?",
            account.label(),
            wallet.label
        ),
        yes,
    )?;
    session.delete(&["accounts", account.id()])?;
    if context.json() {
        return context.print_json(&json!({ "deleted": account.id() }));
    }
    ui::success(&format!("Deleted account {}", account.label()));
    Ok(())
}

fn addresses(context: &Context, query: &str, page: u32) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let (_, account) = find_account(&wallets, query)?;
    let Account::Native(native) = account else {
        return Err(CliError::Usage(
            "only Bitcoin and Ethereum accounts have addresses".to_owned(),
        ));
    };
    let raw = session.get(
        &["accounts", &native.native_account_id, "addresses"],
        &[
            ("scheme", native.scheme.clone()),
            ("page", page.to_string()),
        ],
    )?;
    if context.json() {
        return context.print_json(&raw);
    }
    let addresses: Addresses = decode(raw)?;
    let mut table = Table::new(&[
        ("PATH", Align::Left),
        ("ADDRESS", Align::Left),
        ("TXS", Align::Right),
        ("BALANCE", Align::Right),
    ]);
    for row in &addresses.rows {
        table.row(vec![
            Cell::styled(row.derivation_path.as_str(), Style::Dim),
            Cell::new(row.address.as_str()),
            Cell::new(row.transaction_count.to_string()),
            Cell::new(row.balance.balance_state.formatted().map_or_else(
                || "?".to_owned(),
                |value| ui::amount(value, &row.balance.unit_code),
            )),
        ]);
    }
    if table.is_empty() {
        println!("No addresses derived yet; they appear after the first sync.");
        return Ok(());
    }
    print!("{}", table.render());
    let end =
        (addresses.page.saturating_sub(1)) * addresses.page_size + addresses.rows.len() as u32;
    pagination(
        end.saturating_sub(addresses.rows.len() as u32) + 1,
        end,
        addresses.total,
        addresses.page,
        &format!("bitgarth account addresses {}", account.id()),
    );
    Ok(())
}

// ============ manual balances ============

fn manual_account<'a>(wallets: &'a Wallets, query: &str) -> Result<&'a Account, CliError> {
    let (_, account) = find_account(wallets, query)?;
    match account {
        Account::Manual(_) | Account::Custom(_) => Ok(account),
        Account::Native(_) => Err(CliError::Usage(format!(
            "\"{}\" is synced from the blockchain; only manual accounts take recorded balances",
            account.label()
        ))),
    }
}

fn set_balance(
    context: &Context,
    query: &str,
    amount: &str,
    date: Option<String>,
    note: Option<String>,
) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let account = manual_account(&wallets, query)?;
    let date = match date {
        Some(date) => parse_date(&date, "--date")?,
        None => chrono::Local::now().format("%Y-%m-%d").to_string(),
    };
    let body = json!({ "asserted_on": date, "balance": amount.trim(), "note": note });
    let response = match session.send(
        Method::POST,
        &["accounts", account.id(), "balance-assertions"],
        &body,
    ) {
        Err(error) if error.is_status(409) => {
            let existing = find_assertion(&session, account.id(), &date)?;
            session.send(
                Method::PUT,
                &["accounts", account.id(), "balance-assertions", &existing],
                &body,
            )?
        }
        other => other?,
    };
    if context.json() {
        return context.print_json(&json!({ "account_id": account.id(), "date": date, "balance": amount.trim(), "result": response }));
    }
    ui::success(&format!(
        "Recorded {} for {} on {date}",
        ui::amount(amount.trim(), account.summary().unit),
        account.label()
    ));
    Ok(())
}

fn unset_balance(context: &Context, query: &str, date: &str) -> Result<(), CliError> {
    let session = context.session()?;
    let (_, wallets) = load_wallets(&session)?;
    let account = manual_account(&wallets, query)?;
    let date = parse_date(date, "--date")?;
    let existing = find_assertion(&session, account.id(), &date)?;
    session.delete(&["accounts", account.id(), "balance-assertions", &existing])?;
    if context.json() {
        return context
            .print_json(&json!({ "account_id": account.id(), "date": date, "deleted": existing }));
    }
    ui::success(&format!(
        "Removed the {date} balance of {}",
        account.label()
    ));
    Ok(())
}

/// Assertion dates are unique per account.
fn find_assertion(session: &Session, account_id: &str, date: &str) -> Result<String, CliError> {
    let filters = json!({
        "from_date": day_boundary(date, "--date", false, true)?,
        "to_date": day_boundary(date, "--date", true, true)?,
    });
    let history: History = decode(session.get(
        &["accounts", account_id, "transactions"],
        &[("filters", filters.to_string())],
    )?)?;
    let History::Custom(history) = history else {
        return Err(CliError::Usage("not a manual account".to_owned()));
    };
    history
        .assertions
        .rows
        .into_iter()
        .find(|row| row.asserted_on == date)
        .map(|row| row.assertion_id)
        .ok_or_else(|| CliError::Usage(format!("no balance is recorded on {date}")))
}

#[cfg(test)]
mod tests {
    use super::is_extended_key;

    #[test]
    fn detects_extended_keys_but_not_addresses() {
        let xpub = format!("zpub{}", "a".repeat(107));
        assert!(is_extended_key(&xpub));
        assert!(!is_extended_key(
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"
        ));
        assert!(!is_extended_key("xpub-too-short"));
    }
}

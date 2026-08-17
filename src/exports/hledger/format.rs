use super::GENERATED_HEADER;
use crate::amounts::{UnsignedAmount, format_unsigned_amount_fixed};
use crate::transactions::AccountTransactionDirection;
use crate::wallets::WalletAccountId;
use chrono::{DateTime, NaiveDate, Utc};
use std::cmp::Ordering;

const UNKNOWN_EXPENSE_ACCOUNT: &str = "expenses:unknown";
const UNKNOWN_INCOME_ACCOUNT: &str = "income:unknown";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeTransactionRenderContext {
    pub(crate) asset_display_name: String,
    pub(crate) network_fee_account: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommodityDirective {
    pub(crate) unit_code: String,
    pub(crate) decimal_precision: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AggregatedAccountTransaction {
    pub(crate) account_id: WalletAccountId,
    pub(crate) tx_hash: String,
    pub(crate) direction: AccountTransactionDirection,
    pub(crate) balance_delta: i128,
    pub(crate) fee: UnsignedAmount,
    pub(crate) occurred_at: DateTime<Utc>,
    pub(crate) block_height: Option<i64>,
    pub(crate) nonce: Option<i64>,
    pub(crate) min_transfer_index: Option<i64>,
    pub(crate) closing_balance: Option<UnsignedAmount>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CustomBalanceAssertionRenderRow {
    pub(crate) assertion_id: String,
    pub(crate) asserted_on: NaiveDate,
    pub(crate) asserted_balance: UnsignedAmount,
    pub(crate) note: Option<String>,
    pub(crate) source: BalanceAssertionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BalanceAssertionSource {
    Manual,
    Api,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HledgerFormatError {
    AmountOverflow(&'static str),
    InvalidTransaction(String),
}

impl std::fmt::Display for HledgerFormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HledgerFormatError::AmountOverflow(field) => {
                write!(f, "amount overflow while aggregating {field}")
            }
            HledgerFormatError::InvalidTransaction(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for HledgerFormatError {}

pub(crate) fn needs_hledger_quoting(unit_code: &str) -> bool {
    unit_code
        .chars()
        .any(|character| character.is_ascii_digit())
}

pub(crate) fn format_hledger_commodity(unit_code: &str) -> String {
    if needs_hledger_quoting(unit_code) {
        format!("\"{unit_code}\"")
    } else {
        unit_code.to_string()
    }
}

pub(crate) fn format_directives_journal(directives: &[CommodityDirective]) -> String {
    let mut sorted = directives.to_vec();
    sorted.sort_by(|left, right| left.unit_code.cmp(&right.unit_code));
    sorted.dedup_by(|left, right| {
        left.unit_code == right.unit_code && left.decimal_precision == right.decimal_precision
    });

    let mut lines = vec![GENERATED_HEADER.to_string(), String::new()];

    for directive in sorted {
        let amount =
            format_unsigned_amount_fixed(UnsignedAmount::zero(), directive.decimal_precision);
        lines.push(format!(
            "commodity {amount} {}",
            format_hledger_commodity(&directive.unit_code)
        ));
    }

    lines.push(String::new());
    lines.join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostingSign {
    Positive,
    Negative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Posting {
    account: String,
    sign: PostingSign,
    amount: UnsignedAmount,
    closing_balance: Option<UnsignedAmount>,
}

pub(crate) fn build_hledger_transaction(
    account_name: &str,
    unit_code: &str,
    decimal_precision: u8,
    render_context: &NativeTransactionRenderContext,
    transaction: &AggregatedAccountTransaction,
) -> Result<String, HledgerFormatError> {
    if transaction.tx_hash.trim().is_empty() {
        return Err(HledgerFormatError::InvalidTransaction(
            "tx_hash cannot be empty".to_string(),
        ));
    }

    let closing_balance = transaction.closing_balance;
    let mut postings = Vec::new();

    match transaction.balance_delta.cmp(&0) {
        Ordering::Greater => {
            let incoming =
                unsigned_amount_from_positive_i128(transaction.balance_delta, "balance_delta")?;
            push_posting(
                &mut postings,
                account_name,
                PostingSign::Positive,
                incoming,
                closing_balance,
            );
            push_posting(
                &mut postings,
                UNKNOWN_INCOME_ACCOUNT,
                PostingSign::Negative,
                incoming,
                None,
            );
        }
        Ordering::Less => {
            let outflow = unsigned_amount_from_positive_i128(
                transaction
                    .balance_delta
                    .checked_abs()
                    .ok_or(HledgerFormatError::AmountOverflow("balance_delta_abs"))?,
                "balance_delta_abs",
            )?;
            let fee_leg = if transaction.fee.value() < outflow.value() {
                transaction.fee
            } else {
                outflow
            };
            let external = subtract_unsigned(outflow, fee_leg)?;

            push_posting(
                &mut postings,
                account_name,
                PostingSign::Negative,
                outflow,
                closing_balance,
            );
            if fee_leg.value() > 0 {
                push_posting(
                    &mut postings,
                    &render_context.network_fee_account,
                    PostingSign::Positive,
                    fee_leg,
                    None,
                );
            }
            if external.value() > 0 {
                push_posting(
                    &mut postings,
                    UNKNOWN_EXPENSE_ACCOUNT,
                    PostingSign::Positive,
                    external,
                    None,
                );
            }
        }
        Ordering::Equal => {}
    }

    let mut lines = Vec::new();
    push_transaction_header(&mut lines, transaction, render_context);
    for posting in postings {
        lines.push(format_posting(&posting, unit_code, decimal_precision));
    }

    Ok(lines.join("\n"))
}

fn unsigned_amount_from_positive_i128(
    value: i128,
    field: &'static str,
) -> Result<UnsignedAmount, HledgerFormatError> {
    let magnitude = u128::try_from(value).map_err(|_| HledgerFormatError::AmountOverflow(field))?;
    Ok(UnsignedAmount::from_u128(magnitude))
}

fn transaction_description(
    direction: AccountTransactionDirection,
    asset_display_name: &str,
) -> String {
    match direction {
        AccountTransactionDirection::Outgoing => format!("Sent {asset_display_name}"),
        AccountTransactionDirection::Incoming => format!("Received {asset_display_name}"),
        AccountTransactionDirection::SelfTransfer => {
            format!("Self-transfer {asset_display_name}")
        }
    }
}

fn push_transaction_header(
    lines: &mut Vec<String>,
    transaction: &AggregatedAccountTransaction,
    render_context: &NativeTransactionRenderContext,
) {
    lines.push(format!(
        "{} * {}",
        transaction.occurred_at.format("%Y-%m-%d"),
        transaction_description(transaction.direction, &render_context.asset_display_name)
    ));
    lines.push(format!("    ; Transaction {}", transaction.tx_hash));
}

pub(crate) fn build_custom_balance_assertion_transaction(
    account_name: &str,
    balance_assertions_equity_account_name: &str,
    unit_code: &str,
    decimal_precision: u8,
    assertion: &CustomBalanceAssertionRenderRow,
) -> String {
    let formatted_unit_code = format_hledger_commodity(unit_code);
    let description =
        build_balance_assertion_description(assertion.note.as_deref(), assertion.source);
    let asserted_balance =
        format_unsigned_amount_fixed(assertion.asserted_balance, decimal_precision);

    [
        format!(
            "{} * {description}",
            assertion.asserted_on.format("%Y-%m-%d")
        ),
        format!(
            "    {}    = {} {}",
            account_name, asserted_balance, formatted_unit_code
        ),
        format!("    {balance_assertions_equity_account_name}"),
    ]
    .join("\n")
}

fn subtract_unsigned(
    left: UnsignedAmount,
    right: UnsignedAmount,
) -> Result<UnsignedAmount, HledgerFormatError> {
    let value = left
        .value()
        .checked_sub(right.value())
        .ok_or(HledgerFormatError::AmountOverflow("unsigned_subtraction"))?;
    Ok(UnsignedAmount::from_u128(value))
}

fn push_posting(
    postings: &mut Vec<Posting>,
    account: &str,
    sign: PostingSign,
    amount: UnsignedAmount,
    closing_balance: Option<UnsignedAmount>,
) {
    if amount.value() == 0 {
        return;
    }
    postings.push(Posting {
        account: account.to_string(),
        sign,
        amount,
        closing_balance,
    });
}

fn format_posting(posting: &Posting, unit_code: &str, decimal_precision: u8) -> String {
    let formatted_unit_code = format_hledger_commodity(unit_code);
    let amount = format_unsigned_amount_fixed(posting.amount, decimal_precision);
    let signed_amount = match posting.sign {
        PostingSign::Positive => amount,
        PostingSign::Negative => format!("-{amount}"),
    };
    let mut line = format!(
        "    {}    {} {}",
        posting.account, signed_amount, formatted_unit_code
    );
    if let Some(closing_balance) = posting.closing_balance {
        let assertion = format_unsigned_amount_fixed(closing_balance, decimal_precision);
        line.push_str(&format!(" = {assertion} {formatted_unit_code}"));
    }
    line
}

fn build_balance_assertion_description(
    note: Option<&str>,
    source: BalanceAssertionSource,
) -> String {
    let prefix = match source {
        BalanceAssertionSource::Manual => "Balance Assertion",
        BalanceAssertionSource::Api => "API Balance Assertion",
    };
    match note.and_then(sanitize_hledger_description_fragment) {
        Some(note) => format!("{prefix}: {note}"),
        None => prefix.to_string(),
    }
}

fn sanitize_hledger_description_fragment(note: &str) -> Option<String> {
    let sanitized = note
        .chars()
        .map(|character| match character {
            '\r' | '\n' | '\t' => ' ',
            ';' => ',',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect::<String>();
    let collapsed = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::str::FromStr;

    fn fixed_account_id() -> WalletAccountId {
        WalletAccountId::from_str("01KGQYDBAH5B0JD0BSF2VX95FR").expect("valid ULID")
    }

    fn fixed_timestamp(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 3, 17, hour, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn bitcoin_render_context() -> NativeTransactionRenderContext {
        NativeTransactionRenderContext {
            asset_display_name: "Bitcoin".to_string(),
            network_fee_account: "expenses:Fees:Bitcoin:Network:Mainnet".to_string(),
        }
    }

    #[test]
    fn needs_hledger_quoting_matches_digit_presence() {
        assert!(!needs_hledger_quoting("ADA"));
        assert!(needs_hledger_quoting("SP500"));
        assert!(needs_hledger_quoting("ABC2"));
    }

    #[test]
    fn format_hledger_commodity_quotes_only_when_needed() {
        assert_eq!(format_hledger_commodity("ADA"), "ADA");
        assert_eq!(format_hledger_commodity("SP500"), "\"SP500\"");
    }

    #[test]
    fn format_directives_journal_outputs_expected_commodities() {
        let directives = vec![
            CommodityDirective {
                unit_code: "BTC".to_string(),
                decimal_precision: 8,
            },
            CommodityDirective {
                unit_code: "ETH".to_string(),
                decimal_precision: 18,
            },
        ];

        let rendered = format_directives_journal(&directives);
        let expected = "\
; Generated by https://bitgarth.app/

commodity 0.00000000 BTC
commodity 0.000000000000000000 ETH
";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn format_directives_journal_quotes_digit_containing_commodities() {
        let directives = vec![CommodityDirective {
            unit_code: "SP500".to_string(),
            decimal_precision: 8,
        }];

        let rendered = format_directives_journal(&directives);
        assert!(rendered.contains("commodity 0.00000000 \"SP500\""));
    }

    #[test]
    fn build_custom_balance_assertion_transaction_formats_balance_assignment() {
        let rendered = build_custom_balance_assertion_transaction(
            "assets:Me:MainWallet:ADAAccount1",
            "equity:Balance Assertions:Me:MainWallet:ADAAccount1",
            "ADA",
            8,
            &CustomBalanceAssertionRenderRow {
                assertion_id: "01KGQYDBAH5B0JD0BSF2VX95FR".to_string(),
                asserted_on: NaiveDate::from_ymd_opt(2026, 2, 24).expect("valid date"),
                asserted_balance: UnsignedAmount::from_u128(23_456_700_000),
                note: Some("  corrected;\nmanual snapshot ".to_string()),
                source: BalanceAssertionSource::Manual,
            },
        );

        let expected = "\
2026-02-24 * Balance Assertion: corrected, manual snapshot
    assets:Me:MainWallet:ADAAccount1    = 234.56700000 ADA
    equity:Balance Assertions:Me:MainWallet:ADAAccount1";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn build_custom_balance_assertion_transaction_preserves_repeated_balance_events() {
        let rendered = build_custom_balance_assertion_transaction(
            "assets:Me:MainWallet:ADAAccount1",
            "equity:Balance Assertions:Me:MainWallet:ADAAccount1",
            "ADA",
            8,
            &CustomBalanceAssertionRenderRow {
                assertion_id: "01KGQYDBAH5B0JD0BSF2VX95FS".to_string(),
                asserted_on: NaiveDate::from_ymd_opt(2026, 2, 25).expect("valid date"),
                asserted_balance: UnsignedAmount::from_u128(23_456_700_000),
                note: None,
                source: BalanceAssertionSource::Manual,
            },
        );

        let expected = "\
2026-02-25 * Balance Assertion
    assets:Me:MainWallet:ADAAccount1    = 234.56700000 ADA
    equity:Balance Assertions:Me:MainWallet:ADAAccount1";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn build_custom_balance_assertion_transaction_labels_api_source() {
        let rendered = build_custom_balance_assertion_transaction(
            "assets:Me:MainWallet:BTCAccount1",
            "equity:Balance Assertions:Me:MainWallet:BTCAccount1",
            "BTC",
            8,
            &CustomBalanceAssertionRenderRow {
                assertion_id: "api-balance:account:2026-02-25T00:00:00Z:1".to_string(),
                asserted_on: NaiveDate::from_ymd_opt(2026, 2, 25).expect("valid date"),
                asserted_balance: UnsignedAmount::from_u128(50_000_000),
                note: Some("provider balance sync".to_string()),
                source: BalanceAssertionSource::Api,
            },
        );

        assert!(rendered.starts_with("2026-02-25 * API Balance Assertion: provider balance sync"));
    }

    #[test]
    fn build_custom_balance_assertion_transaction_quotes_digit_containing_unit_codes() {
        let rendered = build_custom_balance_assertion_transaction(
            "assets:Me:MainWallet:SP500Account1",
            "equity:Balance Assertions:Me:MainWallet:SP500Account1",
            "SP500",
            8,
            &CustomBalanceAssertionRenderRow {
                assertion_id: "01KGQYDBAH5B0JD0BSF2VX95FT".to_string(),
                asserted_on: NaiveDate::from_ymd_opt(2026, 2, 26).expect("valid date"),
                asserted_balance: UnsignedAmount::from_u128(250_000_000),
                note: None,
                source: BalanceAssertionSource::Manual,
            },
        );

        assert!(rendered.contains("= 2.50000000 \"SP500\""));
    }

    #[test]
    fn build_hledger_transaction_quotes_digit_containing_unit_codes() {
        let tx = AggregatedAccountTransaction {
            account_id: fixed_account_id(),
            tx_hash: "0xfeedface".to_string(),
            direction: AccountTransactionDirection::Incoming,
            balance_delta: 10_000_000,
            fee: UnsignedAmount::zero(),
            occurred_at: fixed_timestamp(16),
            block_height: Some(123),
            nonce: Some(0),
            min_transfer_index: Some(0),
            closing_balance: Some(UnsignedAmount::from_u128(10_000_000)),
        };

        let rendered = build_hledger_transaction(
            "assets:MainWallet:SP500Account1",
            "SP500",
            8,
            &NativeTransactionRenderContext {
                asset_display_name: "SP500".to_string(),
                network_fee_account: "expenses:Fees:SP500:Network:Mainnet".to_string(),
            },
            &tx,
        )
        .expect("transaction should render");

        assert!(rendered.contains("0.10000000 \"SP500\" = 0.10000000 \"SP500\""));
        assert!(rendered.contains("-0.10000000 \"SP500\""));
    }

    #[test]
    fn build_hledger_transaction_formats_cospend_send_with_fee_clamped_to_outflow() {
        let tx = AggregatedAccountTransaction {
            account_id: fixed_account_id(),
            tx_hash: "0xcospend".to_string(),
            direction: AccountTransactionDirection::Outgoing,
            balance_delta: -888,
            fee: UnsignedAmount::from_u128(10_000),
            occurred_at: fixed_timestamp(12),
            block_height: Some(123),
            nonce: Some(0),
            min_transfer_index: Some(0),
            closing_balance: Some(UnsignedAmount::from_u128(14_352_846_507_848)),
        };

        let rendered = build_hledger_transaction(
            "assets:MainWallet:BitcoinAccount1",
            "BTC",
            8,
            &bitcoin_render_context(),
            &tx,
        )
        .expect("transaction should render");
        let expected = "\
2024-03-17 * Sent Bitcoin
    ; Transaction 0xcospend
    assets:MainWallet:BitcoinAccount1    -0.00000888 BTC = 143528.46507848 BTC
    expenses:Fees:Bitcoin:Network:Mainnet    0.00000888 BTC";
        assert_eq!(rendered, expected);
        assert!(!rendered.contains("expenses:unknown"));
    }

    #[test]
    fn build_hledger_transaction_formats_sole_owner_send_with_fee_and_external_outflow() {
        let tx = AggregatedAccountTransaction {
            account_id: fixed_account_id(),
            tx_hash: "0xsoleowner".to_string(),
            direction: AccountTransactionDirection::Outgoing,
            balance_delta: -5_001_000,
            fee: UnsignedAmount::from_u128(1_000),
            occurred_at: fixed_timestamp(12),
            block_height: Some(123),
            nonce: Some(0),
            min_transfer_index: Some(0),
            closing_balance: Some(UnsignedAmount::from_u128(9_999_000)),
        };

        let rendered = build_hledger_transaction(
            "assets:MainWallet:BitcoinAccount1",
            "BTC",
            8,
            &bitcoin_render_context(),
            &tx,
        )
        .expect("transaction should render");
        let expected = "\
2024-03-17 * Sent Bitcoin
    ; Transaction 0xsoleowner
    assets:MainWallet:BitcoinAccount1    -0.05001000 BTC = 0.09999000 BTC
    expenses:Fees:Bitcoin:Network:Mainnet    0.00001000 BTC
    expenses:unknown    0.05000000 BTC";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn build_hledger_transaction_formats_no_fee_send_without_zero_fee_posting() {
        let tx = AggregatedAccountTransaction {
            account_id: fixed_account_id(),
            tx_hash: "0xnofee".to_string(),
            direction: AccountTransactionDirection::Outgoing,
            balance_delta: -5_000_000,
            fee: UnsignedAmount::zero(),
            occurred_at: fixed_timestamp(12),
            block_height: Some(123),
            nonce: Some(0),
            min_transfer_index: Some(0),
            closing_balance: Some(UnsignedAmount::from_u128(10_000_000)),
        };

        let rendered = build_hledger_transaction(
            "assets:MainWallet:BitcoinAccount1",
            "BTC",
            8,
            &bitcoin_render_context(),
            &tx,
        )
        .expect("transaction should render");
        let expected = "\
2024-03-17 * Sent Bitcoin
    ; Transaction 0xnofee
    assets:MainWallet:BitcoinAccount1    -0.05000000 BTC = 0.10000000 BTC
    expenses:unknown    0.05000000 BTC";
        assert_eq!(rendered, expected);
        assert!(!rendered.contains("expenses:Fees:Bitcoin:Network:Mainnet"));
        assert!(!rendered.contains("0.00000000 BTC"));
    }

    #[test]
    fn build_hledger_transaction_formats_incoming() {
        let tx = AggregatedAccountTransaction {
            account_id: fixed_account_id(),
            tx_hash: "0xdef456".to_string(),
            direction: AccountTransactionDirection::Incoming,
            balance_delta: 600,
            fee: UnsignedAmount::zero(),
            occurred_at: fixed_timestamp(13),
            block_height: Some(123),
            nonce: Some(0),
            min_transfer_index: Some(0),
            closing_balance: Some(UnsignedAmount::from_u128(10_000_600)),
        };

        let rendered = build_hledger_transaction(
            "assets:MainWallet:BitcoinAccount1",
            "BTC",
            8,
            &bitcoin_render_context(),
            &tx,
        )
        .expect("transaction should render");
        let expected = "\
2024-03-17 * Received Bitcoin
    ; Transaction 0xdef456
    assets:MainWallet:BitcoinAccount1    0.00000600 BTC = 0.10000600 BTC
    income:unknown    -0.00000600 BTC";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn build_hledger_transaction_formats_zero_delta_as_header_only() {
        let tx = AggregatedAccountTransaction {
            account_id: fixed_account_id(),
            tx_hash: "zero".to_string(),
            direction: AccountTransactionDirection::SelfTransfer,
            balance_delta: 0,
            fee: UnsignedAmount::from_u128(1_000),
            occurred_at: fixed_timestamp(14),
            block_height: Some(123),
            nonce: Some(0),
            min_transfer_index: Some(0),
            closing_balance: Some(UnsignedAmount::from_u128(5_000_000)),
        };

        let rendered = build_hledger_transaction(
            "assets:MainWallet:BitcoinAccount1",
            "BTC",
            8,
            &bitcoin_render_context(),
            &tx,
        )
        .expect("transaction should render");
        let expected = "\
2024-03-17 * Self-transfer Bitcoin
    ; Transaction zero";
        assert_eq!(rendered, expected);
        assert!(rendered.contains("Self-transfer Bitcoin"));
        assert!(rendered.contains("; Transaction zero"));
        assert!(!rendered.contains("assets:"));
        assert!(!rendered.contains("expenses:"));
        assert!(!rendered.contains("income:"));
    }
}

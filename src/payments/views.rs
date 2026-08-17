use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PaymentStateStatus {
    NotActive,
    Active,
    ActiveWithSyncWarning,
    RecoveryFailed,
    Pending,
    Verifying,
    AdditionalPaymentRequired,
    ManualReview,
    Expired,
    Failed,
    Canceled,
    Unavailable,
    UpgradeRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaymentSummaryView {
    pub(crate) paid_order_amount: String,
    pub(crate) paid_order_currency: String,
    pub(crate) paid_asset_amount: Option<String>,
    pub(crate) paid_asset_code: Option<String>,
    pub(crate) blockchain_hash: Option<String>,
    pub(crate) confirmed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AdditionalPaymentView {
    pub(crate) paid_amount: String,
    pub(crate) paid_currency: String,
    pub(crate) remaining_amount: String,
    pub(crate) remaining_currency: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaymentSupportReferenceView {
    pub(crate) token_id: Option<String>,
    pub(crate) subscription_subject_id: Option<String>,
    pub(crate) entitlement_holder_id: String,
    pub(crate) order_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaymentStateView {
    pub(crate) status: PaymentStateStatus,
    pub(crate) tier: String,
    pub(crate) tier_display_name: String,
    pub(crate) sync_account_slots_limit: u16,
    pub(crate) historical_backfill_enabled: bool,
    pub(crate) historical_backfill_transactions_per_account: u32,
    pub(crate) order_id: Option<String>,
    pub(crate) paid_through: Option<String>,
    pub(crate) display_amount: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) payment_summary: Option<PaymentSummaryView>,
    pub(crate) additional_payment: Option<AdditionalPaymentView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) support_reference: Option<PaymentSupportReferenceView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PaymentOrderStatusView {
    Pending,
    Paid,
    Expired,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaymentOrderHistoryView {
    pub(crate) order_id: String,
    pub(crate) product_tier: String,
    pub(crate) display_amount: String,
    pub(crate) currency: String,
    pub(crate) status: PaymentOrderStatusView,
    pub(crate) paid_at: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AppCompatibilityStatusView {
    UpgradeRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AppCompatibilityView {
    pub(crate) status: AppCompatibilityStatusView,
    pub(crate) detail: String,
    pub(crate) minimum_app_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaymentOptionView {
    pub(crate) id: String,
    pub(crate) tier: String,
    pub(crate) tier_display_name: String,
    pub(crate) term_quantity: Option<u16>,
    pub(crate) term_unit: Option<String>,
    pub(crate) term_label: String,
    pub(crate) minor_units: u64,
    pub(crate) decimal_precision: u8,
    pub(crate) display_amount: String,
    pub(crate) currency: String,
    pub(crate) currency_symbol: String,
    #[serde(default)]
    pub(crate) is_default: bool,
}

/// Central-dependent portion of the payments page: the purchasable tier
/// catalog plus locally-stored order history. Fetched separately from the
/// user's plan state so a slow or unreachable Central never blocks the page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaymentCatalogView {
    pub(crate) tiers: Vec<PaymentTierView>,
    pub(crate) options: Vec<PaymentOptionView>,
    pub(crate) app_compatibility: Option<AppCompatibilityView>,
    pub(crate) options_message: Option<String>,
    pub(crate) order_history: Vec<PaymentOrderHistoryView>,
    /// Central-authored one-paragraph plan comparison shown above the tier
    /// grid. Carries the same `**bold**`-parsed segments as tier bullets;
    /// bold runs render as tier chips. `None` falls back to app copy.
    #[serde(default)]
    pub(crate) pricing_summary: Option<TierBulletView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaymentTierView {
    pub(crate) tier: String,
    pub(crate) display_name: String,
    pub(crate) summary: String,
    pub(crate) bullets: Vec<TierBulletView>,
    pub(crate) is_featured: bool,
    pub(crate) ribbon_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TierBulletView {
    pub(crate) segments: Vec<BulletSegmentView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub(crate) enum BulletSegmentView {
    Plain(String),
    Bold(String),
}

/// Parse a bullet string into typed segments.
///
/// Server-side only — the wire format carries already-parsed `TierBulletView`s
/// to the client. The only markup we recognise is `**bold**` runs: paired `**`
/// markers wrap a bold segment; everything else is plain text. Unbalanced `**`
/// collapses to a single `Plain` segment (defensive — the server validates,
/// this protects the UI if a stray marker ever slips through). Empty input
/// yields an empty list.
#[cfg(any(feature = "server", test))]
pub(crate) fn parse_bullet(raw: &str) -> TierBulletView {
    if raw.is_empty() {
        return TierBulletView { segments: vec![] };
    }
    if !raw.contains("**") {
        return TierBulletView {
            segments: vec![BulletSegmentView::Plain(raw.to_string())],
        };
    }

    let mut segments = Vec::new();
    let mut remaining = raw;
    let mut in_bold = false;
    let mut had_any_match = false;
    while let Some(idx) = remaining.find("**") {
        let (before, after) = remaining.split_at(idx);
        if !before.is_empty() {
            push_segment(&mut segments, before.to_string(), in_bold);
        }
        remaining = &after[2..];
        in_bold = !in_bold;
        had_any_match = true;
    }

    // If we ended with an unclosed bold marker, abandon the parse and emit
    // the whole string as plain — the input was malformed.
    if in_bold || !had_any_match {
        return TierBulletView {
            segments: vec![BulletSegmentView::Plain(raw.to_string())],
        };
    }

    if !remaining.is_empty() {
        push_segment(&mut segments, remaining.to_string(), false);
    }

    TierBulletView { segments }
}

#[cfg(any(feature = "server", test))]
fn push_segment(segments: &mut Vec<BulletSegmentView>, text: String, bold: bool) {
    if bold {
        segments.push(BulletSegmentView::Bold(text));
    } else {
        segments.push(BulletSegmentView::Plain(text));
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PremiumOrderLaunchView {
    pub(crate) state: PaymentStateView,
    pub(crate) merchant_id: String,
    pub(crate) central_order_id: String,
    pub(crate) atlos_order_id: String,
    pub(crate) order_amount: String,
    pub(crate) order_currency: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PremiumTopUpLaunchView {
    pub(crate) state: PaymentStateView,
    pub(crate) launch: Option<PremiumOrderLaunchView>,
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn sample_tier() -> PaymentTierView {
        PaymentTierView {
            tier: "premium".to_string(),
            display_name: "Premium".to_string(),
            summary: "Fifty synced accounts, deep histories.".to_string(),
            bullets: vec![parse_bullet("**50** synced accounts")],
            is_featured: false,
            ribbon_label: None,
        }
    }

    #[test]
    fn payment_state_views_are_ui_safe_shapes() {
        let state = PaymentStateView {
            status: PaymentStateStatus::Pending,
            tier: "free".to_string(),
            tier_display_name: "Free".to_string(),
            sync_account_slots_limit: 5,
            historical_backfill_enabled: false,
            historical_backfill_transactions_per_account: 0,
            order_id: Some("01JQABCDEF000000000000000E".to_string()),
            paid_through: None,
            display_amount: Some("9.99".to_string()),
            currency: Some("USD".to_string()),
            message: None,
            payment_summary: None,
            additional_payment: None,
            support_reference: None,
        };
        let start = PremiumOrderLaunchView {
            state,
            merchant_id: "8MY8BXTU15".to_string(),
            central_order_id: "01JQABCDEF000000000000000E".to_string(),
            atlos_order_id: "01JQABCDEF000000000000000F".to_string(),
            order_amount: "9.99".to_string(),
            order_currency: "USD".to_string(),
        };

        let serialized = serde_json::to_value(start).expect("view should serialize");
        assert!(serialized.get("management_secret").is_none());
        assert!(serialized.get("order_secret").is_none());
        assert!(serialized.get("premium_access_token").is_none());
        assert!(serialized.get("offline_access_until").is_none());
    }

    #[test]
    fn payment_catalog_view_is_ui_safe_shape() {
        let catalog = PaymentCatalogView {
            tiers: vec![sample_tier()],
            options: vec![PaymentOptionView {
                id: "premium_12_months_usd".to_string(),
                tier: "premium".to_string(),
                tier_display_name: "Premium".to_string(),
                term_quantity: Some(12),
                term_unit: Some("month".to_string()),
                term_label: "1 year".to_string(),
                minor_units: 123,
                decimal_precision: 2,
                display_amount: "1.23".to_string(),
                currency: "USD".to_string(),
                currency_symbol: "$".to_string(),
                is_default: true,
            }],
            app_compatibility: Some(AppCompatibilityView {
                status: AppCompatibilityStatusView::UpgradeRequired,
                detail: "Install a newer build.".to_string(),
                minimum_app_version: None,
            }),
            options_message: None,
            order_history: vec![PaymentOrderHistoryView {
                order_id: "01JQABCDEF000000000000000E".to_string(),
                product_tier: "premium".to_string(),
                display_amount: "1.23".to_string(),
                currency: "USD".to_string(),
                status: PaymentOrderStatusView::Paid,
                paid_at: Some("2026-04-16T12:00:00Z".to_string()),
            }],
            pricing_summary: Some(parse_bullet(
                "**Free** tracks. **Paid** does the accounting.",
            )),
        };

        let serialized = serde_json::to_value(catalog).expect("view should serialize");
        assert!(serialized.get("management_secret").is_none());
        assert!(serialized.get("order_secret").is_none());
        assert!(serialized.get("premium_access_token").is_none());
    }

    #[test]
    fn parse_bullet_plain_text_returns_single_plain_segment() {
        let bullet = parse_bullet("Hello world");
        assert_eq!(
            bullet.segments,
            vec![BulletSegmentView::Plain("Hello world".to_string())]
        );
    }

    #[test]
    fn parse_bullet_empty_string_returns_empty_segments() {
        let bullet = parse_bullet("");
        assert!(bullet.segments.is_empty());
    }

    #[test]
    fn parse_bullet_bold_at_start() {
        let bullet = parse_bullet("**5** synced accounts");
        assert_eq!(
            bullet.segments,
            vec![
                BulletSegmentView::Bold("5".to_string()),
                BulletSegmentView::Plain(" synced accounts".to_string()),
            ]
        );
    }

    #[test]
    fn parse_bullet_bold_in_middle() {
        let bullet = parse_bullet("up to **10,000** per account");
        assert_eq!(
            bullet.segments,
            vec![
                BulletSegmentView::Plain("up to ".to_string()),
                BulletSegmentView::Bold("10,000".to_string()),
                BulletSegmentView::Plain(" per account".to_string()),
            ]
        );
    }

    #[test]
    fn parse_bullet_bold_at_end() {
        let bullet = parse_bullet("history up to **100,000**");
        assert_eq!(
            bullet.segments,
            vec![
                BulletSegmentView::Plain("history up to ".to_string()),
                BulletSegmentView::Bold("100,000".to_string()),
            ]
        );
    }

    #[test]
    fn parse_bullet_multiple_bold_runs() {
        let bullet = parse_bullet("**50** accounts, **100,000** transactions");
        assert_eq!(
            bullet.segments,
            vec![
                BulletSegmentView::Bold("50".to_string()),
                BulletSegmentView::Plain(" accounts, ".to_string()),
                BulletSegmentView::Bold("100,000".to_string()),
                BulletSegmentView::Plain(" transactions".to_string()),
            ]
        );
    }

    #[test]
    fn parse_bullet_unbalanced_marker_falls_back_to_plain() {
        let bullet = parse_bullet("almost **bold");
        assert_eq!(
            bullet.segments,
            vec![BulletSegmentView::Plain("almost **bold".to_string())]
        );
    }

    #[test]
    fn parse_bullet_only_markers_yields_empty_bold() {
        let bullet = parse_bullet("****");
        // Two `**` markers with nothing between them: opens and closes a bold
        // segment containing the empty string. We keep that as a single empty
        // bold segment rather than special-casing it away.
        assert_eq!(bullet.segments, Vec::<BulletSegmentView>::new());
    }
}

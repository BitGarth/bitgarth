use crate::backend::{AccountView, WalletView};
use crate::settings::SettingsState;
#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
use crate::trezor;
#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
use crate::wallets::AccountIndex;
#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
use crate::wallets::suggest_next_accounts;
use crate::wallets::{
    AddressScheme, DEFAULT_ACCOUNT_ADDRESSES_PAGE_SIZE, GetAccountAddressesRequest, SyncedAssetId,
    WALLET_LABEL_MAX_LENGTH, WalletId, validate_extended_pubkey_format,
};
use crate::{AuthState, AuthStatus, BannerMessage, BannerState};
use chrono::{DateTime, Utc};
use dioxus::prelude::*;

pub(super) fn format_sync_relative_time(now: DateTime<Utc>, timestamp: DateTime<Utc>) -> String {
    let elapsed = now.signed_duration_since(timestamp);
    if elapsed.num_seconds() < 60 {
        return "just now".to_string();
    }
    if elapsed.num_minutes() < 60 {
        return format!("{}m ago", elapsed.num_minutes());
    }
    if elapsed.num_hours() < 24 {
        return format!("{}h ago", elapsed.num_hours());
    }
    if elapsed.num_days() < 7 {
        return format!("{}d ago", elapsed.num_days());
    }
    timestamp.format("%Y-%m-%d").to_string()
}

pub(super) fn sync_status_error_message(
    error: Option<&crate::transactions::SyncErrorMessage>,
) -> String {
    let fallback = "Unknown sync error";
    let raw = error.map(|value| value.as_str()).unwrap_or(fallback).trim();
    if raw.chars().count() <= 120 {
        return raw.to_string();
    }
    let truncated: String = raw.chars().take(117).collect();
    format!("{truncated}...")
}

pub(super) fn format_sync_absolute_time(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M UTC").to_string()
}

pub(super) fn sync_result_word(
    result: Option<crate::transactions::AccountSyncResult>,
) -> &'static str {
    match result {
        Some(crate::transactions::AccountSyncResult::Success) => "success",
        Some(crate::transactions::AccountSyncResult::Partial) => "partial",
        Some(crate::transactions::AccountSyncResult::Failure) => "failed",
        Some(crate::transactions::AccountSyncResult::InProgress) => "running",
        None => "—",
    }
}

pub(super) fn prevalidate_bitcoin_address_input(input: &str) -> Result<(), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Bitcoin address is required.".to_string());
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("Bitcoin address cannot contain spaces.".to_string());
    }
    if trimmed.len() < 26 {
        return Err("Bitcoin address looks too short.".to_string());
    }
    if !(trimmed.starts_with("bc1") || trimmed.starts_with('1') || trimmed.starts_with('3')) {
        return Err("Bitcoin mainnet addresses usually start with bc1, 1, or 3.".to_string());
    }
    Ok(())
}

pub(super) fn prevalidate_ethereum_address_input(input: &str) -> Result<(), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Ethereum address is required.".to_string());
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("Ethereum address cannot contain spaces.".to_string());
    }
    if !(trimmed.starts_with("0x") || trimmed.starts_with("0X")) {
        return Err("Ethereum address must start with 0x.".to_string());
    }
    if trimmed.len() != 42 {
        return Err("Ethereum address must be 42 characters (0x + 40 hex).".to_string());
    }
    if !trimmed[2..].chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("Ethereum address contains invalid hex characters.".to_string());
    }
    Ok(())
}

pub(super) fn prevalidate_wallet_label_for_new_wallet(label_input: &str) -> Result<(), String> {
    let trimmed = label_input.trim();
    if trimmed.is_empty() {
        return Err("Wallet label is required.".to_string());
    }

    crate::wallets::Label::parse_with_limit(trimmed, WALLET_LABEL_MAX_LENGTH)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

pub(super) fn prevalidate_xpub_input(input: &str) -> Result<(), String> {
    validate_extended_pubkey_format(input)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

pub(super) fn handle_session_expired(
    mut auth_state: AuthState,
    mut banner_state: BannerState,
    context: &'static str,
) {
    let user_id = {
        let auth_snapshot = auth_state.read();
        match &*auth_snapshot {
            AuthStatus::Authenticated(auth) => Some(auth.user.user_id),
            _ => None,
        }
    };
    dioxus::logger::tracing::debug!(
        user_id = ?user_id,
        context,
        "wallets ui: session expired"
    );
    auth_state.set(AuthStatus::Unauthenticated);
    if user_id.is_some() {
        banner_state.set(Some(BannerMessage::SessionExpired));
    }
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn trezor_error_text(kind: trezor::TrezorErrorKind) -> (String, String) {
    match kind {
        trezor::TrezorErrorKind::BridgeNotRunning => (
            "Trezor Bridge needs to be running in order to link accounts from your Trezor.".to_string(),
            "Trezor Bridge is usually started in the background when you run the [Trezor Suite](https://trezor.io/trezor-suite) application. Please ensure that Trezor Suite can detect your device before linking it here.".to_string(),
        ),
        trezor::TrezorErrorKind::NoDevices => (
            "No Trezor devices found.".to_string(),
            "Please connect your Trezor and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::DeviceDisconnected => (
            "Trezor device disconnected.".to_string(),
            "Please reconnect your Trezor and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::UserCancelled => (
            "Operation cancelled.".to_string(),
            "You cancelled the operation on your device. Try again if needed.".to_string(),
        ),
        trezor::TrezorErrorKind::PinRequired => (
            "PIN entry required on device.".to_string(),
            "Please enter your PIN on the Trezor device.".to_string(),
        ),
        trezor::TrezorErrorKind::PassphraseRequired => (
            "Passphrase entry required.".to_string(),
            "Please enter your passphrase on the Trezor device or in Trezor Suite.".to_string(),
        ),
        trezor::TrezorErrorKind::ProtocolError => (
            "Protocol error.".to_string(),
            "Please reconnect your Trezor and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::InternalError => (
            "Internal error.".to_string(),
            "Please try again. If the problem persists, restart the application.".to_string(),
        ),
        trezor::TrezorErrorKind::SessionExpired => (
            "Device session expired.".to_string(),
            "Please try again.".to_string(),
        ),
        trezor::TrezorErrorKind::SessionConflict => (
            "Device session conflict.".to_string(),
            "Close Trezor Suite and any other applications using the Trezor, then try again.".to_string(),
        ),
        trezor::TrezorErrorKind::BridgeRejected => (
            "Bridge rejected the request.".to_string(),
            "The Trezor Bridge may be blocking this application. Please check your Trezor connection and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::BridgeError => (
            "Bridge error.".to_string(),
            "Please check your Trezor connection and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::DeviceError => (
            "Device reported an error.".to_string(),
            "Please check your Trezor device and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::MissingFingerprint => (
            "Missing fingerprint.".to_string(),
            "Please reconnect the device and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::MissingMasterFingerprint => (
            "Missing master fingerprint.".to_string(),
            "Please reconnect the device and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::InvalidFingerprint => (
            "Invalid fingerprint.".to_string(),
            "Please reconnect the device and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::NoAccountsSelected => (
            "No accounts selected.".to_string(),
            "Please select at least one account.".to_string(),
        ),
        trezor::TrezorErrorKind::MissingZpubData => (
            "Missing zpub data.".to_string(),
            "Please retry the linking process.".to_string(),
        ),
        trezor::TrezorErrorKind::WrongDeviceConnected => (
            "Connected wallet does not match the selected wallet.".to_string(),
            "Please connect the correct Trezor device and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::ConnectInitParseFailed => (
            "Failed to parse Trezor Connect init response.".to_string(),
            "Please refresh and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::ConnectInitFailed => (
            "Failed to initialize Trezor Connect.".to_string(),
            "Failed to initialize Trezor Connect.".to_string(),
        ),
        trezor::TrezorErrorKind::ConnectFingerprintParseFailed => (
            "Failed to parse fingerprint response.".to_string(),
            "Please refresh and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::ConnectFingerprintFailed => (
            "Failed to get master fingerprint.".to_string(),
            "Please try again.".to_string(),
        ),
        trezor::TrezorErrorKind::ConnectAccountIndexesSerializeFailed => (
            "Failed to serialize account indexes.".to_string(),
            "Please refresh and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::ConnectZpubParseFailed => (
            "Failed to parse zpub results.".to_string(),
            "Please refresh and try again.".to_string(),
        ),
        trezor::TrezorErrorKind::ConnectZpubFailed => (
            "Failed to fetch zpub.".to_string(),
            "Please try again.".to_string(),
        ),
    }
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
#[derive(Clone, Copy)]
pub(super) struct AddressSchemeChoice {
    pub(super) label: &'static str,
    pub(super) note: &'static str,
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) const ADDRESS_SCHEME_CHOICES: [AddressSchemeChoice; 3] = [
    AddressSchemeChoice {
        label: "Legacy",
        note: "Extended key starts with xpub, addresses usually start with 1",
    },
    AddressSchemeChoice {
        label: "SegWit Compatible",
        note: "Extended key starts with ypub, addresses usually start with 3",
    },
    AddressSchemeChoice {
        label: "Native SegWit",
        note: "Extended key starts with zpub, addresses usually start with bc1q",
    },
];

pub(crate) fn address_scheme_label(address_scheme: AddressScheme) -> &'static str {
    match address_scheme {
        AddressScheme::Legacy => "Legacy",
        AddressScheme::NestedSegwit => "SegWit Compatible",
        AddressScheme::NativeSegwit => "Native SegWit",
        AddressScheme::Taproot => "Taproot",
        AddressScheme::Standard => "Standard",
    }
}

/// Shorten an xpub / address / hash for one-line display. Values that are
/// not longer than prefix + ellipsis + suffix are returned unchanged.
pub(crate) fn truncate_reference(value: &str) -> String {
    truncate_reference_with_lengths(value, 10, 6)
}

pub(crate) fn truncate_reference_with_lengths(value: &str, prefix: usize, suffix: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= prefix + suffix + 1 {
        return value.to_string();
    }
    let prefix: String = value.chars().take(prefix).collect();
    let suffix: String = value.chars().skip(char_count - suffix).collect();
    format!("{prefix}\u{2026}{suffix}")
}

/// One-line account identity for the /wallets row (design system §9.15):
/// scheme + truncated reference for Bitcoin, truncated address for
/// Ethereum. `None` means the row has nothing to show.
pub(super) fn account_row_subline(
    asset: SyncedAssetId,
    address_scheme: AddressScheme,
    account_reference: &str,
) -> Option<String> {
    let scheme_label = address_scheme_label(address_scheme);
    let reference = truncate_reference(account_reference);
    match asset {
        SyncedAssetId::Ethereum => (!reference.is_empty()).then_some(reference),
        SyncedAssetId::Bitcoin => Some(if reference.is_empty() {
            scheme_label.to_string()
        } else {
            format!("{scheme_label} \u{00B7} {reference}")
        }),
    }
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn address_scheme_sort_key(address_scheme: AddressScheme) -> u8 {
    match address_scheme {
        AddressScheme::Legacy => 0,
        AddressScheme::NestedSegwit => 1,
        AddressScheme::NativeSegwit => 2,
        AddressScheme::Taproot => 3,
        AddressScheme::Standard => 4,
    }
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn display_account_number(account: AccountIndex) -> u32 {
    account.as_u32() + 1
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn parse_display_account_number(input: u32) -> Result<AccountIndex, String> {
    if input == 0 {
        return Err("Account number must be 1 or greater.".to_string());
    }
    AccountIndex::new(input - 1).map_err(|err| err.to_string())
}

pub(super) fn supported_address_schemes() -> [AddressScheme; 3] {
    [
        AddressScheme::Legacy,
        AddressScheme::NestedSegwit,
        AddressScheme::NativeSegwit,
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct XpubDefaultSchemeInput {
    pub(super) address_scheme: AddressScheme,
    pub(super) has_activity: Option<bool>,
    pub(super) already_linked: bool,
}

pub(super) fn select_default_xpub_scheme(
    suggested_scheme: AddressScheme,
    schemes: &[XpubDefaultSchemeInput],
) -> Option<AddressScheme> {
    let find_scheme = |address_scheme| {
        schemes
            .iter()
            .find(|scheme| scheme.address_scheme == address_scheme)
    };

    for scheme in supported_address_schemes() {
        if let Some(result) = find_scheme(scheme)
            && result.has_activity == Some(true)
            && !result.already_linked
        {
            return Some(scheme);
        }
    }

    if let Some(suggested) = find_scheme(suggested_scheme)
        && !suggested.already_linked
    {
        return Some(suggested_scheme);
    }

    for scheme in supported_address_schemes() {
        if let Some(result) = find_scheme(scheme)
            && !result.already_linked
        {
            return Some(scheme);
        }
    }

    None
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn default_address_scheme_for_new_account_selection() -> AddressScheme {
    AddressScheme::NativeSegwit
}

/// Parse a label string into a `Label`, with a static fallback.
/// The label string is expected to already be valid (from the DB/backend),
/// so the fallback should never be reached in practice.
#[allow(clippy::expect_used)]
pub(crate) fn parse_label_for_editor(
    value: &str,
    max_len: usize,
    fallback: &str,
) -> crate::wallets::Label {
    crate::wallets::Label::parse_with_limit(value, max_len).unwrap_or_else(|_| {
        crate::wallets::Label::parse_with_limit(fallback, max_len)
            .expect("static fallback label must parse")
    })
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
#[derive(Clone, PartialEq)]
pub(super) struct AccountSelection {
    pub(super) account: AccountIndex,
    pub(super) address_scheme: AddressScheme,
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
#[derive(Clone, PartialEq)]
pub(super) struct ExistingAccountAddressTypes {
    pub(super) account: AccountIndex,
    pub(super) linked_schemes: Vec<AddressScheme>,
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn collect_existing_account_address_types(
    wallet: &WalletView,
) -> Vec<ExistingAccountAddressTypes> {
    let mut seen = std::collections::BTreeMap::<u32, Vec<AddressScheme>>::new();
    for account in &wallet.accounts {
        if let AccountView::Native(account) = account
            && account.account_number > 0
        {
            seen.entry(account.account_number)
                .or_default()
                .push(account.scheme);
        }
    }
    let mut rows: Vec<ExistingAccountAddressTypes> = seen
        .into_iter()
        .filter_map(|(account_number, linked_schemes)| {
            Some(ExistingAccountAddressTypes {
                account: AccountIndex::new(account_number - 1).ok()?,
                linked_schemes,
            })
        })
        .collect();
    rows.sort_by_key(|row| row.account.as_u32());
    rows
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn linked_schemes_for_account(
    existing_account_address_types: &[ExistingAccountAddressTypes],
    account: AccountIndex,
) -> Vec<AddressScheme> {
    existing_account_address_types
        .iter()
        .find(|row| row.account.as_u32() == account.as_u32())
        .map(|row| row.linked_schemes.clone())
        .unwrap_or_default()
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn available_schemes_for_account(
    existing_account_address_types: &[ExistingAccountAddressTypes],
    account: AccountIndex,
) -> Vec<AddressScheme> {
    let linked = linked_schemes_for_account(existing_account_address_types, account);
    missing_supported_address_schemes(&linked)
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn default_selection_for_available_schemes(
    available_schemes: &[AddressScheme],
) -> Option<AddressScheme> {
    if available_schemes.is_empty() {
        return None;
    }
    if available_schemes.contains(&AddressScheme::NativeSegwit) {
        return Some(AddressScheme::NativeSegwit);
    }
    Some(available_schemes[0])
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn available_schemes_for_account_with_selected(
    existing_account_address_types: &[ExistingAccountAddressTypes],
    selected: &[AccountSelection],
    account: AccountIndex,
) -> Vec<AddressScheme> {
    let mut linked_schemes = linked_schemes_for_account(existing_account_address_types, account);
    for row in selected
        .iter()
        .filter(|row| row.account.as_u32() == account.as_u32())
    {
        if !linked_schemes.contains(&row.address_scheme) {
            linked_schemes.push(row.address_scheme);
        }
    }
    missing_supported_address_schemes(&linked_schemes)
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn account_selection_for_account(
    account: AccountIndex,
    existing_account_address_types: &[ExistingAccountAddressTypes],
) -> Option<AccountSelection> {
    let available_schemes = available_schemes_for_account(existing_account_address_types, account);
    default_selection_for_available_schemes(&available_schemes).map(|address_scheme| {
        AccountSelection {
            account,
            address_scheme,
        }
    })
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn suggest_initial_account_selections(
    existing_accounts: &[AccountIndex],
    existing_account_address_types: &[ExistingAccountAddressTypes],
) -> Vec<AccountSelection> {
    for row in existing_account_address_types {
        if let Some(selection) =
            account_selection_for_account(row.account, existing_account_address_types)
        {
            return vec![selection];
        }
    }

    let suggested = suggest_next_accounts(existing_accounts, 1);
    suggested
        .into_iter()
        .map(|account| AccountSelection {
            account,
            address_scheme: default_address_scheme_for_new_account_selection(),
        })
        .collect()
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn selected_scheme_summary(selections: &[AccountSelection]) -> String {
    let mut labels = Vec::new();
    for scheme in supported_address_schemes() {
        if selections
            .iter()
            .any(|selection| selection.address_scheme == scheme)
        {
            labels.push(address_scheme_label(scheme));
        }
    }
    if labels.is_empty() {
        "No address types selected.".to_string()
    } else {
        format!("Address types: {}", labels.join(", "))
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct WalletMoveOption {
    pub(crate) wallet_id: WalletId,
    pub(crate) label: String,
    pub(crate) logical_account_count: u32,
}

#[derive(Clone, PartialEq)]
pub(super) struct AccountSchemeRow {
    pub(super) scheme_view: AccountView,
    pub(super) show_change_wallet_action: bool,
}

pub(super) fn build_wallet_account_scheme_views(wallet: &WalletView) -> Vec<AccountView> {
    wallet.accounts.clone()
}

pub(crate) fn build_wallet_move_options(wallets: &[WalletView]) -> Vec<WalletMoveOption> {
    wallets
        .iter()
        .map(|wallet| WalletMoveOption {
            wallet_id: wallet.id,
            label: wallet.label.clone(),
            logical_account_count: wallet.logical_account_count,
        })
        .collect()
}

pub(super) fn build_account_scheme_rows(wallet: &WalletView) -> Vec<AccountSchemeRow> {
    let mut seen_account_ids = std::collections::BTreeSet::<String>::new();
    let mut rows = Vec::new();
    for scheme_view in build_wallet_account_scheme_views(wallet) {
        let account_id = match &scheme_view {
            AccountView::Native(view) => view.account_id.to_string(),
            AccountView::Custom(view) => view.account_id.to_string(),
            AccountView::Manual(view) => view.account_id.to_string(),
        };
        let is_first_for_account = seen_account_ids.insert(account_id);
        rows.push(AccountSchemeRow {
            scheme_view,
            show_change_wallet_action: is_first_for_account,
        });
    }
    rows
}

pub(super) fn logical_account_count_label(logical_account_count: u32) -> String {
    if logical_account_count == 1 {
        "1 account".to_string()
    } else {
        format!("{logical_account_count} accounts")
    }
}

pub(super) fn move_wallet_error_message(error: &crate::backend::WalletError) -> String {
    if crate::components::form_helpers::is_form_field_error(error) {
        return crate::components::form_helpers::primary_field_or_message(
            error,
            &["destination.label", "destination"],
        );
    }

    error.to_string()
}

#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(super) fn missing_supported_address_schemes(
    linked_schemes: &[AddressScheme],
) -> Vec<AddressScheme> {
    supported_address_schemes()
        .into_iter()
        .filter(|scheme| !linked_schemes.contains(scheme))
        .collect()
}

pub(crate) fn copy_to_clipboard(value: &str) {
    use dioxus::document::eval;
    let encoded_value = match serde_json::to_string(value) {
        Ok(encoded) => encoded,
        Err(err) => {
            dioxus::logger::tracing::warn!(error = %err, "wallets ui: failed to encode copy text");
            return;
        }
    };

    let script = format!(
        "const text = {encoded_value};
if (navigator && navigator.clipboard && navigator.clipboard.writeText) {{
  navigator.clipboard.writeText(text);
}} else {{
  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.setAttribute('readonly', '');
  textarea.style.position = 'fixed';
  textarea.style.opacity = '0';
  document.body.appendChild(textarea);
  textarea.select();
  try {{ document.execCommand('copy'); }} catch (e) {{}}
  document.body.removeChild(textarea);
}}"
    );
    let _ = eval(script.as_str());
}

pub(super) fn address_explorer_url(
    settings_state: &SettingsState,
    target: crate::explorer_links::DigitalAssetAddressRef<'_>,
) -> Result<String, String> {
    crate::explorer_links::explorer_url(
        settings_state,
        crate::explorer_links::ExplorerTarget::Address(target),
    )
    .map_err(|err| format!("Address explorer unavailable: {err}"))
}

pub(super) fn addresses_total_pages(total: u32, page_size: u32) -> u32 {
    if page_size == 0 {
        return 1;
    }

    let total_pages_u64 =
        (u64::from(total) + u64::from(page_size).saturating_sub(1)) / u64::from(page_size);
    let total_pages_u64 = total_pages_u64.max(1);

    u32::try_from(total_pages_u64).unwrap_or(u32::MAX)
}

#[derive(Clone, Copy)]
pub(crate) struct AccountAddressesLoader {
    pub(crate) account_id: crate::wallets::DigitalAssetAccountId,
    pub(crate) address_scheme: AddressScheme,
    pub(crate) auth_state: AuthState,
    pub(crate) banner_state: BannerState,
    pub(crate) addresses_loading: Signal<bool>,
    pub(crate) addresses_error: Signal<Option<String>>,
    pub(crate) addresses_page: Signal<Option<crate::wallets::GetAccountAddressesResponse>>,
}

impl AccountAddressesLoader {
    pub(crate) fn request_page(self, page_to_load: u32) {
        if page_to_load == 0 || (self.addresses_loading)() {
            return;
        }

        let mut addresses_loading = self.addresses_loading;
        let mut addresses_error = self.addresses_error;
        let mut addresses_page = self.addresses_page;
        let auth_state = self.auth_state;
        let banner_state = self.banner_state;
        let account_id = self.account_id;
        let address_scheme = self.address_scheme;

        addresses_loading.set(true);
        addresses_error.set(None);

        spawn(async move {
            let request = GetAccountAddressesRequest {
                account_id,
                address_scheme,
                page: Some(page_to_load),
                page_size: Some(DEFAULT_ACCOUNT_ADDRESSES_PAGE_SIZE),
            };

            match crate::backend::get_account_addresses(request).await {
                Ok(response) => addresses_page.set(Some(response)),
                Err(err) => {
                    if err.is_unauthorized() {
                        handle_session_expired(auth_state, banner_state, "account addresses");
                    }
                    addresses_error.set(Some(err.to_string()));
                }
            }

            addresses_loading.set(false);
        });
    }
}

/// How the "view addresses" modal should render an address's transaction count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TxCountDisplay {
    /// No integration-reported total — show the synced count alone.
    Unknown(u32),
    /// Synced count equals or exceeds the reported total — fully synced.
    Complete(u32),
    /// Fewer transactions synced than the integration reports exist.
    Partial { synced: u32, reported: u32 },
}

/// Decide how to display an address transaction count given the synced count
/// and the optional integration-reported total.
pub(super) fn transaction_count_display(synced: u32, reported: Option<u32>) -> TxCountDisplay {
    match reported {
        None => TxCountDisplay::Unknown(synced),
        Some(reported) if reported > synced => TxCountDisplay::Partial { synced, reported },
        Some(_) => TxCountDisplay::Complete(synced),
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::components::wallets::truncate_reference;
    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    use crate::wallets::AccountIndex;
    use chrono::DateTime;

    #[test]
    fn truncate_reference_shortens_long_values() {
        assert_eq!(
            truncate_reference("abcdefghijKLMNOPQRSTuvwxyz"),
            "abcdefghij\u{2026}uvwxyz"
        );
    }

    #[test]
    fn truncate_reference_keeps_short_values() {
        assert_eq!(truncate_reference("GOLD"), "GOLD");
        // Exactly 17 chars: truncating would not shorten it, so leave it.
        assert_eq!(truncate_reference("abcdefghijklmnopq"), "abcdefghijklmnopq");
    }

    #[test]
    fn truncate_reference_truncates_eth_address() {
        assert_eq!(
            truncate_reference("0x52908400098527886E0F7030069857D2E4169EE7"),
            "0x52908400\u{2026}169EE7"
        );
    }

    #[test]
    fn truncate_reference_with_lengths_is_char_safe() {
        assert_eq!(
            truncate_reference_with_lengths("åßçðéƒghijKLMNOPQRSTuvwxyz", 8, 6),
            "åßçðéƒgh\u{2026}uvwxyz"
        );
    }

    #[test]
    fn truncate_reference_with_lengths_keeps_boundary_values() {
        assert_eq!(truncate_reference_with_lengths("0xabcd", 8, 6), "0xabcd");
        let exact = "0123456789abcde";
        assert_eq!(truncate_reference_with_lengths(exact, 8, 6), exact);
    }

    #[test]
    fn truncate_reference_with_lengths_truncates_long_values() {
        let long = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        assert_eq!(
            truncate_reference_with_lengths(long, 8, 6),
            "0x123456\u{2026}abcdef"
        );
    }

    #[test]
    fn account_row_subline_formats_bitcoin_scheme_and_reference() {
        assert_eq!(
            account_row_subline(
                crate::wallets::SyncedAssetId::Bitcoin,
                crate::wallets::AddressScheme::NativeSegwit,
                "abcdefghijKLMNOPQRSTuvwxyz",
            ),
            Some("Native SegWit \u{00B7} abcdefghij\u{2026}uvwxyz".to_string())
        );
    }

    #[test]
    fn account_row_subline_suppresses_scheme_for_ethereum() {
        assert_eq!(
            account_row_subline(
                crate::wallets::SyncedAssetId::Ethereum,
                crate::wallets::AddressScheme::Standard,
                "0x52908400098527886E0F7030069857D2E4169EE7",
            ),
            Some("0x52908400\u{2026}169EE7".to_string())
        );
    }

    #[test]
    fn account_row_subline_handles_empty_reference() {
        assert_eq!(
            account_row_subline(
                crate::wallets::SyncedAssetId::Bitcoin,
                crate::wallets::AddressScheme::Legacy,
                ""
            ),
            Some("Legacy".to_string())
        );
        assert_eq!(
            account_row_subline(
                crate::wallets::SyncedAssetId::Ethereum,
                crate::wallets::AddressScheme::Standard,
                ""
            ),
            None
        );
    }

    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    fn account_index_for_test(value: u32) -> AccountIndex {
        match AccountIndex::new(value) {
            Ok(account_index) => account_index,
            Err(err) => panic!("invalid test account index {value}: {err}"),
        }
    }

    fn xpub_default_scheme_input_for_test(
        address_scheme: AddressScheme,
        has_activity: Option<bool>,
        already_linked: bool,
    ) -> XpubDefaultSchemeInput {
        XpubDefaultSchemeInput {
            address_scheme,
            has_activity,
            already_linked,
        }
    }

    fn parse_utc(timestamp: &str) -> DateTime<Utc> {
        timestamp
            .parse::<DateTime<Utc>>()
            .unwrap_or_else(|err| panic!("invalid test timestamp {timestamp}: {err}"))
    }

    #[test]
    fn format_sync_absolute_time_is_compact_utc() {
        assert_eq!(
            format_sync_absolute_time(parse_utc("2026-07-03T09:12:00Z")),
            "2026-07-03 09:12 UTC"
        );
    }

    #[test]
    fn sync_result_word_covers_all_results() {
        use crate::transactions::AccountSyncResult;
        assert_eq!(
            sync_result_word(Some(AccountSyncResult::Success)),
            "success"
        );
        assert_eq!(
            sync_result_word(Some(AccountSyncResult::Partial)),
            "partial"
        );
        assert_eq!(sync_result_word(Some(AccountSyncResult::Failure)), "failed");
        assert_eq!(
            sync_result_word(Some(AccountSyncResult::InProgress)),
            "running"
        );
        assert_eq!(sync_result_word(None), "—");
    }

    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    #[test]
    fn trezor_error_text_is_non_empty() {
        let kinds = [
            trezor::TrezorErrorKind::BridgeNotRunning,
            trezor::TrezorErrorKind::NoDevices,
            trezor::TrezorErrorKind::DeviceDisconnected,
            trezor::TrezorErrorKind::UserCancelled,
            trezor::TrezorErrorKind::PinRequired,
            trezor::TrezorErrorKind::PassphraseRequired,
            trezor::TrezorErrorKind::ProtocolError,
            trezor::TrezorErrorKind::InternalError,
            trezor::TrezorErrorKind::SessionExpired,
            trezor::TrezorErrorKind::SessionConflict,
            trezor::TrezorErrorKind::BridgeRejected,
            trezor::TrezorErrorKind::BridgeError,
            trezor::TrezorErrorKind::DeviceError,
            trezor::TrezorErrorKind::MissingFingerprint,
            trezor::TrezorErrorKind::MissingMasterFingerprint,
            trezor::TrezorErrorKind::InvalidFingerprint,
            trezor::TrezorErrorKind::NoAccountsSelected,
            trezor::TrezorErrorKind::MissingZpubData,
            trezor::TrezorErrorKind::WrongDeviceConnected,
            trezor::TrezorErrorKind::ConnectInitParseFailed,
            trezor::TrezorErrorKind::ConnectInitFailed,
            trezor::TrezorErrorKind::ConnectFingerprintParseFailed,
            trezor::TrezorErrorKind::ConnectFingerprintFailed,
            trezor::TrezorErrorKind::ConnectAccountIndexesSerializeFailed,
            trezor::TrezorErrorKind::ConnectZpubParseFailed,
            trezor::TrezorErrorKind::ConnectZpubFailed,
        ];

        for kind in kinds {
            let (message, troubleshooting) = trezor_error_text(kind);
            assert!(!message.is_empty(), "missing message for {:?}", kind);
            assert!(
                !troubleshooting.is_empty(),
                "missing troubleshooting for {:?}",
                kind
            );
        }
    }

    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    #[test]
    fn missing_supported_address_schemes_filters_linked_values() {
        let linked = vec![AddressScheme::Legacy, AddressScheme::NativeSegwit];

        let missing = missing_supported_address_schemes(&linked);

        assert_eq!(missing, vec![AddressScheme::NestedSegwit]);
    }

    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    #[test]
    fn available_schemes_for_account_filters_by_existing_links() {
        let existing = vec![ExistingAccountAddressTypes {
            account: account_index_for_test(0),
            linked_schemes: vec![AddressScheme::Legacy, AddressScheme::NestedSegwit],
        }];

        let available_for_existing =
            available_schemes_for_account(&existing, account_index_for_test(0));
        let available_for_new = available_schemes_for_account(&existing, account_index_for_test(1));

        assert_eq!(available_for_existing, vec![AddressScheme::NativeSegwit]);
        assert_eq!(
            available_for_new,
            vec![
                AddressScheme::Legacy,
                AddressScheme::NestedSegwit,
                AddressScheme::NativeSegwit,
            ]
        );
    }

    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    #[test]
    fn default_selection_prefers_native_segwit_when_available() {
        let with_native = default_selection_for_available_schemes(&[
            AddressScheme::Legacy,
            AddressScheme::NativeSegwit,
        ]);
        let without_native = default_selection_for_available_schemes(&[
            AddressScheme::Legacy,
            AddressScheme::NestedSegwit,
        ]);
        let empty = default_selection_for_available_schemes(&[]);

        assert_eq!(with_native, Some(AddressScheme::NativeSegwit));
        assert_eq!(without_native, Some(AddressScheme::Legacy));
        assert_eq!(empty, None);
    }

    #[test]
    fn select_default_xpub_scheme_prefers_first_active_unlinked_in_display_order() {
        let schemes = vec![
            xpub_default_scheme_input_for_test(AddressScheme::NativeSegwit, Some(true), false),
            xpub_default_scheme_input_for_test(AddressScheme::Legacy, Some(true), true),
            xpub_default_scheme_input_for_test(AddressScheme::NestedSegwit, Some(true), false),
        ];

        assert_eq!(
            select_default_xpub_scheme(AddressScheme::NativeSegwit, &schemes),
            Some(AddressScheme::NestedSegwit)
        );
    }

    #[test]
    fn select_default_xpub_scheme_falls_back_to_prefix_suggested_when_unlinked() {
        let schemes = vec![
            xpub_default_scheme_input_for_test(AddressScheme::Legacy, Some(false), false),
            xpub_default_scheme_input_for_test(AddressScheme::NestedSegwit, None, false),
            xpub_default_scheme_input_for_test(AddressScheme::NativeSegwit, None, false),
        ];

        assert_eq!(
            select_default_xpub_scheme(AddressScheme::NativeSegwit, &schemes),
            Some(AddressScheme::NativeSegwit)
        );
    }

    #[test]
    fn select_default_xpub_scheme_falls_back_to_first_unlinked_in_display_order() {
        let schemes = vec![
            xpub_default_scheme_input_for_test(AddressScheme::Legacy, None, true),
            xpub_default_scheme_input_for_test(AddressScheme::NestedSegwit, None, false),
            xpub_default_scheme_input_for_test(AddressScheme::NativeSegwit, None, false),
        ];

        assert_eq!(
            select_default_xpub_scheme(AddressScheme::Legacy, &schemes),
            Some(AddressScheme::NestedSegwit)
        );
    }

    #[test]
    fn select_default_xpub_scheme_returns_none_when_all_schemes_are_linked() {
        let schemes = vec![
            xpub_default_scheme_input_for_test(AddressScheme::Legacy, Some(true), true),
            xpub_default_scheme_input_for_test(AddressScheme::NestedSegwit, Some(false), true),
            xpub_default_scheme_input_for_test(AddressScheme::NativeSegwit, None, true),
        ];

        assert_eq!(
            select_default_xpub_scheme(AddressScheme::NativeSegwit, &schemes),
            None
        );
    }

    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    #[test]
    fn display_account_number_is_one_based() {
        let zero_based = account_index_for_test(0);
        let next = account_index_for_test(1);

        assert_eq!(display_account_number(zero_based), 1);
        assert_eq!(display_account_number(next), 2);
    }

    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    #[test]
    fn parse_display_account_number_maps_one_based_to_zero_based() {
        let parsed_first = parse_display_account_number(1);
        let parsed_fifth = parse_display_account_number(5);
        let parsed_zero = parse_display_account_number(0);

        assert_eq!(parsed_first.map(|value| value.as_u32()), Ok(0));
        assert_eq!(parsed_fifth.map(|value| value.as_u32()), Ok(4));
        assert!(parsed_zero.is_err());
    }

    #[test]
    fn logical_account_count_label_formats_singular_and_plural() {
        assert_eq!(logical_account_count_label(1), "1 account");
        assert_eq!(logical_account_count_label(2), "2 accounts");
    }

    #[test]
    fn addresses_total_pages_handles_empty_and_partial_pages() {
        assert_eq!(addresses_total_pages(0, 50), 1);
        assert_eq!(addresses_total_pages(1, 50), 1);
        assert_eq!(addresses_total_pages(50, 50), 1);
        assert_eq!(addresses_total_pages(51, 50), 2);
    }

    #[test]
    fn format_sync_relative_time_matches_expected_ranges() {
        let now = parse_utc("2026-03-01T22:00:00Z");
        assert_eq!(
            format_sync_relative_time(now, parse_utc("2026-03-01T21:59:31Z")),
            "just now"
        );
        assert_eq!(
            format_sync_relative_time(now, parse_utc("2026-03-01T21:30:00Z")),
            "30m ago"
        );
        assert_eq!(
            format_sync_relative_time(now, parse_utc("2026-03-01T20:00:00Z")),
            "2h ago"
        );
        assert_eq!(
            format_sync_relative_time(now, parse_utc("2026-02-28T22:00:00Z")),
            "1d ago"
        );
        assert_eq!(
            format_sync_relative_time(now, parse_utc("2026-02-20T22:00:00Z")),
            "2026-02-20"
        );
    }

    #[test]
    fn transaction_count_display_unknown_when_no_reported_count() {
        assert_eq!(
            transaction_count_display(3, None),
            TxCountDisplay::Unknown(3),
        );
    }

    #[test]
    fn transaction_count_display_partial_when_reported_exceeds_synced() {
        assert_eq!(
            transaction_count_display(3, Some(8)),
            TxCountDisplay::Partial {
                synced: 3,
                reported: 8
            },
        );
    }

    #[test]
    fn transaction_count_display_complete_when_synced_matches_reported() {
        assert_eq!(
            transaction_count_display(8, Some(8)),
            TxCountDisplay::Complete(8),
        );
    }

    #[test]
    fn transaction_count_display_complete_when_synced_exceeds_reported() {
        // Reconciliation may discover more than the integration reported.
        assert_eq!(
            transaction_count_display(9, Some(8)),
            TxCountDisplay::Complete(9),
        );
    }
}

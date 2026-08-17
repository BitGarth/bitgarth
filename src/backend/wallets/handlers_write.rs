#[cfg(feature = "server")]
use crate::db::{
    add_bitcoin_address_with_account_label as add_bitcoin_address_db,
    add_ethereum_address_with_account_label as add_ethereum_address_db,
    add_manual_asset_account as add_manual_asset_account_db,
    add_xpub_wallet_with_account_label as add_xpub_wallet_db,
    create_wallet_and_move_account as create_wallet_and_move_account_db,
    delete_wallet as delete_wallet_db, delete_wallet_account as delete_wallet_account_db,
    derive_address_from_extended_pubkey as derive_address_db,
    find_extended_pubkey_scheme_link as find_extended_pubkey_scheme_link_db,
    find_wallet_for_extended_pubkey as find_wallet_for_extended_pubkey_db,
    link_trezor_wallet as link_trezor_wallet_db, load_account_sync_slot_map, load_settings,
    move_account_to_wallet as move_account_to_wallet_db,
    select_account_sync_slot as select_account_sync_slot_db,
    update_wallet_account_label as update_wallet_account_label_db,
    update_wallet_label as update_wallet_label_db,
};
#[cfg(feature = "server")]
use crate::models::{FieldErrors, resolve_effective_mempool_base_url};
#[cfg(feature = "server")]
use crate::payments::types::EntitlementTier;
#[cfg(feature = "server")]
use crate::tasks::automatic_sync::AutomaticSyncAddTarget;
#[cfg(feature = "server")]
use crate::traces::client::{IntegrationLabel, TracedAsyncClient};
#[cfg(feature = "server")]
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, AddManualAssetAccountValidationError, AddressScheme,
    ValidatedMoveDestination, WALLET_LABEL_MAX_LENGTH, detect_address_scheme_from_prefix,
    validate_link_trezor_request,
};
use crate::wallets::{
    AddBtcAddressRequest, AddBtcAddressResponse, AddEthAddressRequest, AddEthAddressResponse,
    AddManualAssetAccountRequest, AddManualAssetAccountResponse, AddXpubRequest,
    DeleteAccountRequest, DeleteWalletRequest, LinkTrezorRequest, LinkTrezorResponse,
    MoveAccountRequest, MoveAccountResponse, SelectAccountSyncSlotRequest,
    UpdateAccountLabelRequest, UpdateWalletLabelRequest, ValidateXpubRequest,
};
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use chrono::Utc;
#[cfg(feature = "server")]
use dioxus::logger::tracing;
use dioxus::prelude::*;

#[cfg(feature = "server")]
use super::helpers::{
    enqueue_automatic_add_sync, internal_error, map_link_trezor_db_error,
    map_move_account_db_error, map_wallet_db_error, not_found_error, run_wallet_db_blocking,
    session_token_from_cookie, unauthorized_error, validation_error,
};
#[cfg(feature = "server")]
use super::types::{AccountCreationStateView, AccountLimitNoticeView, AccountStateView};
use super::types::{AddXpubResponse, ValidateXpubResponse, WalletError};
#[cfg(feature = "server")]
use super::types::{AlreadyLinkedWallet, ExistingNormalizedKeyWallet, SchemeValidationResult};
#[cfg(feature = "server")]
use crate::backend::session_context::require_initialized_session;

#[cfg(feature = "server")]
fn account_state_view(state: crate::account_limits::AccountActivationState) -> AccountStateView {
    match state {
        crate::account_limits::AccountActivationState::Active => AccountStateView::Active,
        crate::account_limits::AccountActivationState::Inactive => AccountStateView::Inactive,
    }
}

#[cfg(feature = "server")]
pub(super) fn created_account_state_view(
    account_id: crate::wallets::WalletAccountId,
    state: crate::account_limits::AccountActivationState,
    active_account_limit: u16,
) -> AccountCreationStateView {
    let account_state = account_state_view(state);
    AccountCreationStateView {
        account_id,
        account_state,
        account_limit_notice: (state == crate::account_limits::AccountActivationState::Inactive)
            .then(|| AccountLimitNoticeView {
                message: format!(
                    "You have reached your limit of {active_account_limit} accounts. This account will be inactive until you upgrade."
                ),
                active_account_limit,
            }),
    }
}

#[cfg(feature = "server")]
fn native_account_sync_eligible_for_entitlements(
    user_id: crate::models::UserId,
    account_id: crate::wallets::DigitalAssetAccountId,
    entitlements: &crate::payments::types::FeatureEntitlements,
) -> Result<bool, WalletError> {
    crate::db::account_limits::native_account_sync_eligible_for_user(
        user_id,
        usize::from(entitlements.sync_account_slots_limit),
        account_id,
        entitlements.tier == EntitlementTier::Free,
    )
    .map_err(|e| internal_error("wallets", e))
}

#[cfg(feature = "server")]
fn classify_created_account_for_user(
    user_id: crate::models::UserId,
    account_id: crate::wallets::WalletAccountId,
    active_account_limit: u16,
) -> Result<AccountCreationStateView, WalletError> {
    let classified_accounts = crate::db::account_limits::classify_supported_accounts_for_user(
        user_id,
        usize::from(active_account_limit),
    )
    .map_err(|e| internal_error("classify_created_account", e))?;
    let state = crate::db::account_limits::account_state_for(&classified_accounts, &account_id);
    Ok(created_account_state_view(
        account_id,
        state,
        active_account_limit,
    ))
}

#[post("/_app/user/wallets/trezor/link", cookies: CookieJar)]
pub(crate) async fn link_trezor_wallet(
    request: LinkTrezorRequest,
) -> Result<LinkTrezorResponse, WalletError> {
    tracing::debug!("wallets: link trezor requested");
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;
    let validated = validate_link_trezor_request(request).map_err(validation_error)?;

    tracing::debug!(
        user_id = %user_id,
        fingerprint = %validated.master_fingerprint.as_str(),
        wallet_label = %validated.wallet_label.as_str(),
        "wallets: link trezor authorized"
    );

    tracing::info!(
        fingerprint = %validated.master_fingerprint.as_str(),
        accounts = ?validated
            .accounts
            .iter()
            .map(|account| account.account_index.as_u32())
            .collect::<Vec<_>>(),
        device_id = ?validated.device_id.as_ref().map(|id| id.as_str()),
        device_label = ?validated.device_label.as_ref().map(|label| label.as_str()),
        "linking trezor wallet"
    );

    let now = Utc::now();
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)
        .map_err(|e| internal_error("wallets", e))?;
    let result = link_trezor_wallet_db(
        user_id,
        validated,
        usize::from(entitlements.sync_account_slots_limit),
        now,
    )
    .map_err(map_link_trezor_db_error)?;
    let created_accounts = result
        .created_account_ids
        .iter()
        .map(|account_id| {
            classify_created_account_for_user(
                user_id,
                (*account_id).into(),
                entitlements.sync_account_slots_limit,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let has_sync_eligible_created_account = result.created_account_ids.iter().try_fold(
        false,
        |found, account_id| -> Result<bool, WalletError> {
            Ok(found
                || native_account_sync_eligible_for_entitlements(
                    user_id,
                    *account_id,
                    &entitlements,
                )?)
        },
    )?;
    if has_sync_eligible_created_account {
        enqueue_automatic_add_sync(user_id, AutomaticSyncAddTarget::MultiAccountImport).await;
    }
    Ok(LinkTrezorResponse {
        wallet_id: result.wallet_id,
        created_account_ids: result.created_account_ids,
        created_accounts,
        skipped_account_indexes: result.skipped_account_indexes,
        outcome: result.outcome,
    })
}

#[post("/_app/user/wallets/label", cookies: CookieJar)]
pub(crate) async fn update_wallet_label(
    request: UpdateWalletLabelRequest,
) -> Result<(), WalletError> {
    tracing::debug!(
        wallet_id = %request.wallet_id,
        "wallets: update wallet label requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    tracing::debug!(
        user_id = %user_id,
        wallet_id = %request.wallet_id,
        "wallets: update wallet label authorized"
    );

    let label = request
        .label
        .validate(WALLET_LABEL_MAX_LENGTH)
        .map_err(|err| {
            let mut errors = FieldErrors::new();
            errors.add("label", err.to_string());
            validation_error(errors)
        })?;

    update_wallet_label_db(user_id, request.wallet_id, label, Utc::now())
        .map_err(|e| map_wallet_db_error(e, "label"))
}

#[post("/_app/user/wallets/account/label", cookies: CookieJar)]
pub(crate) async fn update_account_label(
    request: UpdateAccountLabelRequest,
) -> Result<(), WalletError> {
    tracing::debug!(
        account_id = %request.account_id,
        "wallets: update account label requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    tracing::debug!(
        user_id = %user_id,
        account_id = %request.account_id,
        "wallets: update account label authorized"
    );

    let label = request
        .label
        .validate(ACCOUNT_LABEL_MAX_LENGTH)
        .map_err(|err| {
            let mut errors = FieldErrors::new();
            errors.add("label", err.to_string());
            validation_error(errors)
        })?;

    update_wallet_account_label_db(user_id, request.account_id, label, Utc::now())
        .map_err(|e| map_wallet_db_error(e, "label"))
}

#[post("/_app/user/wallets/account/delete", cookies: CookieJar)]
pub(crate) async fn delete_account(request: DeleteAccountRequest) -> Result<(), WalletError> {
    tracing::debug!(
        account_id = %request.account_id,
        "wallets: delete account requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    tracing::debug!(
        user_id = %user_id,
        account_id = %request.account_id,
        "wallets: delete account authorized"
    );

    let account_id = request.account_id;
    run_wallet_db_blocking(
        move || delete_wallet_account_db(user_id, account_id),
        "Delete account task join failed",
    )
    .await
}

#[post("/_app/user/wallets/account/move", cookies: CookieJar)]
pub(crate) async fn move_wallet_account(
    request: MoveAccountRequest,
) -> Result<MoveAccountResponse, WalletError> {
    tracing::debug!(
        account_id = %request.account_id,
        "wallets: move account requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = request.try_into_validated().map_err(validation_error)?;

    tracing::debug!(
        user_id = %user_id,
        account_id = %validated.account_id,
        "wallets: move account authorized"
    );

    let now = Utc::now();
    let destination_wallet_id = match validated.destination {
        ValidatedMoveDestination::ExistingWallet { wallet_id } => {
            move_account_to_wallet_db(user_id, validated.account_id, wallet_id, now)
                .map_err(map_move_account_db_error)?;
            wallet_id
        }
        ValidatedMoveDestination::NewWallet { label } => {
            create_wallet_and_move_account_db(user_id, validated.account_id, label, now)
                .map_err(map_move_account_db_error)?
        }
    };

    tracing::debug!(
        user_id = %user_id,
        account_id = %validated.account_id,
        destination_wallet_id = %destination_wallet_id,
        "wallets: move account completed"
    );

    Ok(MoveAccountResponse {
        destination_wallet_id,
    })
}

#[post("/_app/user/wallets/account/sync-slot/select", cookies: CookieJar)]
pub(crate) async fn select_account_sync_slot(
    request: SelectAccountSyncSlotRequest,
) -> Result<(), WalletError> {
    tracing::debug!(
        account_id = %request.account_id,
        "wallets: select account sync slot requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)
        .map_err(|e| internal_error("wallets", e))?;
    if !crate::db::account_exists(user_id, request.account_id)
        .map_err(|e| internal_error("wallets", e))?
    {
        return Err(not_found_error("Account not found"));
    }

    let eligibility = crate::db::account_limits::native_account_sync_eligibility_for_user(
        user_id,
        usize::from(entitlements.sync_account_slots_limit),
        request.account_id,
        entitlements.tier == EntitlementTier::Free,
    )
    .map_err(|e| internal_error("wallets", e))?;

    if !eligibility.account_active {
        let mut errors = FieldErrors::new();
        errors.add(
            "account_id",
            "Upgrade to activate this account.".to_string(),
        );
        return Err(validation_error(errors));
    }

    if !eligibility.provider_or_plan_supports_requested_sync {
        let mut errors = FieldErrors::new();
        errors.add(
            "account_id",
            "Balance sync unavailable on Free.".to_string(),
        );
        return Err(validation_error(errors));
    }

    let existing_slots =
        load_account_sync_slot_map(user_id).map_err(|e| internal_error("wallets", e))?;
    if existing_slots.contains_key(&request.account_id) {
        return Ok(());
    }

    if existing_slots.len() >= usize::from(entitlements.sync_account_slots_limit) {
        let mut errors = FieldErrors::new();
        errors.add(
            "account_id",
            "Your plan has no free sync slots.".to_string(),
        );
        return Err(validation_error(errors));
    }

    select_account_sync_slot_db(user_id, request.account_id, &entitlements.tier, now)
        .map_err(|e| internal_error("wallets", e))?;

    Ok(())
}

#[post("/_app/user/wallets/delete", cookies: CookieJar)]
pub(crate) async fn delete_wallet(request: DeleteWalletRequest) -> Result<(), WalletError> {
    tracing::debug!(
        wallet_id = %request.wallet_id,
        delete_accounts = request.delete_accounts.value(),
        "wallets: delete requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    tracing::debug!(
        user_id = %user_id,
        wallet_id = %request.wallet_id,
        "wallets: delete authorized"
    );

    if !request.delete_accounts.value() {
        let mut errors = FieldErrors::new();
        errors.add(
            "delete_accounts",
            "Keeping accounts is not supported yet. Please choose to delete accounts.".to_string(),
        );
        return Err(validation_error(errors));
    }

    let wallet_id = request.wallet_id;
    run_wallet_db_blocking(
        move || delete_wallet_db(user_id, wallet_id),
        "Delete wallet task join failed",
    )
    .await
}

#[cfg(feature = "server")]
const MEMPOOL_REQUEST_TIMEOUT_SECONDS: u64 = 10;

#[cfg(feature = "server")]
async fn check_address_activity(
    client: &TracedAsyncClient,
    mempool_base_url: &crate::models::MempoolBaseUrl,
    address: &str,
) -> Result<bool, String> {
    let url = format!("{}api/address/{}/txs", mempool_base_url.as_str(), address);
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Could not connect to mempool API: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Mempool API returned status {}", response.status()));
    }
    let body = response
        .text()
        .map_err(|e| format!("Failed to read mempool response: {e}"))?;
    Ok(body.trim() != "[]")
}

#[cfg(feature = "server")]
fn apply_activity_result(result: &mut SchemeValidationResult, activity: Result<bool, String>) {
    match activity {
        Ok(has_activity) => result.has_activity = Some(has_activity),
        Err(err) => result.activity_check_error = Some(err),
    }
}

#[cfg(feature = "server")]
fn map_activity_join_result(
    result: Result<Result<bool, String>, tokio::task::JoinError>,
) -> Result<bool, String> {
    match result {
        Ok(activity) => activity,
        Err(_) => Err("Could not check activity due to unexpected runtime failure".to_string()),
    }
}

#[post("/_app/user/wallets/xpub/validate", cookies: CookieJar)]
pub(crate) async fn validate_xpub(
    request: ValidateXpubRequest,
) -> Result<ValidateXpubResponse, WalletError> {
    tracing::debug!("wallets: validate xpub requested");

    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = request.try_into_validated().map_err(validation_error)?;

    // Suggest scheme from prefix (xpub->Legacy, ypub->NestedSegwit, zpub->NativeSegwit)
    let suggested_scheme = detect_address_scheme_from_prefix(&validated.extended_pubkey)
        .unwrap_or(AddressScheme::Legacy);

    // Check whether any scheme variant for this normalized key already exists.
    let existing_wallet = find_wallet_for_extended_pubkey_db(user_id, &validated.extended_pubkey)
        .map_err(|e| internal_error("find_wallet_for_extended_pubkey", e))?;
    let existing_wallet =
        existing_wallet.map(|(wallet_id, wallet_label)| ExistingNormalizedKeyWallet {
            wallet_id,
            wallet_label,
        });

    // Derive first receive address (index 0) for each of the 3 Bitcoin schemes.
    let btc_schemes = [
        AddressScheme::Legacy,
        AddressScheme::NestedSegwit,
        AddressScheme::NativeSegwit,
    ];
    let mut scheme_results: Vec<SchemeValidationResult> = Vec::with_capacity(3);

    for &scheme in &btc_schemes {
        let linked =
            find_extended_pubkey_scheme_link_db(user_id, &validated.extended_pubkey, scheme)
                .map_err(|e| internal_error("find_extended_pubkey_scheme_link", e))?;
        let first_address = derive_address_db(
            crate::wallets::SyncedAssetId::Bitcoin,
            crate::wallets::Network::Mainnet,
            scheme,
            &validated.extended_pubkey,
            0, // receive chain
            0, // index 0
        )
        .map_err(|e| internal_error("derive_address", e))?;

        let (already_linked, linked_wallet_label, linked_account_label) = match linked {
            Some(link) => (
                true,
                Some(link.wallet_label.clone()),
                Some(link.account_label.clone()),
            ),
            None => (false, None, None),
        };

        scheme_results.push(SchemeValidationResult {
            address_scheme: scheme,
            scheme_note: scheme.scheme_note().unwrap_or_default().to_string(),
            first_address,
            has_activity: None,
            activity_check_error: None,
            already_linked,
            linked_wallet_label,
            linked_account_label,
        });
    }

    // Check activity on each scheme's address via mempool API (concurrent)
    let settings = load_settings(user_id).map_err(|e| internal_error("wallets", e))?;
    match resolve_effective_mempool_base_url(settings.mempool_base_url.as_ref()) {
        Ok((mempool_base_url, _source)) => {
            let spawn_activity_check = |address: String| {
                let mempool_base_url = mempool_base_url.clone();
                tokio::spawn(async move {
                    let client =
                        TracedAsyncClient::builder(IntegrationLabel::new("mempool"), user_id)
                            .configure(|b| {
                                b.timeout(std::time::Duration::from_secs(
                                    MEMPOOL_REQUEST_TIMEOUT_SECONDS,
                                ))
                            })
                            .build()
                            .map_err(|e| format!("Could not build mempool HTTP client: {e}"))?;
                    check_address_activity(&client, &mempool_base_url, &address).await
                })
            };

            let handle0 = spawn_activity_check(scheme_results[0].first_address.clone());
            let handle1 = spawn_activity_check(scheme_results[1].first_address.clone());
            let handle2 = spawn_activity_check(scheme_results[2].first_address.clone());

            let (r0, r1, r2) = tokio::join!(handle0, handle1, handle2);

            apply_activity_result(&mut scheme_results[0], map_activity_join_result(r0));
            apply_activity_result(&mut scheme_results[1], map_activity_join_result(r1));
            apply_activity_result(&mut scheme_results[2], map_activity_join_result(r2));
        }
        Err(e) => {
            for result in &mut scheme_results {
                result.activity_check_error = Some(format!("Invalid mempool base URL: {e}"));
            }
        }
    }

    // Backward-compatible terminal marker: only emit when all schemes are
    // already linked for this normalized key family.
    let already_linked = if scheme_results.iter().all(|result| result.already_linked) {
        existing_wallet.clone().map(|wallet| AlreadyLinkedWallet {
            wallet_id: wallet.wallet_id,
            wallet_label: wallet.wallet_label,
        })
    } else {
        None
    };

    tracing::debug!(
        user_id = %user_id,
        suggested_scheme = suggested_scheme.as_str(),
        existing_wallet = existing_wallet.is_some(),
        already_linked = already_linked.is_some(),
        "wallets: validate xpub completed"
    );

    Ok(ValidateXpubResponse {
        schemes: scheme_results,
        suggested_scheme,
        existing_wallet,
        already_linked,
    })
}

#[post("/_app/user/wallets/xpub/add", cookies: CookieJar)]
pub(crate) async fn add_xpub(request: AddXpubRequest) -> Result<AddXpubResponse, WalletError> {
    tracing::debug!("wallets: add xpub requested");

    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = request.try_into_validated().map_err(validation_error)?;

    tracing::debug!(
        user_id = %user_id,
        address_scheme = validated.extended_pubkey.address_scheme().as_str(),
        wallet_id = ?validated.wallet_id,
        has_label = validated.wallet_label.is_some(),
        "wallets: add xpub authorized"
    );

    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|e| internal_error("wallets", e))?;
    let result = add_xpub_wallet_db(
        user_id,
        &validated.extended_pubkey,
        validated.wallet_id,
        validated.wallet_label.as_ref(),
        validated.account_label.as_ref(),
        usize::from(entitlements.sync_account_slots_limit),
        Utc::now(),
    )
    .map_err(|e| map_wallet_db_error(e, "wallet_label"))?;
    let created_account = classify_created_account_for_user(
        user_id,
        result.account_id.into(),
        entitlements.sync_account_slots_limit,
    )?;

    tracing::debug!(
        user_id = %user_id,
        wallet_id = %result.wallet_id,
        account_id = %result.account_id,
        "wallets: add xpub completed"
    );

    if native_account_sync_eligible_for_entitlements(user_id, result.account_id, &entitlements)? {
        enqueue_automatic_add_sync(
            user_id,
            AutomaticSyncAddTarget::Account {
                account_id: result.account_id,
            },
        )
        .await;
    }

    Ok(AddXpubResponse {
        wallet_id: result.wallet_id,
        account_id: result.account_id,
        account_state: created_account.account_state,
        account_limit_notice: created_account.account_limit_notice,
    })
}

#[post("/_app/user/wallets/manual-assets/add", cookies: CookieJar)]
pub(crate) async fn add_manual_asset_account(
    request: AddManualAssetAccountRequest,
) -> Result<AddManualAssetAccountResponse, WalletError> {
    tracing::debug!(
        has_wallet_id = request.wallet_id.is_some(),
        has_wallet_label = request.wallet_label.is_some(),
        "wallets: add manual asset account requested"
    );

    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = request.try_into_validated().map_err(|err| match err {
        AddManualAssetAccountValidationError::Fields(errors) => validation_error(errors),
        AddManualAssetAccountValidationError::Catalog(err) => {
            internal_error("manual_asset_catalog_lookup", err)
        }
    })?;

    tracing::debug!(
        user_id = %user_id,
        has_wallet_id = validated.wallet_id.is_some(),
        has_wallet_label = validated.wallet_label.is_some(),
        asset_source = %validated.asset_source_label(),
        "wallets: add manual asset account authorized"
    );

    let result = add_manual_asset_account_db(user_id, validated, Utc::now()).map_err(|e| {
        if e.to_string().contains("Wallet not found") {
            not_found_error("Wallet not found")
        } else {
            map_wallet_db_error(e, "wallet_label")
        }
    })?;

    tracing::debug!(
        user_id = %user_id,
        wallet_id = %result.wallet_id,
        account_id = %result.account_id,
        "wallets: add manual asset account completed"
    );

    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|e| internal_error("wallets", e))?;
    let created_account = classify_created_account_for_user(
        user_id,
        result.account_id,
        entitlements.sync_account_slots_limit,
    )?;

    Ok(AddManualAssetAccountResponse {
        wallet_id: result.wallet_id,
        account_id: result.account_id,
        account_state: created_account.account_state,
        account_limit_notice: created_account.account_limit_notice,
    })
}

#[post("/_app/user/wallets/ethereum/add", cookies: CookieJar)]
pub(crate) async fn add_ethereum_address(
    request: AddEthAddressRequest,
) -> Result<AddEthAddressResponse, WalletError> {
    tracing::debug!("wallets: add ethereum address requested");

    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = request.try_into_validated().map_err(validation_error)?;

    tracing::debug!(
        user_id = %user_id,
        address = %validated.address,
        network = validated.network.as_str(),
        existing_wallet = ?validated.wallet_id,
        has_label = validated.wallet_label.is_some(),
        "wallets: add ethereum address authorized"
    );

    let result = add_ethereum_address_db(
        user_id,
        &validated.address,
        validated.network,
        validated.wallet_id.as_ref(),
        validated.wallet_label.as_ref(),
        validated.account_label.as_ref(),
        Utc::now(),
    )
    .map_err(|e| map_wallet_db_error(e, "wallet_label"))?;
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|e| internal_error("wallets", e))?;
    let created_account = classify_created_account_for_user(
        user_id,
        result.account_id.into(),
        entitlements.sync_account_slots_limit,
    )?;

    tracing::debug!(
        user_id = %user_id,
        wallet_id = %result.wallet_id,
        account_id = %result.account_id,
        address_id = %result.address_id,
        "wallets: add ethereum address completed"
    );

    if native_account_sync_eligible_for_entitlements(user_id, result.account_id, &entitlements)? {
        enqueue_automatic_add_sync(
            user_id,
            AutomaticSyncAddTarget::EthereumAddress {
                address_id: result.address_id,
            },
        )
        .await;
    }

    Ok(AddEthAddressResponse {
        wallet_id: result.wallet_id,
        account_id: result.account_id,
        address_id: result.address_id,
        account_state: created_account.account_state,
        account_limit_notice: created_account.account_limit_notice,
    })
}

#[post("/_app/user/wallets/bitcoin/add", cookies: CookieJar)]
pub(crate) async fn add_bitcoin_address(
    request: AddBtcAddressRequest,
) -> Result<AddBtcAddressResponse, WalletError> {
    tracing::debug!("wallets: add bitcoin address requested");

    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session =
        require_initialized_session("wallets", &session_token, unauthorized_error, |message| {
            internal_error("require_initialized_session", message)
        })?;
    let user_id = initialized_session.session.user_id;

    let validated = request.try_into_validated().map_err(validation_error)?;

    tracing::debug!(
        user_id = %user_id,
        address = %validated.address,
        network = validated.network.as_str(),
        address_scheme = validated.address.address_scheme().as_str(),
        existing_wallet = ?validated.wallet_id,
        has_label = validated.wallet_label.is_some(),
        "wallets: add bitcoin address authorized"
    );

    let result = add_bitcoin_address_db(
        user_id,
        &validated.address,
        validated.network,
        validated.wallet_id.as_ref(),
        validated.wallet_label.as_ref(),
        validated.account_label.as_ref(),
        Utc::now(),
    )
    .map_err(|e| map_wallet_db_error(e, "wallet_label"))?;
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, Utc::now())
            .map_err(|e| internal_error("wallets", e))?;
    let created_account = classify_created_account_for_user(
        user_id,
        result.account_id.into(),
        entitlements.sync_account_slots_limit,
    )?;

    tracing::debug!(
        user_id = %user_id,
        wallet_id = %result.wallet_id,
        account_id = %result.account_id,
        address_id = %result.address_id,
        "wallets: add bitcoin address completed"
    );

    if native_account_sync_eligible_for_entitlements(user_id, result.account_id, &entitlements)? {
        enqueue_automatic_add_sync(
            user_id,
            AutomaticSyncAddTarget::BitcoinAddress {
                address_id: result.address_id,
            },
        )
        .await;
    }

    Ok(AddBtcAddressResponse {
        wallet_id: result.wallet_id,
        account_id: result.account_id,
        address_id: result.address_id,
        account_state: created_account.account_state,
        account_limit_notice: created_account.account_limit_notice,
    })
}

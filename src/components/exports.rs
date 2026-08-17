use super::HostedRetentionNotice;
use super::PasswordInput;
use super::SectionHead;
use crate::backend::{
    ConfirmPremiumTransferRequest, DescribeWalletDataImportRequest, ExportError,
    ExportWalletDataRequest, ImportResultView, ImportWalletDataRequest,
    PremiumTransferImportStatusView, PremiumTransferResultView, PremiumTransferStatusView,
    WalletDataExportCounts, WalletDataExportDownloadView, WalletDataExportSummary,
    WalletDataImportDescription, confirm_premium_transfer, describe_wallet_data_import,
    export_wallet_data, get_hledger_export_settings, get_wallet_data_export_options,
    import_wallet_data, save_hledger_account_prefix,
};
use crate::components::form_helpers::{
    begin_submit, finish_submit, is_form_field_error, primary_field_or_fallback,
};
use crate::{AuthState, AuthStatus, BannerMessage, BannerState, Route};
use dioxus::document::eval;
use dioxus::prelude::*;

const WALLET_DATA_SUBSCRIPTION_TRANSFER_NOTICE: &str = "This export will include a subscription transfer secret. Anyone with this file may move your subscription to another BitGarth user. Store it securely and delete old copies after migration.";
const WALLET_DATA_IMPORT_FILE_ID: &str = "wallet-data-import-file";
const MAX_IMPORT_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;

fn handle_session_expired(mut auth_state: AuthState, mut banner_state: BannerState) {
    let was_authenticated = matches!(&*auth_state.read(), AuthStatus::Authenticated(_));
    auth_state.set(AuthStatus::Unauthenticated);
    if was_authenticated {
        banner_state.set(Some(BannerMessage::SessionExpired));
    }
}

async fn download_wallet_data_file(file_name: &str, zip_base64: &str) -> Result<(), String> {
    let encoded_file_name = serde_json::to_string(file_name)
        .map_err(|err| format!("Failed to encode export filename: {err}"))?;
    let encoded_zip_base64 = serde_json::to_string(zip_base64)
        .map_err(|err| format!("Failed to encode export payload for download: {err}"))?;

    let script = format!(
        r#"
(() => {{
  try {{
    if (typeof window === "undefined" || typeof document === "undefined") {{
      dioxus.send("Export download is only available in browser builds.");
      return;
    }}

    const fileName = {encoded_file_name};
    const zipBase64 = {encoded_zip_base64};
    const binary = atob(zipBase64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {{
      bytes[i] = binary.charCodeAt(i);
    }}
    const blob = new Blob([bytes], {{ type: "application/zip" }});
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = fileName;
    anchor.style.display = "none";
    document.body.appendChild(anchor);
    anchor.click();
    document.body.removeChild(anchor);
    URL.revokeObjectURL(url);
    dioxus.send("ok");
  }} catch (error) {{
    dioxus.send(`Failed to trigger browser download: ${{String(error)}}`);
  }}
}})();
"#
    );

    let mut eval_result = eval(script.as_str());
    match eval_result.recv().await {
        Ok(serde_json::Value::String(status)) if status == "ok" => Ok(()),
        Ok(serde_json::Value::String(message)) => Err(message),
        Ok(other) => Err(format!("Unexpected download response: {other}")),
        Err(err) => Err(format!("Failed to receive download result: {err}")),
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct WalletDataImportFileBridgeOk {
    file_name: String,
    payload_base64: String,
}

async fn load_wallet_data_import_file() -> Result<ImportWalletDataRequest, String> {
    let encoded_input_id = serde_json::to_string(WALLET_DATA_IMPORT_FILE_ID)
        .map_err(|err| format!("Failed to encode import input id: {err}"))?;
    let encoded_max_bytes = serde_json::to_string(&MAX_IMPORT_PAYLOAD_BYTES)
        .map_err(|err| format!("Failed to encode import size limit: {err}"))?;

    let script = format!(
        r#"
(() => {{
  try {{
    if (typeof window === "undefined" || typeof document === "undefined") {{
      dioxus.send(JSON.stringify({{ kind: "error", message: "Import is only available in browser builds." }}));
      return;
    }}

    const inputId = {encoded_input_id};
    const maxBytes = {encoded_max_bytes};
    const input = document.getElementById(inputId);
    if (!input || !input.files || input.files.length === 0) {{
      dioxus.send(JSON.stringify({{ kind: "error", message: "Select a .zip or .json wallet-data file first." }}));
      return;
    }}

    const file = input.files[0];
    const lowerName = String(file.name || "").toLowerCase();
    if (!lowerName.endsWith(".zip") && !lowerName.endsWith(".json")) {{
      dioxus.send(JSON.stringify({{ kind: "error", message: "Wallet-data import requires a .zip or .json file." }}));
      return;
    }}
    if (file.size > maxBytes) {{
      dioxus.send(JSON.stringify({{ kind: "error", message: `File is too large (${{file.size}} bytes). Maximum is ${{maxBytes}} bytes.` }}));
      return;
    }}

    file.arrayBuffer().then((buffer) => {{
      const bytes = new Uint8Array(buffer);
      let binary = "";
      for (let i = 0; i < bytes.length; i += 1) {{
        binary += String.fromCharCode(bytes[i]);
      }}
      dioxus.send(JSON.stringify({{
        kind: "ok",
        file_name: file.name,
        payload_base64: btoa(binary)
      }}));
    }}).catch((error) => {{
      dioxus.send(JSON.stringify({{ kind: "error", message: `Failed to read selected file: ${{String(error)}}` }}));
    }});
  }} catch (error) {{
    dioxus.send(JSON.stringify({{ kind: "error", message: `Failed to read selected file: ${{String(error)}}` }}));
  }}
}})();
"#
    );

    let mut eval_result = eval(script.as_str());
    let raw_message: serde_json::Value = eval_result
        .recv()
        .await
        .map_err(|err| format!("Failed to receive selected file result: {err}"))?;
    let message_text = raw_message
        .as_str()
        .ok_or_else(|| format!("Unexpected selected file bridge response: {raw_message}"))?;

    let parsed = serde_json::from_str::<serde_json::Value>(message_text)
        .map_err(|err| format!("Failed to parse selected file bridge payload: {err}"))?;
    let kind = parsed
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Selected file bridge response is missing kind".to_string())?;

    match kind {
        "ok" => {
            let ok: WalletDataImportFileBridgeOk =
                serde_json::from_value(parsed).map_err(|err| {
                    format!("Failed to decode selected file bridge success payload: {err}")
                })?;
            Ok(ImportWalletDataRequest {
                file_name: ok.file_name,
                payload_base64: ok.payload_base64,
                password: None,
            })
        }
        "error" => {
            let message = parsed
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Failed to read selected import file.");
            Err(message.to_string())
        }
        other => Err(format!("Unexpected selected file bridge kind: {other}")),
    }
}

fn plural_count(count: u32, singular: &str, plural: &str) -> Option<String> {
    if count == 0 {
        None
    } else if count == 1 {
        Some(format!("1 {singular}"))
    } else {
        Some(format!("{count} {plural}"))
    }
}

fn wallet_data_count_parts(counts: &WalletDataExportCounts) -> Vec<String> {
    let mut parts = Vec::new();
    parts.extend(plural_count(counts.wallets, "wallet", "wallets"));
    parts.extend(plural_count(
        counts.native_accounts,
        "native account",
        "native accounts",
    ));
    parts.extend(plural_count(counts.addresses, "address", "addresses"));
    parts.extend(plural_count(
        counts.custom_accounts,
        "custom account",
        "custom accounts",
    ));
    parts.extend(plural_count(
        counts.balance_assertions,
        "balance assertion",
        "balance assertions",
    ));
    parts.extend(plural_count(counts.api_keys, "api key", "api keys"));
    parts
}

fn wallet_data_counts_are_empty(counts: &WalletDataExportCounts) -> bool {
    counts.wallets == 0
        && counts.native_accounts == 0
        && counts.addresses == 0
        && counts.custom_accounts == 0
        && counts.balance_assertions == 0
        && counts.api_keys == 0
}

fn wallet_data_summary_text(summary: &WalletDataExportSummary) -> String {
    let counts = WalletDataExportCounts {
        wallets: summary.wallets,
        native_accounts: summary.native_accounts,
        addresses: summary.addresses,
        custom_accounts: summary.custom_accounts,
        balance_assertions: summary.balance_assertions,
        api_keys: summary.api_keys,
    };
    let parts = wallet_data_count_parts(&counts);
    let mut text = if parts.is_empty() {
        "Exported no wallet data.".to_string()
    } else {
        format!("Exported {}.", parts.join(", "))
    };
    if summary.premium_transfer_exported {
        text.push_str(" Subscription transfer data was included.");
    }
    if summary.encrypted {
        text.push_str(" Exported as an encrypted ZIP.");
    } else {
        text.push_str(" Exported as an unencrypted ZIP.");
    }
    text
}

fn wallet_data_pre_export_text(counts: &WalletDataExportCounts, encrypted: bool) -> String {
    let parts = wallet_data_count_parts(counts);
    if parts.is_empty() {
        "This export will include nothing yet - add a wallet first.".to_string()
    } else if encrypted {
        format!(
            "This export will include {}. It will be exported as an encrypted ZIP.",
            parts.join(", ")
        )
    } else {
        format!(
            "This export will include {}. It will be exported as an unencrypted ZIP.",
            parts.join(", ")
        )
    }
}

fn describe_wallet_data_import_api_key_notice(api_keys_count: u32) -> String {
    let count = if api_keys_count == 1 {
        "1 API key".to_string()
    } else {
        format!("{api_keys_count} API keys")
    };
    format!(
        "This backup contains {count}. Missing providers will be imported; existing API keys on this device will not be overwritten."
    )
}

fn premium_transfer_result_text(result: &PremiumTransferResultView) -> String {
    let fallback = match result.status {
        PremiumTransferStatusView::Active => "Subscription moved to this local user.".to_string(),
        PremiumTransferStatusView::RetryableFailure => {
            "Subscription transfer could not complete yet. Retry later.".to_string()
        }
        PremiumTransferStatusView::NonRetryableFailure => {
            "Subscription transfer could not be completed.".to_string()
        }
    };
    let mut text = result.message.clone().unwrap_or(fallback);
    if let Some(paid_through) = result.paid_through {
        text.push_str(&format!(" Paid through {paid_through}."));
    }
    if let Some(offline_access_until) = result.offline_access_until {
        text.push_str(&format!(" Offline access until {offline_access_until}."));
    }
    text
}

async fn describe_import_request(
    request: &ImportWalletDataRequest,
) -> Result<WalletDataImportDescription, ExportError> {
    describe_wallet_data_import(DescribeWalletDataImportRequest {
        file_name: request.file_name.clone(),
        payload_base64: request.payload_base64.clone(),
        password: request.password.clone(),
    })
    .await
}

fn apply_import_password(request: &mut ImportWalletDataRequest, password: String) {
    request.password = if password.is_empty() {
        None
    } else {
        Some(password)
    };
}

#[derive(Clone, Copy)]
struct ImportDescriptionUiState {
    import_file_request: Signal<Option<ImportWalletDataRequest>>,
    import_description: Signal<Option<WalletDataImportDescription>>,
    is_describing: Signal<bool>,
    import_error_message: Signal<Option<String>>,
    import_password_error_message: Signal<Option<String>>,
    auth_state: AuthState,
    banner_state: BannerState,
}

fn start_import_description(
    mut request: ImportWalletDataRequest,
    password: String,
    ui: ImportDescriptionUiState,
) {
    apply_import_password(&mut request, password);
    let mut import_file_request = ui.import_file_request;
    let mut import_description = ui.import_description;
    let mut is_describing = ui.is_describing;
    import_file_request.set(Some(request.clone()));
    import_description.set(None);
    is_describing.set(true);

    spawn(async move {
        let mut import_description = ui.import_description;
        let mut import_error_message = ui.import_error_message;
        let mut import_password_error_message = ui.import_password_error_message;
        let mut is_describing = ui.is_describing;
        match describe_import_request(&request).await {
            Ok(description) => {
                import_description.set(Some(description));
            }
            Err(ExportError::Unauthorized(message)) => {
                handle_session_expired(ui.auth_state, ui.banner_state);
                import_error_message.set(Some(message));
            }
            Err(ExportError::PasswordRequired(message))
            | Err(ExportError::EncryptedZipAuthFailed(message)) => {
                import_password_error_message.set(Some(message));
            }
            Err(err) => {
                import_error_message.set(Some(err.to_string()));
            }
        }
        is_describing.set(false);
    });
}

async fn download_hledger_zip(encrypted: bool, password: Option<&str>) -> Result<String, String> {
    let payload = serde_json::to_string(&serde_json::json!({
        "encrypted": encrypted,
        "password": password,
    }))
    .map_err(|err| format!("Failed to encode hledger download request: {err}"))?;
    let encoded_payload = serde_json::to_string(&payload)
        .map_err(|err| format!("Failed to encode hledger download request payload: {err}"))?;

    let script = format!(
        r#"
(async () => {{
  try {{
    if (typeof window === "undefined" || typeof document === "undefined") {{
      dioxus.send(JSON.stringify({{ kind: "error", message: "Download is only available in browser builds." }}));
      return;
    }}
    const requestBody = {encoded_payload};
    const response = await fetch("/_app/user/exports/hledger/download", {{
      method: "POST",
      credentials: "same-origin",
      headers: {{ "Content-Type": "application/json", "Accept": "application/zip" }},
      body: requestBody,
    }});
    if (!response.ok) {{
      let message = `Download failed (HTTP ${{response.status}})`;
      try {{
        const errorBody = await response.json();
        if (errorBody && errorBody.message) {{
          message = errorBody.message;
        }}
      }} catch (_) {{}}
      dioxus.send(JSON.stringify({{ kind: "error", status: response.status, message }}));
      return;
    }}
    const disposition = response.headers.get("Content-Disposition") || "";
    let fileName = "bitgarth-hledger.zip";
    const filenameMatch = disposition.match(/filename="?([^";]+)"?/i);
    if (filenameMatch && filenameMatch[1]) {{
      fileName = filenameMatch[1];
    }}
    const blob = await response.blob();
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = fileName;
    anchor.style.display = "none";
    document.body.appendChild(anchor);
    anchor.click();
    document.body.removeChild(anchor);
    URL.revokeObjectURL(url);
    dioxus.send(JSON.stringify({{ kind: "ok", file_name: fileName }}));
  }} catch (error) {{
    dioxus.send(JSON.stringify({{ kind: "error", message: `Failed to download hledger archive: ${{String(error)}}` }}));
  }}
}})();
"#
    );

    let mut eval_result = eval(script.as_str());
    let raw_message: serde_json::Value = eval_result
        .recv()
        .await
        .map_err(|err| format!("Failed to receive hledger download result: {err}"))?;
    let message_text = raw_message
        .as_str()
        .ok_or_else(|| format!("Unexpected hledger download bridge response: {raw_message}"))?;
    let parsed: serde_json::Value = serde_json::from_str(message_text)
        .map_err(|err| format!("Failed to parse hledger download bridge payload: {err}"))?;
    let kind = parsed
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Hledger download bridge response is missing kind".to_string())?;
    match kind {
        "ok" => Ok(parsed
            .get("file_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("hledger.zip")
            .to_string()),
        "error" => {
            let status = parsed.get("status").and_then(serde_json::Value::as_i64);
            let message = parsed
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Failed to download hledger archive.");
            if status == Some(401) {
                Err(format!("__unauthorized__:{message}"))
            } else {
                Err(message.to_string())
            }
        }
        other => Err(format!("Unexpected hledger download bridge kind: {other}")),
    }
}

#[component]
pub fn HledgerExport() -> Element {
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();

    let mut is_downloading = use_signal(|| false);
    let mut last_downloaded_file = use_signal(|| None::<String>);
    let mut error_message = use_signal(|| None::<String>);
    let mut encrypted_download = use_signal(|| true);
    let export_password = use_signal(String::new);
    let export_password_confirm = use_signal(String::new);
    let mut prefix_settings_synced = use_signal(|| false);
    let mut hledger_account_prefix_input = use_signal(String::new);
    let mut hledger_default_account_prefix = use_signal(String::new);
    let mut hledger_account_prefix_error = use_signal(|| None::<String>);
    let mut hledger_account_prefix_status = use_signal(|| None::<String>);
    let hledger_account_prefix_saving = use_signal(|| false);

    let settings_resource =
        use_server_future(move || async move { get_hledger_export_settings().await })?;
    let settings_value = settings_resource.value();
    match settings_value.read().clone() {
        Some(Ok(settings)) if !*prefix_settings_synced.peek() => {
            hledger_account_prefix_input.set(
                settings
                    .hledger_account_prefix
                    .as_ref()
                    .map(|prefix| prefix.as_str().to_string())
                    .unwrap_or_default(),
            );
            hledger_default_account_prefix.set(settings.hledger_default_account_prefix);
            hledger_account_prefix_error.set(None);
            hledger_account_prefix_status.set(None);
            prefix_settings_synced.set(true);
        }
        Some(Err(err)) if err.is_unauthorized() && !*prefix_settings_synced.peek() => {
            handle_session_expired(auth_state, banner_state);
            hledger_account_prefix_error.set(Some(err.to_string()));
            prefix_settings_synced.set(true);
        }
        Some(Err(err)) if !*prefix_settings_synced.peek() => {
            hledger_account_prefix_error.set(Some(err.to_string()));
            prefix_settings_synced.set(true);
        }
        _ => {}
    }

    let download_button_disabled = is_downloading()
        || (encrypted_download()
            && (export_password().is_empty() || export_password() != export_password_confirm()));
    let export_password_is_weak = encrypted_download() && export_password().len() < 16;

    let save_hledger_account_prefix_setting = move |_| {
        if !begin_submit(hledger_account_prefix_saving) {
            return;
        }

        hledger_account_prefix_error.set(None);
        hledger_account_prefix_status.set(None);
        let candidate = hledger_account_prefix_input();

        spawn(async move {
            let result = save_hledger_account_prefix(Some(candidate)).await;
            finish_submit(hledger_account_prefix_saving);

            match result {
                Ok(saved) => {
                    hledger_account_prefix_input.set(
                        saved
                            .as_ref()
                            .map(|prefix| prefix.as_str().to_string())
                            .unwrap_or_default(),
                    );
                    if saved.is_some() {
                        hledger_account_prefix_status
                            .set(Some("Asset account prefix saved.".to_string()));
                    } else {
                        hledger_account_prefix_status
                            .set(Some("Using the default asset account prefix.".to_string()));
                    }
                }
                Err(err) if err.is_unauthorized() => {
                    handle_session_expired(auth_state, banner_state);
                    hledger_account_prefix_error.set(Some(err.to_string()));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["hledger_account_prefix"],
                        "Invalid hledger account prefix.",
                    );
                    hledger_account_prefix_error.set(Some(message));
                }
                Err(other) => {
                    hledger_account_prefix_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let clear_hledger_account_prefix_setting = move |_| {
        if !begin_submit(hledger_account_prefix_saving) {
            return;
        }

        hledger_account_prefix_error.set(None);
        hledger_account_prefix_status.set(None);

        spawn(async move {
            let result = save_hledger_account_prefix(None).await;
            finish_submit(hledger_account_prefix_saving);

            match result {
                Ok(saved) => {
                    hledger_account_prefix_input.set(String::new());
                    hledger_account_prefix_status
                        .set(Some("Using the default asset account prefix.".to_string()));
                    let _ = saved;
                }
                Err(err) if err.is_unauthorized() => {
                    handle_session_expired(auth_state, banner_state);
                    hledger_account_prefix_error.set(Some(err.to_string()));
                }
                Err(err) if is_form_field_error(&err) => {
                    let message = primary_field_or_fallback(
                        &err,
                        &["hledger_account_prefix"],
                        "Invalid hledger account prefix.",
                    );
                    hledger_account_prefix_error.set(Some(message));
                }
                Err(other) => {
                    hledger_account_prefix_error.set(Some(other.to_string()));
                }
            }
        });
    };

    let start_download = move |_| {
        if is_downloading() {
            return;
        }

        is_downloading.set(true);
        error_message.set(None);
        last_downloaded_file.set(None);

        let encrypted = encrypted_download();
        let password = if encrypted {
            Some(export_password())
        } else {
            None
        };
        let auth_state = auth_state;
        let banner_state = banner_state;
        let mut is_downloading = is_downloading;
        let mut last_downloaded_file = last_downloaded_file;
        let mut error_message = error_message;

        spawn(async move {
            match download_hledger_zip(encrypted, password.as_deref()).await {
                Ok(file_name) => {
                    last_downloaded_file.set(Some(file_name));
                }
                Err(err) => {
                    if let Some(rest) = err.strip_prefix("__unauthorized__:") {
                        handle_session_expired(auth_state, banner_state);
                        error_message.set(Some(rest.to_string()));
                    } else {
                        error_message.set(Some(err));
                    }
                }
            }
            is_downloading.set(false);
        });
    };

    let is_authenticated = matches!(&*auth_state.read(), AuthStatus::Authenticated(_));
    rsx! {
        div { class: "page-container",
            div { class: "page-header",
                h1 { class: "page-title page-title-display", "Accounting Export" }
                p { class: "page-subtitle",
                    "Your confirmed on-chain transactions, as plain-text double-entry journals — readable by hledger, ledger-cli, and a text editor."
                }
            }

            SectionHead {
                num: "01".to_string(),
                title: "Plain text, kept plain".to_string(),
                emphasis: Some("kept plain".to_string()),
            }
            div { class: "card",
                div { class: "card-body",
                    p {
                        "The "
                        a {
                            href: "https://hledger.org",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            "hledger"
                        }
                        " and "
                        a {
                            href: "https://ledger-cli.org",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            "ledger-cli"
                        }
                        " tools use a plain-text, double-entry bookkeeping format. "
                        "Each transaction records both sides of a money movement, so balances stay auditable and errors are easy to spot."
                    }
                    p { class: "mt-sm",
                        "The files are human-readable — open and edit them in any text editor. "
                        "The format works well for tracking capital gains, cost basis, and preparing data for tax reporting."
                    }
                    div { class: "code-card", "data-label": "journal",
                        pre {
                            "2026-04-18 * Received BTC\n"
                            "    assets:MyUser:MyWallet:BitcoinAccount1     "
                            span { class: "tok-img", "0.3 BTC = 0.3 BTC" }
                            "\n    income:unknown                            "
                            span { class: "tok-img", "-0.3 BTC" }
                            "\n\n"
                            "2026-04-21 * Sent BTC\n"
                            "    assets:MyUser:MyWallet:BitcoinAccount1    "
                            span { class: "tok-img", "-0.1 BTC = 0.2 BTC" }
                            "\n    expenses:unknown                           "
                            span { class: "tok-img", "0.1 BTC" }
                        }
                    }
                }
            }

            SectionHead {
                num: "02".to_string(),
                title: "Account names".to_string(),
                emphasis: Some("names".to_string()),
            }
            div { class: "card",
                div { class: "card-body",
                    if is_authenticated {
                        div { class: "form-group",
                            label {
                                class: "form-label",
                                r#for: "hledger-account-prefix",
                                "Asset account prefix"
                            }
                            input {
                                id: "hledger-account-prefix",
                                class: "form-input",
                                "data-testid": "hledger-account-prefix-input",
                                value: "{hledger_account_prefix_input}",
                                disabled: hledger_account_prefix_saving(),
                                placeholder: "{hledger_default_account_prefix}",
                                oninput: move |evt| {
                                    hledger_account_prefix_input.set(evt.value());
                                    hledger_account_prefix_error.set(None);
                                    hledger_account_prefix_status.set(None);
                                },
                            }
                            p { class: "form-hint mt-sm",
                                "This prefix starts every asset account. Equity, transfer, and other hledger accounts are named to match it."
                            }
                        }
                        div { class: "form-actions mt-md",
                            button {
                                class: "btn btn-primary",
                                "data-testid": "hledger-account-prefix-save",
                                disabled: hledger_account_prefix_saving(),
                                onclick: save_hledger_account_prefix_setting,
                                if hledger_account_prefix_saving() {
                                    "Saving..."
                                } else {
                                    "Save prefix"
                                }
                            }
                            button {
                                class: "btn btn-secondary",
                                "data-testid": "hledger-account-prefix-clear",
                                disabled: hledger_account_prefix_saving(),
                                onclick: clear_hledger_account_prefix_setting,
                                "Use default"
                            }
                        }
                        if let Some(status) = hledger_account_prefix_status() {
                            p { class: "settings-status-success", "{status}" }
                        }
                        if let Some(error) = hledger_account_prefix_error() {
                            p { class: "settings-status-error", "{error}" }
                        }
                    } else {
                        p {
                            "You need to be logged in to edit hledger account names."
                            " "
                            Link { to: Route::Login, "Go to login" }
                        }
                    }
                }
            }

            SectionHead {
                num: "03".to_string(),
                title: "Download your journal".to_string(),
                emphasis: Some("your journal".to_string()),
            }
            div { class: "card",
                div { class: "card-body",
                    div { class: "form-group",
                        label { class: "checkbox",
                            input {
                                "data-testid": "hledger-export-encrypted-checkbox",
                                r#type: "checkbox",
                                checked: encrypted_download(),
                                disabled: is_downloading(),
                                onchange: move |_| {
                                    encrypted_download.set(!encrypted_download());
                                },
                            }
                            " Encrypted"
                        }
                        if encrypted_download() {
                            p { class: "form-hint mt-sm",
                                "Format: AES-256 encrypted ZIP. Decrypt outside BitGarth with 7-Zip, Keka, or 7z x file.zip."
                            }
                            div { class: "form-group mt-md",
                                label { class: "form-label", r#for: "hledger-export-password", "Password" }
                                PasswordInput {
                                    id: "hledger-export-password".to_string(),
                                    value: export_password,
                                    placeholder: "Enter encryption password".to_string(),
                                    autocomplete: "new-password",
                                }
                                if export_password_is_weak {
                                    p {
                                        class: "form-hint",
                                        "data-testid": "hledger-export-password-guidance",
                                        "Use a long random password or passphrase. Short, common passwords are easier to crack if the file leaks."
                                    }
                                }
                            }
                            div { class: "form-group mt-md",
                                label { class: "form-label", r#for: "hledger-export-confirm-password", "Confirm password" }
                                PasswordInput {
                                    id: "hledger-export-confirm-password".to_string(),
                                    value: export_password_confirm,
                                    placeholder: "Confirm encryption password".to_string(),
                                    autocomplete: "new-password",
                                }
                                if !export_password_confirm().is_empty() && export_password() != export_password_confirm() {
                                    p { class: "form-error", "Passwords do not match." }
                                }
                            }
                            p { class: "form-hint mt-sm",
                                "The server uses this password for this download only and does not store it. If you forget it, the encrypted file cannot be recovered — and that is the point."
                            }
                        } else {
                            p {
                                class: "form-hint mt-sm",
                                "data-testid": "hledger-export-unencrypted-warning",
                                "This ZIP is not encrypted. It holds your journals as plain text — store it somewhere only you can reach."
                            }
                        }
                    }

                    if is_authenticated {
                        button {
                            class: "btn btn-primary mt-md",
                            "data-testid": "hledger-export-button",
                            disabled: download_button_disabled,
                            onmounted: move |e| async move {
                                let _ = e.set_focus(true).await;
                            },
                            onclick: start_download,
                            if is_downloading() {
                                "Downloading..."
                            } else {
                                "Download journal"
                            }
                        }
                    } else {
                        p { class: "mt-md",
                            "You need to be logged in to run exports."
                            " "
                            Link { to: Route::Login, "Go to login" }
                        }
                    }

                    if let Some(message) = error_message() {
                        p { class: "mt-md", style: "color: var(--color-error);", "{message}" }
                    }

                    if let Some(file_name) = last_downloaded_file() {
                        p { class: "mt-md",
                            "Downloaded "
                            code { "{file_name}" }
                            " ("
                            if encrypted_download() { "encrypted" } else { "unencrypted" }
                            ")."
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn WalletDataExport() -> Element {
    let auth_state = use_context::<AuthState>();
    let banner_state = use_context::<BannerState>();

    let mut is_exporting = use_signal(|| false);
    let export_success = use_signal(|| None::<WalletDataExportSummary>);
    let mut export_error_message = use_signal(|| None::<String>);
    let mut include_premium_transfer = use_signal(|| false);
    let mut encrypted_export = use_signal(|| true);
    let export_password = use_signal(String::new);
    let export_password_confirm = use_signal(String::new);
    let mut is_importing = use_signal(|| false);
    let mut import_success = use_signal(|| None::<ImportResultView>);
    let mut import_error_message = use_signal(|| None::<String>);
    let mut import_password_error_message = use_signal(|| None::<String>);
    let mut import_file_selected = use_signal(|| false);
    let mut import_file_request = use_signal(|| None::<ImportWalletDataRequest>);
    let mut import_description = use_signal(|| None::<WalletDataImportDescription>);
    let is_describing = use_signal(|| false);
    let import_password = use_signal(String::new);
    let mut is_transferring_premium = use_signal(|| false);
    let mut premium_transfer_result = use_signal(|| None::<PremiumTransferResultView>);
    let mut premium_transfer_error_message = use_signal(|| None::<String>);
    let export_options_resource =
        use_server_future(move || async move { get_wallet_data_export_options().await })?;

    let export_options = export_options_resource
        .value()
        .read()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .cloned();
    let premium_transfer_available = export_options
        .as_ref()
        .map(|options| options.premium_transfer_available)
        .unwrap_or(false);
    let export_counts_empty = export_options
        .as_ref()
        .map(|options| wallet_data_counts_are_empty(&options.counts))
        .unwrap_or(false);
    let export_button_disabled = is_exporting()
        || export_counts_empty
        || (encrypted_export()
            && (export_password().is_empty() || export_password() != export_password_confirm()));
    let export_password_is_weak = encrypted_export() && export_password().len() < 16;

    let start_export = move |_| {
        if is_exporting() {
            return;
        }

        is_exporting.set(true);
        export_error_message.set(None);

        let mut is_exporting = is_exporting;
        let mut export_success = export_success;
        let mut export_error_message = export_error_message;
        let include_premium_transfer_value = include_premium_transfer();
        let encrypted_export_value = encrypted_export();
        let export_password_value = if encrypted_export_value {
            Some(export_password())
        } else {
            None
        };
        let auth_state = auth_state;
        let banner_state = banner_state;

        spawn(async move {
            match export_wallet_data(ExportWalletDataRequest {
                include_premium_transfer: include_premium_transfer_value,
                encrypted: encrypted_export_value,
                password: export_password_value,
            })
            .await
            {
                Ok(WalletDataExportDownloadView {
                    file_name,
                    zip_base64,
                    summary,
                }) => {
                    if let Err(err) = download_wallet_data_file(&file_name, &zip_base64).await {
                        export_error_message.set(Some(err));
                    }

                    export_success.set(Some(summary));
                }
                Err(ExportError::Unauthorized(message)) => {
                    handle_session_expired(auth_state, banner_state);
                    export_error_message.set(Some(message));
                }
                Err(err) => {
                    export_error_message.set(Some(err.to_string()));
                }
            }

            is_exporting.set(false);
        });
    };

    let start_import = move |_| {
        if is_importing() {
            return;
        }

        is_importing.set(true);
        import_error_message.set(None);
        import_password_error_message.set(None);
        premium_transfer_result.set(None);
        premium_transfer_error_message.set(None);

        let mut is_importing = is_importing;
        let mut import_success = import_success;
        let mut import_error_message = import_error_message;
        let mut import_password_error_message = import_password_error_message;
        let import_password_value = import_password();
        let import_file_request_value = import_file_request();
        let auth_state = auth_state;
        let banner_state = banner_state;

        spawn(async move {
            let Some(mut request) = import_file_request_value else {
                import_error_message.set(Some(
                    "Select a .zip or .json wallet-data file first.".to_string(),
                ));
                is_importing.set(false);
                return;
            };
            apply_import_password(&mut request, import_password_value);

            match import_wallet_data(request).await {
                Ok(result) => {
                    import_success.set(Some(result));
                }
                Err(ExportError::Unauthorized(message)) => {
                    handle_session_expired(auth_state, banner_state);
                    import_error_message.set(Some(message));
                }
                Err(ExportError::PasswordRequired(message))
                | Err(ExportError::EncryptedZipAuthFailed(message)) => {
                    import_password_error_message.set(Some(message));
                }
                Err(err) => {
                    import_error_message.set(Some(err.to_string()));
                }
            }

            is_importing.set(false);
        });
    };

    let start_premium_transfer = move |_| {
        if is_transferring_premium() {
            return;
        }

        let Some(pending_transfer_id) =
            import_success().and_then(|result| result.pending_premium_transfer_id)
        else {
            return;
        };

        is_transferring_premium.set(true);
        premium_transfer_result.set(None);
        premium_transfer_error_message.set(None);

        let mut is_transferring_premium = is_transferring_premium;
        let mut premium_transfer_result = premium_transfer_result;
        let mut premium_transfer_error_message = premium_transfer_error_message;
        let mut import_success = import_success;
        let auth_state = auth_state;
        let banner_state = banner_state;

        spawn(async move {
            match confirm_premium_transfer(ConfirmPremiumTransferRequest {
                pending_transfer_id,
            })
            .await
            {
                Ok(result) => {
                    if matches!(
                        result.status,
                        PremiumTransferStatusView::Active
                            | PremiumTransferStatusView::NonRetryableFailure
                    ) && let Some(mut import_result) = import_success()
                    {
                        import_result.pending_premium_transfer_id = None;
                        import_result.premium_transfer_status =
                            PremiumTransferImportStatusView::NotPresent;
                        import_success.set(Some(import_result));
                    }
                    premium_transfer_result.set(Some(result));
                }
                Err(ExportError::Unauthorized(message)) => {
                    handle_session_expired(auth_state, banner_state);
                    premium_transfer_error_message.set(Some(message));
                }
                Err(err) => {
                    premium_transfer_error_message.set(Some(err.to_string()));
                }
            }

            is_transferring_premium.set(false);
        });
    };

    let is_authenticated = matches!(&*auth_state.read(), AuthStatus::Authenticated(_));

    rsx! {
        div { class: "page-container",
            div { class: "page-header",
                h1 { class: "page-title page-title-display", "Backup & Restore" }
                p { class: "page-subtitle",
                    "Back up your wallet configuration — public keys, addresses, account labels. This is not your transaction history; transactions re-sync after a restore."
                }
            }

            HostedRetentionNotice {}

            SectionHead {
                num: "01".to_string(),
                title: "Back up your wallets".to_string(),
                emphasis: Some("your wallets".to_string()),
            }
            div { class: "card",
                div { class: "card-body",
                    if let Some(options) = export_options.as_ref() {
                        p {
                            class: "form-hint",
                            "data-testid": "wallet-data-pre-export-summary",
                            "{wallet_data_pre_export_text(&options.counts, encrypted_export())}"
                        }
                    }
                    div { class: "form-group",
                        label { class: "checkbox",
                            input {
                                "data-testid": "wallet-data-subscription-transfer-checkbox",
                                r#type: "checkbox",
                                checked: include_premium_transfer(),
                                disabled: !premium_transfer_available || is_exporting(),
                                onchange: move |_| {
                                    include_premium_transfer.set(!include_premium_transfer());
                                },
                            }
                            " Include subscription transfer data"
                        }
                        if premium_transfer_available {
                            p {
                                class: "mt-sm",
                                "data-testid": "wallet-data-subscription-transfer-warning",
                                "{WALLET_DATA_SUBSCRIPTION_TRANSFER_NOTICE}"
                            }
                        } else {
                            p { class: "mt-sm",
                                "Subscription transfer data is not available for this user yet."
                            }
                        }
                    }
                    div { class: "form-group mt-md",
                        label { class: "checkbox",
                            input {
                                "data-testid": "wallet-data-encrypted-checkbox",
                                r#type: "checkbox",
                                checked: encrypted_export(),
                                disabled: is_exporting(),
                                onchange: move |_| {
                                    encrypted_export.set(!encrypted_export());
                                },
                            }
                            " Encrypted"
                        }
                        if encrypted_export() {
                            p { class: "form-hint mt-sm",
                                "Format: AES-256 encrypted ZIP. Decrypt outside BitGarth with 7-Zip, Keka, or 7z x file.zip."
                            }
                            div { class: "form-group mt-md",
                                label { class: "form-label", r#for: "wallet-data-export-password", "Password" }
                                PasswordInput {
                                    id: "wallet-data-export-password".to_string(),
                                    value: export_password,
                                    placeholder: "Enter encryption password".to_string(),
                                    autocomplete: "new-password",
                                }
                                if export_password_is_weak {
                                    p {
                                        class: "form-hint",
                                        "data-testid": "wallet-data-export-password-guidance",
                                        "Use a long random password or passphrase. Short, common passwords are easier to crack if the file leaks."
                                    }
                                }
                            }
                            div { class: "form-group mt-md",
                                label { class: "form-label", r#for: "wallet-data-export-confirm-password", "Confirm password" }
                                PasswordInput {
                                    id: "wallet-data-export-confirm-password".to_string(),
                                    value: export_password_confirm,
                                    placeholder: "Confirm encryption password".to_string(),
                                    autocomplete: "new-password",
                                }
                                if !export_password_confirm().is_empty() && export_password() != export_password_confirm() {
                                    p { class: "form-error", "Passwords do not match." }
                                }
                            }
                            p { class: "form-hint mt-sm",
                                "The server uses this password for this backup only and does not store it. If you forget it, the encrypted file cannot be recovered."
                            }
                        } else {
                            p {
                                class: "form-hint mt-sm",
                                "data-testid": "wallet-data-unencrypted-warning",
                                "This ZIP is not encrypted. It holds your wallet backup data and API keys as plain text — store it somewhere only you can reach."
                            }
                            p { class: "form-hint mt-sm",
                                "Format: unencrypted ZIP containing wallet-data JSON. Extract it with any ZIP tool to inspect the JSON."
                            }
                        }
                    }

                    if is_authenticated {
                        button {
                            class: "btn btn-primary mt-md",
                            "data-testid": "wallet-data-export-button",
                            disabled: export_button_disabled,
                            onmounted: move |e| async move {
                                let _ = e.set_focus(true).await;
                            },
                            onclick: start_export,
                            if is_exporting() {
                                "Creating backup..."
                            } else {
                                "Create backup"
                            }
                        }
                    } else {
                        p { class: "mt-md",
                            "You need to be logged in to create a backup."
                            " "
                            Link { to: Route::Login, "Go to login" }
                        }
                    }

                    if let Some(message) = export_error_message() {
                        p { class: "mt-md", style: "color: var(--color-error);", "{message}" }
                    }

                    if let Some(summary) = export_success() {
                        p { class: "mt-md", "{wallet_data_summary_text(&summary)}" }
                    }
                }
            }

            SectionHead {
                num: "02".to_string(),
                title: "Restore from a backup".to_string(),
                emphasis: Some("from a backup".to_string()),
            }
            div { class: "card",
                div { class: "card-body",
                    p {
                        "Restore a BitGarth backup ZIP. Duplicate identifiers are skipped and reported below."
                    }

                    if is_authenticated {
                        div { class: "form-group mt-md",
                            label { class: "form-label", r#for: "{WALLET_DATA_IMPORT_FILE_ID}", "Backup file (.zip or .json)" }
                            input {
                                id: "{WALLET_DATA_IMPORT_FILE_ID}",
                                class: "form-input",
                                "data-testid": "wallet-data-import-file",
                                r#type: "file",
                                accept: ".zip,.json,application/zip,application/json",
                                onchange: move |_| {
                                    import_file_selected.set(false);
                                    import_file_request.set(None);
                                    import_description.set(None);
                                    import_success.set(None);
                                    import_error_message.set(None);
                                    import_password_error_message.set(None);
                                    let mut import_file_selected = import_file_selected;
                                    let import_password_value = import_password();
                                    let ui = ImportDescriptionUiState {
                                        import_file_request,
                                        import_description,
                                        is_describing,
                                        import_error_message,
                                        import_password_error_message,
                                        auth_state,
                                        banner_state,
                                    };
                                    let mut import_error_message = import_error_message;

                                    spawn(async move {
                                        match load_wallet_data_import_file().await {
                                            Ok(request) => {
                                                import_file_selected.set(true);
                                                start_import_description(
                                                    request,
                                                    import_password_value,
                                                    ui,
                                                );
                                            }
                                            Err(err) => {
                                                import_error_message.set(Some(err));
                                            }
                                        }
                                    });
                                },
                            }
                        }
                        if import_file_selected() {
                            div { class: "form-group mt-md",
                                label { class: "form-label", r#for: "wallet-data-import-password", "Password (if encrypted)" }
                                PasswordInput {
                                    id: "wallet-data-import-password".to_string(),
                                    value: import_password,
                                    placeholder: "Enter decryption password".to_string(),
                                    autocomplete: "current-password",
                                    on_change: move |new_value| {
                                        import_password_error_message.set(None);
                                        import_error_message.set(None);
                                        if let Some(request) = import_file_request() {
                                            let ui = ImportDescriptionUiState {
                                                import_file_request,
                                                import_description,
                                                is_describing,
                                                import_error_message,
                                                import_password_error_message,
                                                auth_state,
                                                banner_state,
                                            };
                                            start_import_description(
                                                request,
                                                new_value,
                                                ui,
                                            );
                                        }
                                    },
                                }
                                p { class: "form-hint",
                                    "Enter a password if this ZIP was encrypted when it was created. Raw JSON backups cannot be encrypted."
                                }
                                if let Some(message) = import_password_error_message() {
                                    p {
                                        class: "form-error",
                                        "data-testid": "wallet-data-import-password-error",
                                        "{message}"
                                    }
                                }
                            }
                        }
                        if is_describing() {
                            p { class: "form-hint mt-md", "Inspecting backup..." }
                        }
                        if let Some(description) = import_description() {
                            if description.api_keys_count > 0 || description.has_subscription_transfer {
                                div {
                                    class: "mt-md",
                                    "data-testid": "wallet-data-import-notices",
                                    if description.api_keys_count > 0 {
                                        p { class: "form-hint",
                                            "{describe_wallet_data_import_api_key_notice(description.api_keys_count)}"
                                        }
                                    }
                                    if description.has_subscription_transfer {
                                        p { class: "form-hint",
                                            "This backup contains subscription transfer data. After restore, you can choose whether to move the subscription to this device."
                                        }
                                    }
                                }
                            }
                        }

                        button {
                            class: "btn btn-primary mt-md",
                            "data-testid": "wallet-data-import-button",
                            disabled: is_importing() || is_describing() || !import_file_selected(),
                            onclick: start_import,
                            if is_importing() {
                                "Restoring..."
                            } else {
                                "Restore from backup"
                            }
                        }
                    } else {
                        p { class: "mt-md",
                            "You need to be logged in to restore a backup."
                            " "
                            Link { to: Route::Login, "Go to login" }
                        }
                    }

                    if let Some(message) = import_error_message() {
                        p { class: "mt-md", style: "color: var(--color-error);", "{message}" }
                    }

                    if let Some(result) = import_success() {
                        div { class: "mt-md",
                            dl { class: "import-summary",
                                dt { "Created wallets" }
                                dd { "{result.wallets_created.len()}" }
                                dt { "Matched wallets" }
                                dd { "{result.wallets_matched.len()}" }
                                dt { "Created native accounts" }
                                dd { "{result.native_accounts_created.len()}" }
                                dt { "Matched native accounts" }
                                dd { "{result.native_accounts_matched.len()}" }
                                dt { "Skipped duplicates in same account" }
                                dd { "{result.duplicate_skips.len()}" }
                                dt { "Skipped global duplicates" }
                                dd { "{result.global_duplicate_skips.len()}" }
                                dt { "Assertions created" }
                                dd { "{result.assertions_created}" }
                                dt { "Assertions skipped" }
                                dd { "{result.assertions_skipped}" }
                                dt { "API keys imported" }
                                dd { "{result.api_keys_imported}" }
                                dt { "API keys skipped" }
                                dd { "{result.api_keys_skipped_already_present}" }
                            }
                            if !result.validation_warnings.is_empty() {
                                p { class: "mt-sm",
                                    strong { "Validation warnings: " }
                                    "{result.validation_warnings.join(\" | \")}"
                                }
                            }
                            if matches!(
                                result.premium_transfer_status,
                                PremiumTransferImportStatusView::PendingConfirmation
                            ) {
                                p { class: "mt-sm",
                                    "Subscription transfer data found in this backup. Confirm the transfer to move the subscription to this local user."
                                }
                                if let Some(_pending_transfer_id) = result.pending_premium_transfer_id {
                                    button {
                                        class: "btn btn-primary mt-md",
                                        "data-testid": "wallet-data-subscription-transfer-confirm-button",
                                        disabled: is_transferring_premium(),
                                        onclick: start_premium_transfer,
                                        if is_transferring_premium() {
                                            "Moving subscription..."
                                        } else {
                                            "Move subscription here"
                                        }
                                    }
                                }
                            }
                            if matches!(
                                result.premium_transfer_status,
                                PremiumTransferImportStatusView::InvalidMetadata
                            ) {
                                p { class: "mt-sm",
                                    "Subscription transfer data was present, but it was invalid and was not imported."
                                }
                            }
                            if result.sync_triggered {
                                p { class: "mt-sm", "Sync has started for the restored data." }
                            } else {
                                p { class: "mt-sm",
                                    "Restore completed, but automatic sync could not be queued right now."
                                }
                            }
                        }
                    }
                    if let Some(message) = premium_transfer_error_message() {
                        p { class: "mt-md", style: "color: var(--color-error);", "{message}" }
                    }
                    if let Some(result) = premium_transfer_result() {
                        p {
                            class: "mt-md",
                            "data-testid": "wallet-data-subscription-transfer-result",
                            "{premium_transfer_result_text(&result)}"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_data_summary_text_includes_api_keys_and_subscription_copy() {
        let summary = WalletDataExportSummary {
            wallets: 1,
            native_accounts: 2,
            addresses: 3,
            custom_accounts: 4,
            balance_assertions: 5,
            api_keys: 1,
            settings_exported: true,
            premium_transfer_exported: true,
            encrypted: true,
        };

        let text = wallet_data_summary_text(&summary);

        assert!(text.contains("1 api key"));
        assert!(text.contains("Subscription transfer data was included."));
        assert!(!text.contains("Premium transfer data was included."));
    }
}

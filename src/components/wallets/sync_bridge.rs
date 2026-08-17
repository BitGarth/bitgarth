use super::helpers::handle_session_expired;
use super::sync_state::{
    AccountSyncStateSignal, SyncBridgeMessage, SyncRunCompletion,
    apply_account_integration_sync_event, apply_account_sync_event, parse_sync_bridge_message,
};
use crate::backend::get_sync_state;
use crate::transactions::{TransactionSyncEvent, TransactionSyncEventType};
use crate::{AuthState, BannerState};
use chrono::{DateTime, Utc};
use dioxus::document::eval;
use dioxus::prelude::*;

#[cfg(any(test, target_arch = "wasm32", feature = "desktop"))]
pub(super) const SYNC_BRIDGE_CLEANUP_SCRIPT: &str = r#"
const bridge = globalThis.__bitgarthSyncBridge;
if (bridge && typeof bridge.close === "function") {
  bridge.close();
}
"#;

pub(super) const SYNC_BRIDGE_SCRIPT: &str = r#"
const bridgeKey = "__bitgarthSyncBridge";
const previousBridge = globalThis[bridgeKey];
if (previousBridge && typeof previousBridge.close === "function") {
  previousBridge.close();
}
if (typeof EventSource === "undefined") {
  dioxus.send(JSON.stringify({ kind: "stream_unavailable" }));
  return;
}
let active = true;
let source = null;
let intervalId = null;
let stopBridge = () => {};
const stopPromise = new Promise((resolve) => { stopBridge = resolve; });
const cleanup = () => {
  if (!active) return;
  active = false;
  if (source !== null) { source.close(); source = null; }
  if (intervalId !== null) { clearInterval(intervalId); intervalId = null; }
  if (globalThis[bridgeKey] && globalThis[bridgeKey].close === closeBridge) {
    delete globalThis[bridgeKey];
  }
};
const closeBridge = () => { cleanup(); stopBridge(); };
const send = (message) => {
  if (!active) return;
  try { dioxus.send(JSON.stringify(message)); } catch (_) { closeBridge(); }
};
globalThis[bridgeKey] = { close: closeBridge };
source = new EventSource("/_app/user/transactions/sync/events", { withCredentials: true });
source.addEventListener("open", () => send({ kind: "stream_open" }));
source.addEventListener("error", () => send({ kind: "stream_error" }));
const forward = (eventName) => (event) => send({
  kind: "sync_event", event_name: eventName, data: event.data
});
source.addEventListener("sync_snapshot", forward("sync_snapshot"));
source.addEventListener("sync_started", forward("sync_started"));
source.addEventListener("sync_completed", forward("sync_completed"));
source.addEventListener("sync_failed", forward("sync_failed"));
source.addEventListener("account_sync_started", forward("account_sync_started"));
source.addEventListener("account_sync_progress", forward("account_sync_progress"));
source.addEventListener("account_sync_completed", forward("account_sync_completed"));
source.addEventListener("account_sync_failed", forward("account_sync_failed"));
source.addEventListener("account_integration_sync_started", forward("account_integration_sync_started"));
source.addEventListener("account_integration_sync_progress", forward("account_integration_sync_progress"));
source.addEventListener("account_integration_sync_completed", forward("account_integration_sync_completed"));
source.addEventListener("account_integration_sync_failed", forward("account_integration_sync_failed"));
intervalId = setInterval(() => send({ kind: "poll_tick" }), 60000);
await stopPromise;
cleanup();
"#;

#[derive(Clone, Copy)]
pub(crate) struct SyncBridgeSignals {
    pub(crate) account_sync_state: AccountSyncStateSignal,
    pub(crate) account_sync_now: Signal<Option<DateTime<Utc>>>,
    pub(crate) global_sync_in_progress: Signal<bool>,
    pub(crate) last_run_completion: Signal<Option<SyncRunCompletion>>,
    pub(crate) action_error: Signal<Option<String>>,
}

pub(crate) fn use_sync_event_bridge(
    signals: SyncBridgeSignals,
    auth_state: AuthState,
    banner_state: BannerState,
    on_sync_settled: Callback<()>,
) {
    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    use_drop(move || {
        let _ = eval(SYNC_BRIDGE_CLEANUP_SCRIPT);
    });

    use_effect(move || {
        let SyncBridgeSignals {
            mut account_sync_state,
            mut account_sync_now,
            mut global_sync_in_progress,
            mut last_run_completion,
            mut action_error,
        } = signals;
        spawn(async move {
            let mut eval_result = eval(SYNC_BRIDGE_SCRIPT);
            while let Ok(value) = eval_result.recv().await {
                let bridge_message = match parse_sync_bridge_message(value) {
                    Ok(message) => message,
                    Err(error) => {
                        action_error.set(Some(error));
                        continue;
                    }
                };
                match bridge_message {
                    SyncBridgeMessage::StreamOpen => {
                        // Reconcile state that may have changed between SSR and
                        // the EventSource subscription becoming active.
                        on_sync_settled.call(());
                    }
                    SyncBridgeMessage::StreamError | SyncBridgeMessage::StreamUnavailable => {}
                    SyncBridgeMessage::PollTick => {
                        account_sync_now.set(Some(Utc::now()));
                        match get_sync_state().await {
                            Ok(snapshot) => {
                                let was_running = *global_sync_in_progress.peek();
                                global_sync_in_progress.set(snapshot.is_running);
                                if was_running && !snapshot.is_running {
                                    on_sync_settled.call(());
                                }
                            }
                            Err(err) => {
                                if err.is_unauthorized() {
                                    handle_session_expired(
                                        auth_state,
                                        banner_state,
                                        "wallets sync poll",
                                    );
                                } else {
                                    action_error.set(Some(err.to_string()));
                                }
                            }
                        }
                    }
                    SyncBridgeMessage::SyncEvent { event_name, data } => {
                        let sync_event = match serde_json::from_str::<TransactionSyncEvent>(&data) {
                            Ok(event) => event,
                            Err(err) => {
                                action_error.set(Some(format!(
                                    "Failed to parse sync event payload: {err}"
                                )));
                                continue;
                            }
                        };
                        if event_name != sync_event.event_name() {
                            continue;
                        }
                        account_sync_now.set(Some(Utc::now()));
                        match sync_event.event_type {
                            TransactionSyncEventType::Snapshot => {
                                if let Some(snapshot) = sync_event.snapshot.as_ref() {
                                    let was_running = *global_sync_in_progress.peek();
                                    global_sync_in_progress.set(snapshot.is_running);
                                    if was_running && !snapshot.is_running {
                                        on_sync_settled.call(());
                                    }
                                }
                            }
                            TransactionSyncEventType::Started => {
                                global_sync_in_progress.set(true);
                            }
                            TransactionSyncEventType::Completed
                            | TransactionSyncEventType::Failed => {
                                global_sync_in_progress.set(false);
                                last_run_completion.set(Some(SyncRunCompletion {
                                    run_id: sync_event.run_id,
                                    occurred_at: sync_event.occurred_at,
                                    failed: sync_event.event_type
                                        == TransactionSyncEventType::Failed,
                                    new_tx_count: sync_event
                                        .new_tx_count
                                        .map_or(0, |count| count.value()),
                                    updated_tx_count: sync_event
                                        .updated_tx_count
                                        .map_or(0, |count| count.value()),
                                    addresses_synced: sync_event
                                        .addresses_synced
                                        .map_or(0, |count| count.value()),
                                    error: sync_event.error.clone(),
                                }));
                                on_sync_settled.call(());
                            }
                            TransactionSyncEventType::AccountSyncStarted
                            | TransactionSyncEventType::AccountSyncProgress
                            | TransactionSyncEventType::AccountSyncCompleted
                            | TransactionSyncEventType::AccountSyncFailed => {
                                account_sync_state.with_mut(|states| {
                                    apply_account_sync_event(states, &sync_event);
                                });
                            }
                            TransactionSyncEventType::AccountIntegrationSyncStarted
                            | TransactionSyncEventType::AccountIntegrationSyncProgress
                            | TransactionSyncEventType::AccountIntegrationSyncCompleted
                            | TransactionSyncEventType::AccountIntegrationSyncFailed => {
                                account_sync_state.with_mut(|states| {
                                    apply_account_integration_sync_event(states, &sync_event);
                                });
                            }
                        }
                    }
                }
            }
        });
    });
}

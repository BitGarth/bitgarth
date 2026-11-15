//! Authority and completion state for work accepted from paired clients.
use super::{JobKey, TransactionSyncRunId, TriggerEnqueueResult, UserId};
use crate::auth::lifecycle::{UserRequestLease, acquire_client_key_request};
use crate::client_capabilities::CapabilityId;
use crate::transactions::AggregateSyncResult;
use std::collections::HashMap;
use tokio::time::{Duration, Instant};

tokio::task_local! {
    static PAIRED_AUTHORITY: (UserId, CapabilityId);
}

#[cfg(any(
    all(not(test), not(feature = "desktop")),
    all(test, not(bitgarth_db_unit_only))
))]
pub(crate) async fn with_paired_job_authority<F: std::future::Future>(
    user_id: UserId,
    capability_id: CapabilityId,
    work: F,
) -> F::Output {
    PAIRED_AUTHORITY.scope((user_id, capability_id), work).await
}

/// Keeps the database open and makes revocation wait through a queued job.
pub(super) struct ClientJobLease {
    lease: Option<UserRequestLease>,
    #[cfg(any(feature = "dev-config", test))]
    runtime: Option<std::sync::Arc<crate::runtime_context::RuntimeContext>>,
}

impl ClientJobLease {
    pub(super) fn for_request(key: JobKey) -> Result<Option<Self>, ()> {
        let Ok((owner, capability)) = PAIRED_AUTHORITY.try_with(|value| *value) else {
            return Ok(None);
        };
        let JobKey::User { user_id, .. } = key else {
            return Ok(None);
        };
        if owner != user_id {
            return Err(());
        }
        let lease = acquire_client_key_request(user_id, capability)
            .map_err(|_| ())?
            .ok_or(())?;
        Ok(Some(Self {
            lease: Some(lease),
            #[cfg(any(feature = "dev-config", test))]
            runtime: crate::runtime_context::current_runtime_context(),
        }))
    }

    #[cfg(any(feature = "dev-config", test))]
    pub(super) fn enter_runtime(
        &self,
    ) -> Option<crate::runtime_context::DefaultRuntimeContextGuard> {
        self.runtime
            .clone()
            .map(crate::runtime_context::push_default_runtime_context)
    }
}

impl Drop for ClientJobLease {
    fn drop(&mut self) {
        // A pending or rejected request may be dropped on the manager thread.
        #[cfg(any(feature = "dev-config", test))]
        let _runtime = self.enter_runtime();
        drop(self.lease.take());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ClientSyncStatus {
    Queued,
    Running,
    Completed { result: AggregateSyncResult },
}

#[derive(Default)]
pub(super) struct PairedSyncRuns {
    entries: HashMap<(UserId, TransactionSyncRunId), (ClientSyncStatus, Option<Instant>)>,
}

impl PairedSyncRuns {
    pub(super) fn register(&mut self, user: UserId, enqueue: TriggerEnqueueResult) {
        let (id, state) = match enqueue {
            TriggerEnqueueResult::AcceptedStarted { run_id: Some(id) } => {
                (id, ClientSyncStatus::Running)
            }
            TriggerEnqueueResult::AcceptedQueued { run_id: Some(id) } => {
                (id, ClientSyncStatus::Queued)
            }
            _ => return,
        };
        self.entries.entry((user, id)).or_insert((state, None));
    }

    pub(super) fn start(&mut self, user: UserId, run: TransactionSyncRunId) {
        if let Some(entry) = self.entries.get_mut(&(user, run)) {
            entry.0 = ClientSyncStatus::Running;
        }
    }

    pub(super) fn complete(
        &mut self,
        user: UserId,
        run: TransactionSyncRunId,
        result: AggregateSyncResult,
    ) {
        if let Some(entry) = self.entries.get_mut(&(user, run)) {
            *entry = (ClientSyncStatus::Completed { result }, Some(Instant::now()));
        }
    }

    pub(super) fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, (_, finished)| {
            finished.is_none_or(|at| now.duration_since(at) < Duration::from_secs(3600))
        });
    }

    #[cfg(any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    ))]
    pub(super) fn get(&self, user: UserId, run: TransactionSyncRunId) -> Option<ClientSyncStatus> {
        self.entries.get(&(user, run)).map(|entry| entry.0)
    }
}

#[cfg(any(
    all(not(test), not(feature = "desktop")),
    all(test, not(bitgarth_db_unit_only))
))]
pub(crate) async fn client_sync_status(
    user_id: UserId,
    run_id: TransactionSyncRunId,
) -> Result<Option<ClientSyncStatus>, ()> {
    let Some(manager) = super::TASK_MANAGER_COMMAND_TX.get() else {
        return Ok(None);
    };
    let (reply, result) = tokio::sync::oneshot::channel();
    manager
        .send(super::ManagerCommand::ClientSyncStatus {
            user_id,
            run_id,
            reply,
        })
        .await
        .map_err(|_| ())?;
    result.await.map_err(|_| ())
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn coalesced_sync_waits_for_its_own_completion() {
        let mut runs = PairedSyncRuns::default();
        let user = UserId::new();
        let first = TransactionSyncRunId::new();
        let next = TransactionSyncRunId::new();
        runs.register(
            user,
            TriggerEnqueueResult::AcceptedStarted {
                run_id: Some(first),
            },
        );
        let queued = TriggerEnqueueResult::AcceptedQueued { run_id: Some(next) };
        runs.register(user, queued);
        runs.register(user, queued);
        runs.complete(user, first, AggregateSyncResult::Success);
        assert_eq!(runs.get(user, next), Some(ClientSyncStatus::Queued));
        runs.start(user, next);
        assert_eq!(runs.get(user, next), Some(ClientSyncStatus::Running));
        runs.complete(user, next, AggregateSyncResult::Partial);
        assert_eq!(
            runs.get(user, next),
            Some(ClientSyncStatus::Completed {
                result: AggregateSyncResult::Partial
            })
        );
        assert_eq!(runs.get(UserId::new(), next), None);
        runs.prune(Instant::now() + Duration::from_secs(3601));
        assert_eq!(runs.get(user, next), None);
    }
}

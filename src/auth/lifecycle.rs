use crate::client_capabilities::CapabilityId;
use crate::models::{SessionId, UserId};
use dioxus::logger::tracing;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Condvar, Mutex, OnceLock};

#[derive(Default)]
struct UserLifecycleState {
    exclusive_user: bool,
    blocked_capabilities: HashSet<CapabilityId>,
    active_requests: usize,
    active_by_capability: HashMap<CapabilityId, usize>,
    browser_sessions: HashSet<SessionId>,
}

impl UserLifecycleState {
    fn is_idle(&self) -> bool {
        !self.exclusive_user
            && self.blocked_capabilities.is_empty()
            && self.active_requests == 0
            && self.active_by_capability.is_empty()
            && self.browser_sessions.is_empty()
    }
}

struct UserLifecycleRegistry {
    users: Mutex<HashMap<UserId, UserLifecycleState>>,
    changed: Condvar,
}

static USER_LIFECYCLE_REGISTRY: OnceLock<UserLifecycleRegistry> = OnceLock::new();

pub(crate) struct UserLifecycleGuard {
    user_id: UserId,
}

pub(crate) struct UserRequestLease {
    user_id: UserId,
    capability_id: Option<CapabilityId>,
    #[cfg(all(test, feature = "db-tests"))]
    before_close: Option<BeforeCloseHook>,
}

#[cfg(all(test, feature = "db-tests"))]
struct BeforeCloseHook {
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

pub(crate) struct CapabilityShutdownGuard {
    user_id: UserId,
    capability_id: CapabilityId,
}

#[derive(Debug)]
pub(crate) enum ClientCapabilityShutdownError {
    Lifecycle(UserLifecycleLockError),
    Database(crate::db::DbError),
}

impl fmt::Display for ClientCapabilityShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lifecycle(error) => write!(f, "{error}"),
            Self::Database(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ClientCapabilityShutdownError {}

#[derive(Debug)]
pub(crate) struct UserLifecycleLockError;

impl fmt::Display for UserLifecycleLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "user lifecycle lock registry is poisoned")
    }
}

impl std::error::Error for UserLifecycleLockError {}

pub(crate) fn acquire_user_lifecycle_lock(
    user_id: UserId,
) -> Result<UserLifecycleGuard, UserLifecycleLockError> {
    let registry = user_lifecycle_registry();
    let mut users = registry.users.lock().map_err(|_| UserLifecycleLockError)?;

    loop {
        let state = users.entry(user_id).or_default();
        if !state.exclusive_user && state.blocked_capabilities.is_empty() {
            state.exclusive_user = true;
            break;
        }
        users = registry
            .changed
            .wait(users)
            .map_err(|_| UserLifecycleLockError)?;
    }
    registry.changed.notify_all();

    while users
        .get(&user_id)
        .is_some_and(|state| state.active_requests > 0)
    {
        users = registry
            .changed
            .wait(users)
            .map_err(|_| UserLifecycleLockError)?;
    }

    Ok(UserLifecycleGuard { user_id })
}

pub(crate) fn acquire_session_request(
    user_id: UserId,
) -> Result<Option<UserRequestLease>, UserLifecycleLockError> {
    acquire_request(user_id, None)
}

pub(crate) fn acquire_pending_pairing_lease(
    user_id: UserId,
) -> Result<Option<UserRequestLease>, UserLifecycleLockError> {
    acquire_request(user_id, None)
}

pub(crate) fn acquire_client_key_request(
    user_id: UserId,
    capability_id: CapabilityId,
) -> Result<Option<UserRequestLease>, UserLifecycleLockError> {
    acquire_request(user_id, Some(capability_id))
}

#[cfg(all(not(test), feature = "desktop"))]
const _: fn(UserId, CapabilityId) -> Result<Option<UserRequestLease>, UserLifecycleLockError> =
    acquire_client_key_request;

fn acquire_request(
    user_id: UserId,
    capability_id: Option<CapabilityId>,
) -> Result<Option<UserRequestLease>, UserLifecycleLockError> {
    let registry = user_lifecycle_registry();
    let mut users = registry.users.lock().map_err(|_| UserLifecycleLockError)?;
    let state = users.entry(user_id).or_default();
    if state.exclusive_user
        || capability_id.is_some_and(|id| state.blocked_capabilities.contains(&id))
    {
        return Ok(None);
    }

    state.active_requests += 1;
    if let Some(capability_id) = capability_id {
        *state.active_by_capability.entry(capability_id).or_default() += 1;
    }
    Ok(Some(UserRequestLease {
        user_id,
        capability_id,
        #[cfg(all(test, feature = "db-tests"))]
        before_close: None,
    }))
}

pub(crate) fn pin_browser_session(
    user_id: UserId,
    session_id: SessionId,
) -> Result<(), UserLifecycleLockError> {
    let registry = user_lifecycle_registry();
    let mut users = registry.users.lock().map_err(|_| UserLifecycleLockError)?;
    users
        .entry(user_id)
        .or_default()
        .browser_sessions
        .insert(session_id);
    Ok(())
}

pub(crate) fn unpin_browser_session(
    user_id: UserId,
    session_id: SessionId,
) -> Result<bool, UserLifecycleLockError> {
    let registry = user_lifecycle_registry();
    let mut users = registry.users.lock().map_err(|_| UserLifecycleLockError)?;
    let Some(state) = users.get_mut(&user_id) else {
        return Ok(false);
    };
    state.browser_sessions.remove(&session_id);
    let should_close = state.active_requests == 0 && state.browser_sessions.is_empty();
    if state.is_idle() {
        users.remove(&user_id);
    }
    if should_close {
        crate::db::close_user_db(user_id).map_err(|_| UserLifecycleLockError)?;
    }
    drop(users);
    registry.changed.notify_all();
    Ok(should_close)
}

pub(crate) fn begin_capability_shutdown(
    user_id: UserId,
    capability_id: CapabilityId,
) -> Result<CapabilityShutdownGuard, UserLifecycleLockError> {
    let registry = user_lifecycle_registry();
    let mut users = registry.users.lock().map_err(|_| UserLifecycleLockError)?;
    loop {
        let state = users.entry(user_id).or_default();
        if !state.exclusive_user && !state.blocked_capabilities.contains(&capability_id) {
            state.blocked_capabilities.insert(capability_id);
            break;
        }
        users = registry
            .changed
            .wait(users)
            .map_err(|_| UserLifecycleLockError)?;
    }
    registry.changed.notify_all();
    Ok(CapabilityShutdownGuard {
        user_id,
        capability_id,
    })
}

pub(crate) fn shutdown_expired_client_capability(
    user_id: UserId,
    capability_id: CapabilityId,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, ClientCapabilityShutdownError> {
    let shutdown = begin_capability_shutdown(user_id, capability_id)
        .map_err(ClientCapabilityShutdownError::Lifecycle)?;
    let cleared = crate::db::clear_expired_client_capability_wrap(user_id, capability_id, now)
        .map_err(ClientCapabilityShutdownError::Database)?;
    shutdown
        .wait_for_requests()
        .map_err(ClientCapabilityShutdownError::Lifecycle)?;
    Ok(cleared)
}

impl CapabilityShutdownGuard {
    pub(crate) fn wait_for_requests(&self) -> Result<(), UserLifecycleLockError> {
        let registry = user_lifecycle_registry();
        let mut users = registry.users.lock().map_err(|_| UserLifecycleLockError)?;
        while users.get(&self.user_id).is_some_and(|state| {
            state
                .active_by_capability
                .get(&self.capability_id)
                .copied()
                .unwrap_or_default()
                > 0
        }) {
            users = registry
                .changed
                .wait(users)
                .map_err(|_| UserLifecycleLockError)?;
        }
        Ok(())
    }
}

impl UserLifecycleGuard {
    pub(crate) fn clear_browser_sessions(&self) -> Result<(), UserLifecycleLockError> {
        let registry = user_lifecycle_registry();
        let mut users = registry.users.lock().map_err(|_| UserLifecycleLockError)?;
        if let Some(state) = users.get_mut(&self.user_id) {
            state.browser_sessions.clear();
        }
        Ok(())
    }
}

impl Drop for UserRequestLease {
    fn drop(&mut self) {
        let registry = user_lifecycle_registry();
        let mut users = match registry.users.lock() {
            Ok(users) => users,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(state) = users.get_mut(&self.user_id) else {
            return;
        };
        state.active_requests = state.active_requests.saturating_sub(1);
        if let Some(capability_id) = self.capability_id
            && let Some(active) = state.active_by_capability.get_mut(&capability_id)
        {
            *active = active.saturating_sub(1);
            if *active == 0 {
                state.active_by_capability.remove(&capability_id);
            }
        }
        let should_close = state.active_requests == 0 && state.browser_sessions.is_empty();
        if state.is_idle() {
            users.remove(&self.user_id);
        }
        if should_close {
            #[cfg(all(test, feature = "db-tests"))]
            if let Some(hook) = self.before_close.take() {
                let _ = hook.reached.send(());
                let _ = hook.release.recv();
            }
            if let Err(error) = crate::db::close_user_db(self.user_id) {
                tracing::error!(
                    user_id = %self.user_id,
                    error = %error,
                    "user lifecycle: failed to close unpinned user database after request"
                );
            }
        }
        drop(users);
        registry.changed.notify_all();
    }
}

impl Drop for UserLifecycleGuard {
    fn drop(&mut self) {
        let registry = user_lifecycle_registry();
        let mut users = match registry.users.lock() {
            Ok(users) => users,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(state) = users.get_mut(&self.user_id) {
            state.exclusive_user = false;
            if state.is_idle() {
                users.remove(&self.user_id);
            }
        }
        registry.changed.notify_all();
    }
}

impl Drop for CapabilityShutdownGuard {
    fn drop(&mut self) {
        let registry = user_lifecycle_registry();
        let mut users = match registry.users.lock() {
            Ok(users) => users,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(state) = users.get_mut(&self.user_id) {
            state.blocked_capabilities.remove(&self.capability_id);
            if state.is_idle() {
                users.remove(&self.user_id);
            }
        }
        registry.changed.notify_all();
    }
}

fn user_lifecycle_registry() -> &'static UserLifecycleRegistry {
    USER_LIFECYCLE_REGISTRY.get_or_init(|| UserLifecycleRegistry {
        users: Mutex::new(HashMap::new()),
        changed: Condvar::new(),
    })
}

#[cfg(test)]
pub(crate) fn wait_until_user_exclusive_for_test(
    user_id: UserId,
) -> Result<(), UserLifecycleLockError> {
    let registry = user_lifecycle_registry();
    let mut users = registry.users.lock().map_err(|_| UserLifecycleLockError)?;
    while !users
        .get(&user_id)
        .is_some_and(|state| state.exclusive_user)
    {
        users = registry
            .changed
            .wait(users)
            .map_err(|_| UserLifecycleLockError)?;
    }
    Ok(())
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn wait_until_capability_blocked_for_test(
    user_id: UserId,
    capability_id: CapabilityId,
) -> Result<(), UserLifecycleLockError> {
    let registry = user_lifecycle_registry();
    let mut users = registry.users.lock().map_err(|_| UserLifecycleLockError)?;
    while !users
        .get(&user_id)
        .is_some_and(|state| state.blocked_capabilities.contains(&capability_id))
    {
        users = registry
            .changed
            .wait(users)
            .map_err(|_| UserLifecycleLockError)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn user_exclusive_operation_waits_for_request_lease() {
        let user_id = UserId::new();
        let request = acquire_session_request(user_id)
            .expect("request lease registry should lock")
            .expect("request should acquire");
        let started = Arc::new(Barrier::new(2));
        let worker_started = Arc::clone(&started);
        let (acquired_tx, acquired_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            worker_started.wait();
            let _exclusive =
                acquire_user_lifecycle_lock(user_id).expect("exclusive should eventually acquire");
            acquired_tx.send(()).expect("send exclusive acquisition");
        });
        started.wait();
        wait_until_user_exclusive_for_test(user_id)
            .expect("exclusive operation should block new requests before waiting");
        assert!(acquired_rx.try_recv().is_err());
        drop(request);
        acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("exclusive should acquire after request drops");
        handle.join().expect("exclusive waiter should finish");
    }

    #[test]
    fn capability_shutdown_blocks_only_its_capability() {
        let user_id = UserId::new();
        let blocked_id = CapabilityId::from_bytes([1_u8; 32]);
        let other_id = CapabilityId::from_bytes([2_u8; 32]);
        let shutdown = begin_capability_shutdown(user_id, blocked_id)
            .expect("capability shutdown should begin");

        assert!(
            acquire_client_key_request(user_id, blocked_id)
                .expect("blocked request registry should lock")
                .is_none()
        );
        assert!(
            acquire_client_key_request(user_id, other_id)
                .expect("other request registry should lock")
                .is_some()
        );
        drop(shutdown);
        assert!(
            acquire_client_key_request(user_id, blocked_id)
                .expect("unblocked request registry should lock")
                .is_some()
        );
    }

    #[test]
    fn capability_shutdown_waits_only_for_target_capability() {
        let user_id = UserId::new();
        let target_id = CapabilityId::from_bytes([3_u8; 32]);
        let other_id = CapabilityId::from_bytes([4_u8; 32]);
        let request_acquired = Arc::new(Barrier::new(2));
        let release_request = Arc::new(Barrier::new(2));
        let worker_acquired = Arc::clone(&request_acquired);
        let worker_release = Arc::clone(&release_request);
        let request_worker = thread::spawn(move || {
            let target_request = acquire_client_key_request(user_id, target_id)
                .expect("target request registry should lock")
                .expect("target request should acquire");
            worker_acquired.wait();
            worker_release.wait();
            drop(target_request);
        });
        request_acquired.wait();
        let _other_request = acquire_client_key_request(user_id, other_id)
            .expect("other request registry should lock")
            .expect("other request should acquire");
        let shutdown = begin_capability_shutdown(user_id, target_id)
            .expect("capability shutdown should begin");
        let (started_tx, started_rx) = mpsc::channel();
        let (drained_tx, drained_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            started_tx.send(()).expect("send shutdown wait start");
            shutdown
                .wait_for_requests()
                .expect("shutdown wait should succeed");
            drained_tx.send(()).expect("send shutdown drained");
        });
        started_rx.recv().expect("shutdown waiter should start");
        assert!(drained_rx.try_recv().is_err());
        assert!(
            acquire_client_key_request(user_id, target_id)
                .expect("blocked request registry should lock")
                .is_none()
        );
        release_request.wait();
        drained_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("target shutdown should drain without other capability");
        handle.join().expect("shutdown waiter should finish");
        request_worker.join().expect("target request should finish");
    }

    #[test]
    fn same_user_lifecycle_lock_blocks_until_guard_drops() {
        let user_id = UserId::new();
        let guard = acquire_user_lifecycle_lock(user_id).expect("first lock should acquire");
        let (tx, rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            started_tx.send(()).expect("send waiter start signal");
            let _guard = acquire_user_lifecycle_lock(user_id).expect("second lock should acquire");
            tx.send(()).expect("send acquisition signal");
        });

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("same user lock waiter should start");
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "same user lock should block while first guard is held"
        );

        drop(guard);
        rx.recv_timeout(Duration::from_secs(1))
            .expect("same user lock should acquire after first guard drops");
        handle.join().expect("lock waiter should finish");
    }

    #[test]
    fn different_user_lifecycle_locks_do_not_block_each_other() {
        let first_user = UserId::new();
        let second_user = UserId::new();
        let _guard =
            acquire_user_lifecycle_lock(first_user).expect("first user lock should acquire");
        let (tx, rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            let _guard =
                acquire_user_lifecycle_lock(second_user).expect("second user lock should acquire");
            tx.send(()).expect("send acquisition signal");
        });

        rx.recv_timeout(Duration::from_secs(1))
            .expect("different user lock should acquire immediately");
        handle
            .join()
            .expect("different user lock thread should finish");
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod db_tests {
    use super::*;
    use crate::db::{
        acquire_test_runtime, list_open_user_db_users, setup_test_user, unique_user_id,
    };
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn last_unpinned_request_lease_closes_user_database() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let lease = acquire_session_request(user_id)
            .expect("request registry should lock")
            .expect("request should acquire");
        assert!(
            list_open_user_db_users()
                .expect("open user databases should list")
                .contains(&user_id)
        );

        drop(lease);

        assert!(
            !list_open_user_db_users()
                .expect("open user databases should list after drop")
                .contains(&user_id)
        );
    }

    #[test]
    fn request_admission_waits_until_idle_database_close_finishes() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let mut closing_lease = acquire_session_request(user_id)
            .expect("request registry should lock")
            .expect("request should acquire");
        let (close_reached_tx, close_reached_rx) = mpsc::sync_channel(0);
        let (close_release_tx, close_release_rx) = mpsc::channel();
        closing_lease.before_close = Some(BeforeCloseHook {
            reached: close_reached_tx,
            release: close_release_rx,
        });

        let closing = thread::spawn(move || drop(closing_lease));
        close_reached_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("last lease should reach database close");

        let (admitted_tx, admitted_rx) = mpsc::channel();
        let admitted = thread::spawn(move || {
            admitted_tx
                .send(acquire_session_request(user_id))
                .expect("send admission result");
        });
        let admitted_before_close = admitted_rx.recv_timeout(Duration::from_millis(50)).is_ok();

        close_release_tx.send(()).expect("release database close");
        closing.join().expect("closing request should finish");
        if !admitted_before_close {
            let lease = admitted_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("request should acquire after close")
                .expect("request registry should lock")
                .expect("request should acquire");
            drop(lease);
        }
        admitted.join().expect("admission thread should finish");

        assert!(
            !admitted_before_close,
            "a new request acquired while the idle database close was paused"
        );
    }
}

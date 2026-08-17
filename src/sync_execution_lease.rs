use crate::models::UserId;
use std::collections::HashSet;
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};

static USER_SYNC_LEASES: LazyLock<(Mutex<HashSet<UserId>>, Condvar)> =
    LazyLock::new(|| (Mutex::new(HashSet::new()), Condvar::new()));

pub(crate) struct UserSyncExecutionLease {
    user_id: UserId,
}

impl UserSyncExecutionLease {
    pub(crate) fn try_acquire(user_id: UserId) -> Option<Self> {
        let mut leased = lock_leased_users();
        let acquired = leased.insert(user_id);
        drop(leased);
        acquired.then_some(Self { user_id })
    }

    pub(crate) fn acquire(user_id: UserId) -> Self {
        let (leased_mutex, released) = &*USER_SYNC_LEASES;
        let mut leased = leased_mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while leased.contains(&user_id) {
            leased = released
                .wait(leased)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        leased.insert(user_id);
        drop(leased);
        Self { user_id }
    }
}

impl Drop for UserSyncExecutionLease {
    fn drop(&mut self) {
        let (leased_mutex, released) = &*USER_SYNC_LEASES;
        leased_mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.user_id);
        released.notify_all();
    }
}

fn lock_leased_users() -> MutexGuard<'static, HashSet<UserId>> {
    USER_SYNC_LEASES
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

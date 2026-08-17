use crate::auth::lifecycle::UserRequestLease;
use crate::client_capabilities::{CapabilityId, ClientKeyVerifier, ClientPermission};
use crate::db::encryption::Dek;
use crate::models::{FieldErrors, UserId};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
#[cfg(any(test, all(not(feature = "desktop"), not(target_arch = "wasm32"))))]
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(any(test, all(not(feature = "desktop"), not(target_arch = "wasm32"))))]
use std::time::Duration as StdDuration;
use zeroize::Zeroize;

pub(crate) const MAX_START_BODY_BYTES: usize = 1024;
const PAIRING_LIFETIME: Duration = Duration::minutes(10);
const SOURCE_WINDOW: Duration = Duration::minutes(10);
const GLOBAL_WINDOW: Duration = Duration::minutes(1);
const MAX_PENDING_PAIRINGS: usize = 1024;
const MAX_SOURCE_STARTS: usize = 10;
const MAX_GLOBAL_STARTS: usize = 300;
#[cfg(any(test, all(not(feature = "desktop"), not(target_arch = "wasm32"))))]
const EXPIRY_CLEANUP_INTERVAL: StdDuration = StdDuration::from_secs(1);
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PairingStartRequest {
    pub(crate) client_name: String,
    pub(crate) key_verifier: String,
    pub(crate) permissions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PairingStartResponse {
    pub(crate) pairing_id: String,
    pub(crate) code: String,
    pub(crate) approval_url: String,
    pub(crate) expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedPairingStart {
    client_name: String,
    key_verifier: ClientKeyVerifier,
    permission: ClientPermission,
}

impl PairingStartRequest {
    pub(crate) fn validate(self) -> Result<ValidatedPairingStart, FieldErrors> {
        let mut errors = FieldErrors::new();
        let scalar_count = self.client_name.chars().count();
        if scalar_count == 0 || scalar_count > 64 || self.client_name.len() > 256 {
            errors.add(
                "client_name",
                "Client name must contain 1 to 64 characters and at most 256 UTF-8 bytes"
                    .to_owned(),
            );
        }
        if self.client_name.trim() != self.client_name {
            errors.add(
                "client_name",
                "Client name must not have leading or trailing whitespace".to_owned(),
            );
        }
        if self.client_name.chars().any(char::is_control) {
            errors.add(
                "client_name",
                "Client name must not contain control characters".to_owned(),
            );
        }

        let verifier = match parse_verifier(&self.key_verifier) {
            Ok(verifier) => Some(verifier),
            Err(message) => {
                errors.add("key_verifier", message);
                None
            }
        };

        if self.permissions.as_slice() != [ClientPermission::BalancesRead.as_str()] {
            errors.add(
                "permissions",
                "Permissions must be exactly [\"balances_read\"]".to_owned(),
            );
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        let Some(key_verifier) = verifier else {
            return Err(errors);
        };
        Ok(ValidatedPairingStart {
            client_name: self.client_name,
            key_verifier,
            permission: ClientPermission::BalancesRead,
        })
    }
}

fn parse_verifier(value: &str) -> Result<ClientKeyVerifier, String> {
    if value.len() != 43 {
        return Err("Key verifier must be 43 characters".to_owned());
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| "Key verifier must be canonical unpadded base64url".to_owned())?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| "Key verifier must decode to 32 bytes".to_owned())?;
    if URL_SAFE_NO_PAD.encode(bytes) != value {
        return Err("Key verifier must be canonical unpadded base64url".to_owned());
    }
    Ok(ClientKeyVerifier::from_bytes(bytes))
}

enum PairingStatus {
    AwaitingApproval,
    Approved(ApprovedPairing),
    Denied,
    Completed(ClaimedPairing),
}

struct ApprovedPairing {
    user_id: UserId,
    dek: Option<Dek>,
    _lease: UserRequestLease,
    active_expires_at: Option<DateTime<Utc>>,
}

pub(crate) struct ApprovedPairingBinding {
    pub(crate) user_id: UserId,
    pub(crate) dek: Option<Dek>,
    pub(crate) lease: UserRequestLease,
}

pub(crate) struct ApprovedPairingClaim<'a> {
    pub(crate) capability_id: CapabilityId,
    pub(crate) user_id: UserId,
    pub(crate) dek: Option<&'a Dek>,
    pub(crate) client_name: &'a str,
    pub(crate) permission: ClientPermission,
    pub(crate) active_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimedPairing {
    pub(crate) user_id: UserId,
    pub(crate) permission: ClientPermission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingClaimError {
    NotFound,
    Unauthorized,
    Pending,
    Denied,
    Activation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingPairingReview {
    pub(crate) pairing_id: String,
    pub(crate) code: String,
    pub(crate) client_name: String,
    pub(crate) permissions: Vec<String>,
    pub(crate) expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingTransitionError {
    NotFound,
    Conflict,
    Binding,
}

struct PendingPairing {
    code: String,
    client_name: String,
    key_verifier: ClientKeyVerifier,
    permission: ClientPermission,
    expires_at: DateTime<Utc>,
    status: PairingStatus,
}

#[derive(PartialEq, Eq, Hash)]
struct VerifierReservation([u8; 32]);

impl VerifierReservation {
    fn new(verifier: ClientKeyVerifier) -> Self {
        Self(*verifier.as_bytes())
    }

    fn matches(&self, verifier: ClientKeyVerifier) -> bool {
        self.0 == *verifier.as_bytes()
    }
}

impl Drop for VerifierReservation {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for PendingPairing {
    fn drop(&mut self) {
        self.code.zeroize();
        self.client_name.zeroize();
        self.key_verifier = ClientKeyVerifier::from_bytes([0_u8; 32]);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartedPairing {
    pub(crate) capability_id: CapabilityId,
    pub(crate) code: String,
    pub(crate) expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingStartError {
    RateLimited { retry_after_seconds: u64 },
    CapacityFull { retry_after_seconds: u64 },
    VerifierConflict,
    Database,
    GenerationExhausted,
}

#[derive(Default)]
struct PairingState {
    pending: HashMap<CapabilityId, PendingPairing>,
    verifiers: HashSet<VerifierReservation>,
    source_starts: HashMap<IpAddr, VecDeque<DateTime<Utc>>>,
    global_starts: VecDeque<DateTime<Utc>>,
}

#[derive(Default)]
pub(crate) struct PairingStore {
    state: Mutex<PairingState>,
}

impl PairingStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(any(test, all(not(feature = "desktop"), not(target_arch = "wasm32"))))]
    pub(crate) async fn run_expiry_cleanup(self: Arc<Self>) {
        let mut interval = tokio::time::interval(EXPIRY_CLEANUP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.remove_expired(Utc::now());
        }
    }

    pub(crate) fn start<I, F, E>(
        &self,
        now: DateTime<Utc>,
        source: IpAddr,
        start: ValidatedPairingStart,
        generated: I,
        durable_verifier_exists: F,
    ) -> Result<StartedPairing, PairingStartError>
    where
        I: IntoIterator<Item = ([u8; 32], [u8; 8])>,
        F: FnOnce(ClientKeyVerifier) -> Result<bool, E>,
    {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.remove_expired(now);
        state.prune_rate_limits(now);

        if let Some(retry_after_seconds) = state.rate_limit_retry_after(source, now) {
            return Err(PairingStartError::RateLimited {
                retry_after_seconds,
            });
        }
        if state.pending.len() >= MAX_PENDING_PAIRINGS {
            let retry_after_seconds = state
                .pending
                .values()
                .map(|entry| seconds_until(entry.expires_at, now))
                .min()
                .unwrap_or(1);
            return Err(PairingStartError::CapacityFull {
                retry_after_seconds,
            });
        }

        state.record_start(source, now);
        if state
            .verifiers
            .iter()
            .any(|reservation| reservation.matches(start.key_verifier))
            || durable_verifier_exists(start.key_verifier)
                .map_err(|_| PairingStartError::Database)?
        {
            return Err(PairingStartError::VerifierConflict);
        }

        for (id_bytes, code_bytes) in generated {
            let capability_id = CapabilityId::from_bytes(id_bytes);
            let code = encode_code(code_bytes);
            if state.pending.contains_key(&capability_id)
                || state.pending.values().any(|entry| entry.code == code)
            {
                continue;
            }

            let expires_at = now + PAIRING_LIFETIME;
            state
                .verifiers
                .insert(VerifierReservation::new(start.key_verifier));
            state.pending.insert(
                capability_id,
                PendingPairing {
                    code: code.clone(),
                    client_name: start.client_name,
                    key_verifier: start.key_verifier,
                    permission: start.permission,
                    expires_at,
                    status: PairingStatus::AwaitingApproval,
                },
            );
            return Ok(StartedPairing {
                capability_id,
                code,
                expires_at,
            });
        }

        Err(PairingStartError::GenerationExhausted)
    }

    pub(crate) fn is_live_approval_code(&self, now: DateTime<Utc>, code: &str) -> bool {
        self.review(now, code).is_ok()
    }

    pub(crate) fn review(
        &self,
        now: DateTime<Utc>,
        display_code: &str,
    ) -> Result<PendingPairingReview, PairingTransitionError> {
        let Some(code) = parse_display_code(display_code) else {
            return Err(PairingTransitionError::NotFound);
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.remove_expired(now);
        let Some((capability_id, entry)) =
            state.pending.iter().find(|(_, entry)| entry.code == code)
        else {
            return Err(PairingTransitionError::NotFound);
        };
        if !matches!(entry.status, PairingStatus::AwaitingApproval) {
            return Err(PairingTransitionError::Conflict);
        }
        Ok(PendingPairingReview {
            pairing_id: capability_id.to_string(),
            code: format_code(&entry.code),
            client_name: entry.client_name.clone(),
            permissions: vec![entry.permission.as_str().to_owned()],
            expires_at: format_expiry(entry.expires_at),
        })
    }

    pub(crate) fn approve<F, E>(
        &self,
        now: DateTime<Utc>,
        capability_id: CapabilityId,
        display_code: &str,
        active_expires_at: Option<DateTime<Utc>>,
        bind: F,
    ) -> Result<(), PairingTransitionError>
    where
        F: FnOnce() -> Result<ApprovedPairingBinding, E>,
    {
        let Some(code) = parse_display_code(display_code) else {
            return Err(PairingTransitionError::NotFound);
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.remove_expired(now);
        let Some(entry) = state.pending.get_mut(&capability_id) else {
            return Err(PairingTransitionError::NotFound);
        };
        if entry.code != code {
            return Err(PairingTransitionError::NotFound);
        }
        if !matches!(entry.status, PairingStatus::AwaitingApproval) {
            return Err(PairingTransitionError::Conflict);
        }
        let binding = bind().map_err(|_| PairingTransitionError::Binding)?;
        entry.status = PairingStatus::Approved(ApprovedPairing {
            user_id: binding.user_id,
            dek: binding.dek,
            _lease: binding.lease,
            active_expires_at,
        });
        Ok(())
    }

    pub(crate) fn deny(
        &self,
        now: DateTime<Utc>,
        capability_id: CapabilityId,
        display_code: &str,
    ) -> Result<(), PairingTransitionError> {
        let Some(code) = parse_display_code(display_code) else {
            return Err(PairingTransitionError::NotFound);
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.remove_expired(now);
        let Some(entry) = state.pending.get_mut(&capability_id) else {
            return Err(PairingTransitionError::NotFound);
        };
        if entry.code != code {
            return Err(PairingTransitionError::NotFound);
        }
        if !matches!(entry.status, PairingStatus::AwaitingApproval) {
            return Err(PairingTransitionError::Conflict);
        }
        entry.status = PairingStatus::Denied;
        Ok(())
    }

    pub(crate) fn claim<F, E>(
        &self,
        now: DateTime<Utc>,
        capability_id: CapabilityId,
        key_verifier: ClientKeyVerifier,
        activate: F,
    ) -> Result<ClaimedPairing, PairingClaimError>
    where
        F: FnOnce(ApprovedPairingClaim<'_>) -> Result<ClaimedPairing, E>,
    {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.remove_expired(now);
        let Some(entry) = state.pending.get_mut(&capability_id) else {
            return Err(PairingClaimError::NotFound);
        };
        if entry.key_verifier != key_verifier {
            return Err(PairingClaimError::Unauthorized);
        }

        let claimed = match &entry.status {
            PairingStatus::AwaitingApproval => return Err(PairingClaimError::Pending),
            PairingStatus::Denied => return Err(PairingClaimError::Denied),
            PairingStatus::Completed(claimed) => return Ok(claimed.clone()),
            PairingStatus::Approved(approved) => activate(ApprovedPairingClaim {
                capability_id,
                user_id: approved.user_id,
                dek: approved.dek.as_ref(),
                client_name: &entry.client_name,
                permission: entry.permission,
                active_expires_at: approved.active_expires_at,
            })
            .map_err(|_| PairingClaimError::Activation)?,
        };
        entry.client_name.zeroize();
        entry.code.zeroize();
        entry.status = PairingStatus::Completed(claimed.clone());
        Ok(claimed)
    }

    #[cfg(test)]
    fn len_at(&self, now: DateTime<Utc>) -> usize {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.remove_expired(now);
        state.pending.len()
    }

    #[cfg(test)]
    fn len_without_cleanup(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .len()
    }
}

impl PairingState {
    fn remove_expired(&mut self, now: DateTime<Utc>) {
        let expired = self
            .pending
            .iter()
            .filter_map(|(id, entry)| (entry.expires_at <= now).then_some(*id))
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(entry) = self.pending.remove(&id) {
                self.verifiers
                    .retain(|reservation| !reservation.matches(entry.key_verifier));
            }
        }
    }

    fn prune_rate_limits(&mut self, now: DateTime<Utc>) {
        let source_cutoff = now - SOURCE_WINDOW;
        self.source_starts.retain(|_, starts| {
            while starts.front().is_some_and(|at| *at <= source_cutoff) {
                starts.pop_front();
            }
            !starts.is_empty()
        });
        let global_cutoff = now - GLOBAL_WINDOW;
        while self
            .global_starts
            .front()
            .is_some_and(|at| *at <= global_cutoff)
        {
            self.global_starts.pop_front();
        }
    }

    fn rate_limit_retry_after(&self, source: IpAddr, now: DateTime<Utc>) -> Option<u64> {
        let source_retry = self
            .source_starts
            .get(&source)
            .filter(|starts| starts.len() >= MAX_SOURCE_STARTS)
            .and_then(VecDeque::front)
            .map(|at| seconds_until(*at + SOURCE_WINDOW, now));
        let global_retry = (self.global_starts.len() >= MAX_GLOBAL_STARTS)
            .then(|| self.global_starts.front())
            .flatten()
            .map(|at| seconds_until(*at + GLOBAL_WINDOW, now));
        match (source_retry, global_retry) {
            (Some(source), Some(global)) => Some(source.max(global)),
            (Some(retry), None) | (None, Some(retry)) => Some(retry),
            (None, None) => None,
        }
    }

    fn record_start(&mut self, source: IpAddr, now: DateTime<Utc>) {
        self.source_starts.entry(source).or_default().push_back(now);
        self.global_starts.push_back(now);
    }
}

fn encode_code(bytes: [u8; 8]) -> String {
    bytes
        .into_iter()
        .map(|byte| char::from(CODE_ALPHABET[usize::from(byte & 31)]))
        .collect()
}

pub(crate) fn format_code(code: &str) -> String {
    format!("{}-{}", &code[..4], &code[4..])
}

fn parse_display_code(value: &str) -> Option<String> {
    if value.len() != 9 || value.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    let code = format!("{}{}", &value[..4], &value[5..]);
    (code.len() == 8 && code.bytes().all(|byte| CODE_ALPHABET.contains(&byte))).then_some(code)
}

pub(crate) fn format_expiry(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn seconds_until(deadline: DateTime<Utc>, now: DateTime<Utc>) -> u64 {
    u64::try_from((deadline - now).num_seconds().max(1)).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::net::{IpAddr, Ipv4Addr};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 31, 15, 0, 0)
            .single()
            .expect("fixed timestamp should be valid")
    }

    fn request(name: &str, verifier_byte: u8) -> PairingStartRequest {
        PairingStartRequest {
            client_name: name.to_owned(),
            key_verifier: URL_SAFE_NO_PAD.encode([verifier_byte; 32]),
            permissions: vec!["balances_read".to_owned()],
        }
    }

    fn source(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
    }

    fn generated(id: u8, code: u8) -> [([u8; 32], [u8; 8]); 1] {
        [([id; 32], [code; 8])]
    }

    fn unique_bytes(index: u16) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        bytes[..2].copy_from_slice(&index.to_be_bytes());
        bytes
    }

    fn unique_code(index: u16) -> [u8; 8] {
        let mut bytes = [0_u8; 8];
        bytes[0] = u8::try_from(index & 31).unwrap();
        bytes[1] = u8::try_from((index >> 5) & 31).unwrap();
        bytes
    }

    #[test]
    fn validates_client_name_verifier_and_exact_permissions() {
        assert!(request("business", 1).validate().is_ok());
        for invalid_name in ["", " business", "business ", "bad\nname"] {
            assert!(request(invalid_name, 1).validate().is_err());
        }
        assert!(request(&"x".repeat(65), 1).validate().is_err());
        assert!(request(&"🦀".repeat(64), 1).validate().is_ok());
        assert!(
            request(&format!("{}a", "🦀".repeat(64)), 1)
                .validate()
                .is_err()
        );

        let mut padded = request("business", 1);
        padded.key_verifier.push('=');
        assert!(padded.validate().is_err());
        let mut too_short = request("business", 1);
        too_short.key_verifier.pop();
        assert!(too_short.validate().is_err());
        let mut extra_permission = request("business", 1);
        extra_permission
            .permissions
            .push("transactions_read".to_owned());
        assert!(extra_permission.validate().is_err());
        let mut duplicate_permission = request("business", 1);
        duplicate_permission
            .permissions
            .push("balances_read".to_owned());
        assert!(duplicate_permission.validate().is_err());
    }

    #[test]
    fn collisions_regenerate_without_overwriting() {
        let store = PairingStore::new();
        let first = store
            .start(
                now(),
                source(1),
                request("one", 1).validate().unwrap(),
                generated(1, 1),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        let second = store
            .start(
                now(),
                source(2),
                request("two", 2).validate().unwrap(),
                [([1; 32], [2; 8]), ([2; 32], [1; 8]), ([2; 32], [2; 8])],
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        assert_ne!(first.capability_id, second.capability_id);
        assert_ne!(first.code, second.code);
        assert_eq!(store.len_at(now()), 2);
    }

    #[test]
    fn expires_entries_and_releases_verifier_reservations() {
        let store = PairingStore::new();
        let first = store
            .start(
                now(),
                source(1),
                request("one", 1).validate().unwrap(),
                generated(1, 1),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        assert_eq!(first.expires_at, now() + Duration::minutes(10));
        assert_eq!(store.len_at(now() + Duration::minutes(10)), 0);
        assert!(
            store
                .start(
                    now() + Duration::minutes(10),
                    source(1),
                    request("one", 1).validate().unwrap(),
                    generated(2, 2),
                    |_| Ok::<_, ()>(false),
                )
                .is_ok()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn expiry_task_releases_an_approved_pairing_without_another_api_request() {
        let store = Arc::new(PairingStore::new());
        let started_at = Utc::now() - PAIRING_LIFETIME - Duration::seconds(1);
        let started = store
            .start(
                started_at,
                source(1),
                request("approved", 1).validate().unwrap(),
                generated(1, 1),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        let user_id = UserId::new();
        store
            .approve(
                started_at,
                started.capability_id,
                &format_code(&started.code),
                None,
                || {
                    Ok::<_, ()>(ApprovedPairingBinding {
                        user_id,
                        dek: None,
                        lease: crate::auth::lifecycle::acquire_pending_pairing_lease(user_id)
                            .unwrap()
                            .unwrap(),
                    })
                },
            )
            .unwrap();

        let cleanup = tokio::spawn(Arc::clone(&store).run_expiry_cleanup());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while store.len_without_cleanup() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the background expiry task should remove the expired approval");
        cleanup.abort();
    }

    #[test]
    fn rejects_pending_and_durable_verifier_conflicts() {
        let store = PairingStore::new();
        store
            .start(
                now(),
                source(1),
                request("one", 1).validate().unwrap(),
                generated(1, 1),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        assert_eq!(
            store.start(
                now(),
                source(2),
                request("two", 1).validate().unwrap(),
                generated(2, 2),
                |_| Ok::<_, ()>(false)
            ),
            Err(PairingStartError::VerifierConflict)
        );
        assert_eq!(
            store.start(
                now(),
                source(3),
                request("three", 3).validate().unwrap(),
                generated(3, 3),
                |_| Ok::<_, ()>(true)
            ),
            Err(PairingStartError::VerifierConflict)
        );
    }

    #[test]
    fn applies_source_and_global_rate_limits() {
        let store = PairingStore::new();
        for index in 0_u16..10 {
            store
                .start(
                    now(),
                    source(1),
                    request("client", u8::try_from(index).unwrap())
                        .validate()
                        .unwrap(),
                    generated(
                        u8::try_from(index + 1).unwrap(),
                        u8::try_from(index + 1).unwrap(),
                    ),
                    |_| Ok::<_, ()>(false),
                )
                .unwrap();
        }
        assert_eq!(
            store.start(
                now(),
                source(1),
                request("limited", 20).validate().unwrap(),
                generated(20, 20),
                |_| Ok::<_, ()>(false)
            ),
            Err(PairingStartError::RateLimited {
                retry_after_seconds: 600
            })
        );

        let global = PairingStore::new();
        for index in 0_u16..300 {
            let verifier = unique_bytes(index);
            let validated = PairingStartRequest {
                client_name: "client".to_owned(),
                key_verifier: URL_SAFE_NO_PAD.encode(verifier),
                permissions: vec!["balances_read".to_owned()],
            }
            .validate()
            .unwrap();
            global
                .start(
                    now(),
                    source(u8::try_from(index / 9 + 1).unwrap()),
                    validated,
                    [(unique_bytes(index), unique_code(index))],
                    |_| Ok::<_, ()>(false),
                )
                .unwrap();
        }
        assert_eq!(
            global.start(
                now(),
                source(250),
                request("limited", 250).validate().unwrap(),
                generated(250, 250),
                |_| Ok::<_, ()>(false)
            ),
            Err(PairingStartError::RateLimited {
                retry_after_seconds: 60
            })
        );
    }

    #[test]
    fn rejects_capacity_without_evicting_live_entries() {
        let store = PairingStore::new();
        for index in 0_u16..1024 {
            let verifier = unique_bytes(index);
            let validated = PairingStartRequest {
                client_name: "client".to_owned(),
                key_verifier: URL_SAFE_NO_PAD.encode(verifier),
                permissions: vec!["balances_read".to_owned()],
            }
            .validate()
            .unwrap();
            store
                .start(
                    now() + Duration::minutes(i64::from(index / 250)),
                    source(u8::try_from(index % 200 + 1).unwrap()),
                    validated,
                    [(unique_bytes(index), unique_code(index))],
                    |_| Ok::<_, ()>(false),
                )
                .unwrap();
        }
        assert_eq!(
            store.start(
                now() + Duration::minutes(4),
                source(250),
                request("full", 250).validate().unwrap(),
                generated(250, 250),
                |_| Ok::<_, ()>(false)
            ),
            Err(PairingStartError::CapacityFull {
                retry_after_seconds: 360
            })
        );
    }

    #[test]
    fn formats_code_and_expiry_at_the_boundary() {
        assert_eq!(format_code("ABCDEFGH"), "ABCD-EFGH");
        assert_eq!(format_expiry(now()), "2026-07-31T15:00:00Z");
        assert_eq!(MAX_START_BODY_BYTES, 1024);
    }

    #[test]
    fn canonical_client_api_fixtures_match_server_types() {
        let key_fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/client_api/client-key.json"))
                .unwrap();
        let raw_key = URL_SAFE_NO_PAD
            .decode(key_fixture["client_key"].as_str().unwrap())
            .unwrap();
        let raw_key: [u8; 32] = raw_key.try_into().unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(ClientKeyVerifier::from_raw_key(&raw_key).as_bytes()),
            key_fixture["key_verifier"]
        );

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/client_api/pairing-start.json"
        ))
        .unwrap();
        let request: PairingStartRequest =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert!(request.validate().is_ok());
        let response: PairingStartResponse =
            serde_json::from_value(fixture["response"].clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), fixture["response"]);
    }

    #[test]
    fn review_rejects_wrong_expired_and_non_awaiting_codes() {
        let store = PairingStore::new();
        let started = store
            .start(
                now(),
                source(1),
                request("business", 1).validate().unwrap(),
                generated(1, 1),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        let code = format_code(&started.code);
        let review = store.review(now(), &code).unwrap();
        assert_eq!(review.pairing_id, started.capability_id.to_string());
        assert_eq!(review.client_name, "business");
        assert_eq!(review.permissions, ["balances_read"]);
        assert_eq!(
            store.review(now(), "2222-2222"),
            Err(PairingTransitionError::NotFound)
        );
        assert_eq!(
            store.review(now() + Duration::minutes(10), &code),
            Err(PairingTransitionError::NotFound)
        );
    }

    #[test]
    fn approve_and_deny_are_single_use_transitions() {
        let approved_store = PairingStore::new();
        let approved = approved_store
            .start(
                now(),
                source(1),
                request("approved", 1).validate().unwrap(),
                generated(1, 1),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        let approved_code = format_code(&approved.code);
        let user_id = UserId::new();
        approved_store
            .approve(
                now(),
                approved.capability_id,
                &approved_code,
                Some(now() + Duration::days(3650)),
                || {
                    Ok::<_, ()>(ApprovedPairingBinding {
                        user_id,
                        dek: None,
                        lease: crate::auth::lifecycle::acquire_pending_pairing_lease(user_id)
                            .unwrap()
                            .unwrap(),
                    })
                },
            )
            .unwrap();
        assert_eq!(
            approved_store.deny(now(), approved.capability_id, &approved_code),
            Err(PairingTransitionError::Conflict)
        );

        let denied_store = PairingStore::new();
        let denied = denied_store
            .start(
                now(),
                source(1),
                request("denied", 2).validate().unwrap(),
                generated(2, 2),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        let denied_code = format_code(&denied.code);
        denied_store
            .deny(now(), denied.capability_id, &denied_code)
            .unwrap();
        assert_eq!(
            denied_store.deny(now(), denied.capability_id, &denied_code),
            Err(PairingTransitionError::Conflict)
        );
    }

    #[test]
    fn approval_rechecks_code_to_id_binding_before_acquiring_a_lease() {
        let store = PairingStore::new();
        let first = store
            .start(
                now(),
                source(1),
                request("first", 1).validate().unwrap(),
                generated(1, 1),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        let second = store
            .start(
                now(),
                source(2),
                request("second", 2).validate().unwrap(),
                generated(2, 2),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        let mut binding_called = false;
        assert_eq!(
            store.approve(
                now(),
                first.capability_id,
                &format_code(&second.code),
                None,
                || {
                    binding_called = true;
                    Err::<ApprovedPairingBinding, _>(())
                },
            ),
            Err(PairingTransitionError::NotFound)
        );
        assert!(!binding_called);
    }

    #[cfg(feature = "dev-config")]
    #[test]
    fn approved_unencrypted_pairing_can_be_claimed_without_a_dek() {
        let store = PairingStore::new();
        let started = store
            .start(
                now(),
                source(1),
                request("unencrypted dev", 1).validate().unwrap(),
                generated(1, 1),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        let user_id = UserId::new();
        store
            .approve(
                now(),
                started.capability_id,
                &format_code(&started.code),
                None,
                || {
                    Ok::<_, ()>(ApprovedPairingBinding {
                        user_id,
                        dek: None,
                        lease: crate::auth::lifecycle::acquire_pending_pairing_lease(user_id)
                            .unwrap()
                            .unwrap(),
                    })
                },
            )
            .unwrap();

        let claimed = store
            .claim(
                now(),
                started.capability_id,
                ClientKeyVerifier::from_bytes([1; 32]),
                |_| {
                    Ok::<_, ()>(ClaimedPairing {
                        user_id,
                        permission: ClientPermission::BalancesRead,
                    })
                },
            )
            .expect("an approved unencrypted development pairing should be claimable");

        assert_eq!(claimed.user_id, user_id);
        assert_eq!(claimed.permission, ClientPermission::BalancesRead);
    }

    #[test]
    fn claims_require_the_reserved_key_and_complete_exactly_once() {
        let store = PairingStore::new();
        let started = store
            .start(
                now(),
                source(1),
                request("claim", 1).validate().unwrap(),
                generated(1, 1),
                |_| Ok::<_, ()>(false),
            )
            .unwrap();
        let verifier = ClientKeyVerifier::from_bytes([1; 32]);
        assert_eq!(
            store.claim(now(), started.capability_id, verifier, |_| {
                Ok::<_, ()>(ClaimedPairing {
                    user_id: UserId::new(),
                    permission: ClientPermission::BalancesRead,
                })
            }),
            Err(PairingClaimError::Pending)
        );
        assert_eq!(
            store.claim(
                now(),
                started.capability_id,
                ClientKeyVerifier::from_bytes([2; 32]),
                |_| Err::<ClaimedPairing, _>(())
            ),
            Err(PairingClaimError::Unauthorized)
        );

        let code = format_code(&started.code);
        let user_id = UserId::new();
        store
            .approve(now(), started.capability_id, &code, None, || {
                Ok::<_, ()>(ApprovedPairingBinding {
                    user_id,
                    dek: Some(Dek::from_bytes([3; 32])),
                    lease: crate::auth::lifecycle::acquire_pending_pairing_lease(user_id)
                        .unwrap()
                        .unwrap(),
                })
            })
            .unwrap();

        let mut activations = 0;
        let claimed = store
            .claim(now(), started.capability_id, verifier, |claim| {
                activations += 1;
                assert_eq!(claim.capability_id, started.capability_id);
                assert_eq!(claim.user_id, user_id);
                assert_eq!(claim.client_name, "claim");
                Ok::<_, ()>(ClaimedPairing {
                    user_id: claim.user_id,
                    permission: claim.permission,
                })
            })
            .unwrap();
        assert_eq!(claimed.user_id, user_id);
        assert_eq!(activations, 1);
        let retry = store
            .claim(now(), started.capability_id, verifier, |_| {
                activations += 1;
                Err::<ClaimedPairing, _>(())
            })
            .unwrap();
        assert_eq!(retry, claimed);
        assert_eq!(activations, 1);
    }
}

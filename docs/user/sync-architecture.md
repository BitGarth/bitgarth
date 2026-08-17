# Sync Architecture

How BitGarth synchronizes blockchain data from external providers, normalizes it into canonical state, and projects it into user-visible read models.

## Overview

Sync turns external provider data into the transaction ledger the UI displays. The pipeline is layered:

```
  Layer 3 (UI reads)    account_transaction_ledger
                             ▲ rebuilt from
  Layer 2 (structured)  chain_transactions + inputs/outputs/utxos
                             ▲ reconciled from
  Layer 1 (raw archive) raw_*_transaction_versions
                             ▲ parsed from
  Layer 0 (wire)        Provider API JSON response
```

All tables live in the **user database** (per-user SQLite).

Sync is driven by a shared **breadth-first planner** that selects the next bounded sync iteration for the active sync-slot accounts. Both automatic background sync and `BITGARTH_SYNC_CONTROL` use the same planner, the same provider iteration boundaries, and the same per-address failure handling. They differ only in run budget, ledger rebuild timing, and the developer-facing summary they return.

---

## End-to-End Flow

```
 USER ADDS A BLOCKCHAIN ADDRESS
 ==============================

 ┌─────────────────────────────────────────────────────────────┐
 │  UI: Add Address Modal                                      │
 │  (src/components/wallets/add_bitcoin.rs, add_ethereum.rs)   │
 │                                                             │
 │  User provides:                                             │
 │   - blockchain address text                                 │
 │   - new wallet (with label) or existing wallet              │
 └──────────────────────┬──────────────────────────────────────┘
                        │ spawn(add_address(request))
                        ▼
 ┌─────────────────────────────────────────────────────────────┐
 │  Server Function: add_*_address()                           │
 │  (src/backend/wallets/handlers_write.rs)                    │
 │                                                             │
 │  1. Validate session → get user_id                          │
 │  2. Validate request → ValidatedAdd*AddressRequest          │
 │  3. Call add_*_address_db() ────────────────────────────┐   │
 │  4. Call enqueue_automatic_add_sync() ──────────────┐   │   │
 └─────────────────────────────────────────────────────┼───┼───┘
                                                       │   │
         ┌─────────────────────────────────────────────┘   │
         │  SYNC TRIGGER                                   │
         ▼                                                 │
 ┌───────────────────────────────────┐                     │
 │  enqueue_automatic_add_sync()     │                     │
 │  (src/backend/wallets/helpers.rs) │                     │
 │                                   │                     │
 │  Enqueues a                       │                     │
 │  UserTransactionMonitor job       │                     │
 │  into the task manager            │                     │
 │                                   │                     │
 │  (suppressed when                 │                     │
 │   BITGARTH_SYNC_CONTROL=1)        │                     │
 └───────────┬───────────────────────┘                     │
             │                                             │
             │                    ┌────────────────────────┘
             │                    │  DB SAVE (user db)
             │                    ▼
             │  ┌───────────────────────────────────────────────────┐
             │  │  add_*_address_db()                               │
             │  │  (src/db/wallets/single_address.rs)               │
             │  │                                                   │
             │  │  Creates/retrieves in USER database:              │
             │  │   ┌───────────────────────────────────────┐       │
             │  │   │ wallets (new or existing)             │       │
             │  │   ├───────────────────────────────────────┤       │
             │  │   │ digital_asset_accounts                │       │
             │  │   ├───────────────────────────────────────┤       │
             │  │   │ digital_asset_addresses               │       │
             │  │   └───────────────────────────────────────┘       │
             │  │                                                   │
             │  │  Returns: wallet_id, account_id, address_id       │
             │  └───────────────────────────────────────────────────┘
             │
             │  No syncing has happened yet. Data is saved.
             │  The sync runs asynchronously below.
             │
             ▼
 ════════════════════════════════════════════════════════════════
         SYNC JOB (runs asynchronously in task manager)
 ════════════════════════════════════════════════════════════════

 ┌──────────────────────────────────────────────────────────────┐
 │  run_sync_cycle()                                            │
 │  (src/tasks/jobs/sync/automatic.rs)                          │
 │  Integration fan-out: src/tasks/jobs/sync/parent_cycle.rs    │
 │                                                              │
 │  ┌── PLANNER LAYER ──────────────────────────────────────┐   │
 │  │  Sort non-HD addresses + HD bundles by priority tier  │   │
 │  │  (priority_tier_for_address, planner.rs)              │   │
 │  └───────────────────────────────────────────────────────┘   │
 │                         │                                    │
 │                         ▼                                    │
 │  For each address in planner order:                          │
 │                                                              │
 │    ┌── GATE LAYER ────────────────────────────────────────┐  │
 │    │  Cooldown check    → skip if too recent              │  │
 │    │  Rate-limit check  → skip if provider limited        │  │
 │    │  Tip-unchanged     → skip if no new blocks           │  │
 │    │  Inter-address     → pacing delay between addresses  │  │
 │    └──────────────────────────────────────────────────────┘  │
 │                         │                                    │
 │                         ▼                                    │
 │    ┌── EXECUTOR LAYER ───────────────────────────────────┐   │
 │    │  1. Resolve chain tip (cached or fresh)             │   │
 │    │  2. Compute sync plan (backfill vs incremental)     │   │
 │    │  3. Start raw sync_run                              │   │
 │    │  4. Provider performs ONE bounded iteration:        │   │
 │    │     - Mempool: 1 transaction page (or 1 stats call) │   │
 │    │     - Etherscan: 1 normal page + 1 internal page    │   │
 │    │  5. Complete raw sync_run                           │   │
 │    └─────────────────────────────────────────────────────┘   │
 │                         │                                    │
 │                         ▼                                    │
 │    ┌── PER-ADDRESS POST-SYNC ────────────────────────────┐   │
 │    │  Mark address sync success/failure                  │   │
 │    │  Update consecutive_failure_count                   │   │
 │    │  Publish sync events                                │   │
 │    │  If reconciled txs → rebuild account ledger         │   │
 │    └─────────────────────────────────────────────────────┘   │
 │                                                              │
 │  HD bundles: gap-limit aware traversal of external/internal  │
 │  chains, bounded by MAX_ADDRESSES_PER_ACCOUNT_PER_RUN = 200. │
 └──────────────────────────────────────────────────────────────┘
                        │
                        ▼
 ┌──────────────────────────────────────────────────────────────┐
 │  UI: Account Transactions Page                               │
 │                                                              │
 │  Reads from account_transaction_ledger:                      │
 │   - Balance = resulting_balance of the latest entry          │
 │   - Transaction list = all ledger rows for the account       │
 └──────────────────────────────────────────────────────────────┘
```

---

## Shared Concepts

### Sync Triggers

Sync runs can be triggered by:

- **Scheduled** (`TriggerSource::Schedule`): periodic background job.
- **Auto-add** (`TriggerSource::AutoAdd`): enqueued immediately when a user adds a new address.
- **Auto-session-restore** (`TriggerSource::AutoSessionRestore`): triggered on user login.
- **Auto-freshness** (`TriggerSource::AutoFreshness`): triggered when the UI detects stale data.
- **Manual** (`TriggerSource::ManualInternal`): sync control developer feature.

### Sync Control Mode

The environment variable `BITGARTH_SYNC_CONTROL=1` enables a developer-only mode. It shares the planner, gate, executor, and provider iteration logic with automatic sync, and only differs where automatic sync would batch for performance.

Intentional differences from automatic sync:

| Concern | Automatic | Sync Control |
|---------|-----------|-------------|
| Loop termination | Stop conditions or run budget | Exactly N iterations (developer-supplied), or first error/rate-limit |
| Address selection | Sort once up front by planner priority | Re-run `pick_next_address_index` before each iteration |
| Ledger rebuild | After each non-HD address that reconciled transactions; once per HD bundle | After every transaction iteration that reconciled transactions |
| Auto-add trigger | Active | Suppressed |
| Schedule hints | Computed | Not used |
| Return value | Aggregated cycle summary + schedule hint | Per-invocation summary (`SyncControlInvocationResponse`) |

Everything else — planner priority tiers and decisions, provider planning, entitlement gates, raw `sync_run` lifecycle, parsing, reconciliation, per-address sync state writes, and failure-count rules — is identical.

Config module: `src/sync_control.rs`

### Provider Dispatch

`LiveAddressSyncExecutor` owns concrete `MempoolAddressSyncIntegration` and `EtherscanAddressSyncIntegration` fields. Exhaustive `SyncProviderId` matches select the provider for execution; the provider-neutral estimate and unfinished-backfill helpers use the same closed-set dispatch. Each provider implements the shared `AddressSyncIntegration` trait:

```rust
pub(crate) trait AddressSyncIntegration {
    fn sync_plan(&self, address, allow_early_exit) -> Result<IntegrationSyncPlan>;
    fn estimate_first_sync_tx_count(&self, context) -> Result<Option<TxCountEstimate>>;
    fn unfinished_backfill_state(&self, address) -> Option<AddressBackfillState>;
    fn sync_one_iteration(&mut self, context) -> Result<SyncIterationResult>;
    fn current_run_summary_json(&self) -> Result<Option<OpaqueJsonText>>;
    fn reset_iteration_state(&mut self);
}
```

Supported providers:

| Provider | Asset | Module |
|----------|-------|--------|
| Mempool | Bitcoin | `src/tasks/jobs/sync/integrations/mempool/` |
| Etherscan | Ethereum | `src/tasks/jobs/sync/integrations/etherscan/` |

Asset-to-provider mapping: `src/asset_capabilities/mod.rs::default_sync_provider`.

`sync_one_iteration()` is the sole provider execution method and `SyncIterationResult` is its sole success result. One call performs one bounded transaction iteration and then returns control to orchestration.

---

## The Planner

Module: `src/tasks/jobs/sync/planner.rs`

The planner is a (mostly) pure function that takes the current sync workset plus a snapshot of side inputs and returns one `PlannedSyncIteration`. It is invoked in three modes:

1. **Sort** — `sort_addresses_by_planner_priority` / `sort_hd_bundles_by_planner_priority` order the workset for automatic sync's loop.
2. **Pick** — `pick_next_address_index` returns the index of the next address to sync, used by sync control before each iteration.
3. **Plan** — `plan_next_iteration` returns either an `Execute`, `DeriveHdAddresses`, or `Stop` decision.

All three entry points share the same candidate classification and priority logic.

### Inputs

`SyncPlannerInput` carries everything the planner needs without doing I/O of its own:

```
 PLANNER INPUTS
 ══════════════

   workset:        non-HD SyncAddress[]     HD AccountSyncBundle[]
                          │                          │
                          ▼                          ▼
   side inputs:    pending_address_ids
                   known_activity_address_ids
                   account_transaction_counts (canonical)
                   run_excluded_address_ids
                   transaction_cap (entitlement)
                   historical_backfill_enabled
                   now_utc
                          │
                          ▼
                  ┌──────────────────┐
                  │  plan_next_      │      ┌─ Execute
                  │  iteration()     │ ───▶ ├─ DeriveHdAddresses
                  └──────────────────┘      └─ Stop { reason }
```

The active sync-slot account set is enforced by the caller before populating the workset; the planner never widens scope past it.

`Execute` and `DeriveHdAddresses` are fieldless decisions. Priority is computed from workset state and the side inputs above; provider-specific backfill state remains persisted on the selected address. After selection, the concrete provider derives its `IntegrationSyncPlan` from that state.

### Priority Tiers

`SyncPlannerPriorityTier` is a single ordered enum used for both single-address accounts and HD bundles. Lower variants run first.

```
 PRIORITY ORDER (best → worst)
 ═════════════════════════════

  1. ActiveUnfinishedBackfill    resume in-progress history fetch
  2. PendingTransactionRefresh   refresh an address with pending transactions
  3. NeverAttemptedFirstSync     first transaction-history attempt
  4. RetryableFailedAttempt      retry an eligible failed address
  5. KnownActivityRefresh        refresh an address known to have activity
  6. BalanceRefresh              perform eligible balance-only work
  7. ColdRefresh                 refresh remaining eligible addresses
  8. HdDerivation                continue HD discovery after address work

 Within a tier: oldest last_completed_at wins, then deterministic
 tie-breaker on address_id (or `account_id:change` for HD derivation).
```

Balance-only eligibility is evaluated before first transaction-history sync for each address. Therefore a never-attempted address whose entitlement is balance-only, or whose account has reached its history cap, can classify as `BalanceRefresh`; this ordering is intentional.

Pending refresh stays ahead of first sync because a confirmed transition is immediately user-visible and can change balances/classification. HD derivation is normally lowest, but a brand-new bundle with no derived external/internal addresses produces an HD derivation candidate so the planner can bootstrap the frontier.

### Stop Reasons

`plan_next_iteration` returns `Stop { reason }` when no candidate remains:

```
 STOP REASONS
 ════════════

 NoEligibleAction        no candidates at all
 OnlyBlockedActions      every candidate is rate-limited / over failure threshold
 BalanceRefreshesFresh   only balance work exists and every balance is within TTL
```

Automatic sync stops on any of these. Sync control stops at the requested iteration count, on the first `Stop`, or on the first error/rate-limit.

### Entitlements

Before classifying an address for transaction-history work, the planner consults:

1. `historical_backfill_enabled` for the user's plan.
2. The plan's `historical_backfill_transactions_per_account` cap.
3. The canonical `account_transaction_counts` snapshot loaded from chain/account ownership tables (not the ledger projection, which can lag rebuilds).

If the cap is reached, the planner classifies the address in the `BalanceRefresh` tier instead. If a bounded transaction iteration crosses the cap mid-run, the iteration finishes and the next planner pass observes the updated count and switches to balance-only.

### Failure Handling

The planner uses durable per-address `consecutive_failure_count` (added by user migration `V30__transaction_sync_failure_count.sql`).

```
 FAILURE COUNT RULES
 ═══════════════════

 +1   address-scoped real failure
        (provider response/parse/deserialize that left the iteration incomplete)
   0   reset after any successful iteration on this address
  +0   provider rate limit
  +0   planner skip / no-op
  +0   tip-unchanged or fresh-enough decision
 ✕     DB / invariant errors (fatal, not counted)

 ≥ 2  blocked for the rest of the conceptual run.
       Reset by a successful iteration in a later run after the existing
       failure cooldown / backoff has elapsed.
```

This gives starvation-resistant retry: one bad address can't loop forever inside a run, but it also can't permanently block its peers in the same account.

### Balance Freshness

An address in the `BalanceRefresh` tier is considered fresh when `now_utc - last_completed_at < BALANCE_REFRESH_TTL` (currently `30 minutes`). The TTL is longer than provider cooldowns to avoid churn and short enough to fit normal sync cadence. When all eligible balance refreshes are fresh and no transaction work remains, the planner returns `Stop { reason: BalanceRefreshesFresh }` so free/balance-only runs settle quickly.

---

## Bounded Provider Iterations

Both providers expose the same shape: one `sync_one_iteration()` call performs a single bounded fetch unit, persists raw and canonical state, advances its cursor, and returns `SyncIterationResult`. There is no internal multi-page loop.

```
 ONE BOUNDED ITERATION (shared shape)
 ════════════════════════════════════

 ┌──────────────────────────────────────────────────────────────┐
 │  1. FETCH                                                    │
 │     Mempool:   1 page of /api/address/{addr}/txs[/chain/..]  │
 │     Etherscan: 1 page of txlist + 1 page of txlistinternal   │
 │                  (page=1, offset=1000, same block window)    │
 └────────────────────┬─────────────────────────────────────────┘
                      ▼
 ┌──────────────────────────────────────────────────────────────┐
 │  2. PERSIST (raw archive)                                    │
 │     Compute SHA256 of each tx payload                        │
 │     Deduplicate against existing raw versions                │
 │     Insert into raw_*_transaction_versions (user db)         │
 │     Link via raw_observation_sets → sync_run                 │
 └────────────────────┬─────────────────────────────────────────┘
                      ▼
 ┌──────────────────────────────────────────────────────────────┐
 │  3. MAP                                                      │
 │     Parse raw JSON → provider-specific typed structs         │
 │     Transform to canonical Vec<SyncTransactionRecord>        │
 └────────────────────┬─────────────────────────────────────────┘
                      ▼
 ┌──────────────────────────────────────────────────────────────┐
 │  4. RECONCILE                                                │
 │     Upsert into chain-level tables (user db):                │
 │      ┌────────────────────────────────────────────────────┐  │
 │      │ chain_transactions    (one row per unique tx)      │  │
 │      │ transaction_inputs    (owned-address inputs)       │  │
 │      │ transaction_outputs   (owned-address outputs)      │  │
 │      │ utxos                 (current unspent outputs)    │  │
 │      └────────────────────────────────────────────────────┘  │
 └────────────────────┬─────────────────────────────────────────┘
                      ▼
 ┌──────────────────────────────────────────────────────────────┐
 │  5. UPDATE CURSOR                                            │
 │     Persist resume cursor (survives interruption)            │
 │     Return SyncIterationResult:                              │
 │       { new_tx_count, updated_tx_count, has_more_work,       │
 │         early_exited, tip_height,                            │
 │         api_confirmed_balance? }                             │
 └──────────────────────────────────────────────────────────────┘
```

`has_more_work=true` keeps the concrete provider's in-memory iteration state for the same provider/address. Exhaustion, provider error, or an address change clears that state. It never creates an internal provider loop; automatic and HD orchestration still perform one bounded iteration per selected address step.

---

## Mempool (Bitcoin)

Module: `src/tasks/jobs/sync/integrations/mempool/`

### Transaction Iteration

Normal Bitcoin history work starts with one `/api/address/{addr}` statistics
request for the visit. That single observation drives current confirmed
balance, provider transaction count, HD discovery, zero-address avoidance, and
progress. A fresh zero/zero observation proves the address empty without a
`/txs` request.

When history is needed, one iteration = one Mempool transaction page:

- First page: `GET /api/address/{addr}/txs` (most recent confirmed transactions first)
- Subsequent pages use either
  `GET /api/address/{addr}/txs/chain/{last_txid}` or
  `GET /api/address/{addr}/txs?after_txid={last_txid}`. The client probes once
  and caches the working style for that host.

Pages return up to 25 confirmed transactions. Backfill mode walks backward
through history; incremental mode short-circuits when an entire page of
confirmed transactions is already known.

```
 MEMPOOL TRANSACTION ITERATION
 ═════════════════════════════

   one planner-selected iteration  ──▶  one HTTP page
   ┌─────────────┐
   │ 25 txs (or  │── persist raw / map / reconcile ──┐
   │ < 25 if end)│                                   │
   └─────────────┘                                   ▼
        │                              update mempool_backfill_cursor
        │
        ▼
   has_more_work = (cursor remains && not early-exit)
   Subsequent iterations selected by the planner advance the cursor.

 Backfill cursor: transaction_sync_state.mempool_backfill_cursor_txid
 Early exit (incremental only): an entire page of confirmed txs that
 are already known.
```

### Balance Refresh

Every successful statistics observation atomically replaces both limbs of the
API-confirmed balance. A missing provider balance clears both limbs; a failed
request preserves the previous successful observation. Current-balance reads
aggregate the complete relevant address set, so one missing observation makes
the account and its wallet/fiat aggregates unavailable. This current balance
is independent of historical coverage and excludes pending transactions.

### Raw Storage

Each transaction's JSON is stored individually in `raw_mempool_transaction_versions`:

```
 MEMPOOL RAW JSON PARSING
 ════════════════════════

 Response bytes:  [{tx1}, {tx2}, {tx3}]
                    │
 Step 1: serde_json::from_slice → Vec<Box<RawValue>>
         (parses array structure, keeps each element as
          raw JSON text — NOT fully deserialized)
                    │
 Step 2: For each RawValue:
         - raw_value.get() → "&str" of that one tx object
         - Deserialize to provider tx struct
           (to validate + extract txid)
         - Store raw_json.as_bytes().to_vec() as payload
                    │
 Result per tx: MempoolPageTransaction {
     txid: TxHash,
     payload_bytes: Vec<u8>   ← raw JSON of single tx
 }

 The array scaffolding ([], commas) is stripped by serde's
 RawValue parser — each tx's JSON is stored individually.

 ┌────────────────────────────────────────────────────┐
 │ raw_mempool_transaction_versions                   │
 │  id, txid, network                                 │
 │  payload_bytes (BLOB) ← the raw JSON of one tx     │
 │  payload_hash_sha256_hex                           │
 ├────────────────────────────────────────────────────┤
 │ raw_mempool_transaction_observations               │
 │  links version → observation_set → sync_run        │
 ├────────────────────────────────────────────────────┤
 │ raw_observation_sets                               │
 │  metadata about the API page fetch                 │
 └────────────────────────────────────────────────────┘

 Deduplicated by: source_connection_id + txid + payload_hash_sha256_hex
```

### Coverage proofs, caps, and historical balances

Normal sync stores a per-address proof pair: confirmed transaction count and
the run's Mempool tip height. The pair is either fully present or fully null.
An account is complete only when every relevant address is proven at a common
coverage height and discovery is complete. A newly observed count
contradiction invalidates proof and clears published closing balances before a
replacement can publish.

`history.max_transactions_per_account` is a soft, account-wide distinct
transaction cap checked after each complete page. The page that reaches or
crosses the cap is retained. For HD accounts the durable next-address frontier
advances to the following address and no later address is requested in that
run; increasing the cap resumes there. Single-address accounts use only their
address cursor and never create `account_sync_state` solely for paging.

Incomplete normal history may have provisional synthetic closing balances when
all inputs are known. Complete history is rebuilt forward from canonical zero
with checked arithmetic and deterministic same-block dependency ordering.
Pending Bitcoin rows never receive closing balances. Dated reads—including all
three boundary queries and wallet reports—require complete coverage; current
wallet reads continue to use the independent provider balance. For incomplete
Bitcoin accounts, hledger keeps postings and persisted transaction/API provider
assertions for the available history window. Any window with persisted
transaction closing balances receives strict chain validation, so mixed
asserted/unasserted native evidence is rejected. Only separate year-opening and
year-closing journals are suppressed until coverage is complete.

### Strict post-V49 evidence

Every successful nonempty or empty Mempool page is recorded as one atomic raw
observation set in the encrypted user database. Legacy repair tags the whole
resumable scan with one start-run ID. A nonzero proof can publish only after an
empty terminal page, a complete advancing cursor chain, exact equality with
the fresh provider count, and exact equality between the observed and
canonical confirmed transaction sets touching the address. Missing retained
evidence, a duplicate cursor, parse failure, or either count/set mismatch
restarts the strict scan and leaves history unavailable.

---

## Etherscan (Ethereum)

Module: `src/tasks/jobs/sync/integrations/etherscan/`

### Transaction Iteration

One iteration = one stable block-range window with two paired API calls, each capped at one HTTP page:

1. Normal: `GET ?module=account&action=txlist&address={addr}&startblock={start}&endblock={end}&page=1&offset=1000`
2. Internal: `GET ?module=account&action=txlistinternal&address={addr}&startblock={start}&endblock={end}&page=1&offset=1000`

Both calls share the same `(start_block, end_block)`, so the normal and internal results form a coherent logical pair. Native ETH correctness depends on both families being reconciled together.

```
 ETHERSCAN TRANSACTION ITERATION
 ═══════════════════════════════

 Chain:  [block 0] ─────────────────────────────── [block tip]

 One planner-selected iteration fetches ONE window:
 ┌───────────────────────────────────────────────┐
 │  block range [start .. end]                   │
 │                                               │
 │  txlist          (page=1, offset=1000)        │
 │  txlistinternal  (page=1, offset=1000)        │
 └───────────────────────────────────────────────┘
                    │
                    ▼
            persist raw normal / persist raw internal
            map (combine normal + internal)
            reconcile chain_transactions
                    │
                    ▼
   has_more_work = either page returned a full 1000 results
                   (we may have skipped older transactions in
                    that window)

 If full: the resume cursor is set to the oldest block on the
 fuller page, and the next iteration uses that as the new
 end_block. If both pages were partial: backfill complete,
 cursor cleared.

 Resume cursor column: transaction_sync_state.etherscan_backfill_end_block
```

There is no provider-internal loop over the historical 10-page / 10,000-result page window. Each iteration intentionally fetches at most one HTTP page per family; reaching that boundary persists a resume cursor and yields control back to the planner so entitlements and other addresses can be re-evaluated between iterations.

### Cursor Semantics

A single `etherscan_backfill_end_block` column tracks resume position. When one stream (normal vs internal) is denser than the other, the cursor advances to the safer block boundary and the next iteration re-fetches the sparser stream over the narrower window. Reconciliation is idempotent, so duplicate observations within a window do not corrupt state.

### Balance Refresh

When a cycle selects a balance refresh for an Ethereum address, the Etherscan provider calls the native balance endpoint once and persists API-confirmed balance only. It does **not** imply that history is continuous; the provider-specific persisted history state remains authoritative for whether the historical sync is complete.

### Raw Storage

Normal and internal transactions are stored in separate raw tables:

- `raw_etherscan_normal_tx_versions`
- `raw_etherscan_internal_tx_versions`

Deduplicated by content hash per source connection.

---

## HD / xpub Breadth-First Behavior

Module: `src/tasks/jobs/sync/hd_scan.rs`

HD/xpub discovery is statistics-first and completes external and internal
chains independently. History runs in breadth rounds: each active relevant
address receives at most one page before any address receives the next page.
The durable next-address history frontier belongs only to HD accounts and
preserves this order across process restarts and provider-wide rate-limit
backoff.

```
 HD BUNDLE FLOW
 ══════════════

 ┌────────────────────────────────────────────────────────────┐
 │  External chain (change=0)        Internal chain (change=1)│
 │                                                            │
 │  derived addr 0 ─┐                derived addr 0 ─┐        │
 │  derived addr 1 ─┤ statistics,    derived addr 1 ─┤        │
 │  ...            ─┤ then one-page  ...            ─┤        │
 │  derived addr K ─┘ breadth rounds derived addr K ─┘        │
 │                                                            │
 │  Frontier: derive next batch when consecutive_unused < gap │
 │  limit and the planner runs out of address-level work.     │
 └────────────────────────────────────────────────────────────┘

 Gap-limit invariant preserved:
   - keep deriving while consecutive_unused < gap_limit
   - reset consecutive_unused on any address with activity
   - active rescan is an explicit phase in HdAccountChainSyncState

 Cap: MAX_ADDRESSES_PER_ACCOUNT_PER_RUN = 200 addresses processed per
      account per run. When hit, the bundle yields control with its
      durable HD frontier state intact.
```

Single-address accounts retain the shared planner priority logic but page only
with their address cursor. They do not synthesize the HD account-local
next-address frontier.

---

## Orchestration

### Automatic Sync Cycle

File: `src/tasks/jobs/sync/automatic.rs` — `run_sync_cycle()`

```
 AUTOMATIC SYNC CYCLE
 ════════════════════

 1. Preload settings, entitlements, sync slots, account labels,
    pending and known-activity address sets, account transaction counts.
 2. Build SyncPlannerInput.
 3. sort_addresses_by_planner_priority(non_hd)
    sort_hd_bundles_by_planner_priority(hd_bundles)
 4. plan_next_iteration() once for telemetry / first decision.
 5. For each non-HD address (in planner order):
      a. Gate (cooldown, rate limit, tip unchanged, inter-address pacing)
      b. Executor dispatches ONE bounded provider iteration
      c. Update consecutive_failure_count + sync state
      d. Publish progress / completion / failure events
      e. If transactions were reconciled → rebuild the account ledger before yielding
 6. For each HD bundle (in planner order):
      a. run_hd_bundle_scan() — gap-limit aware breadth rounds
      b. Bounded by MAX_ADDRESSES_PER_ACCOUNT_PER_RUN
      c. Rebuild once per completed breadth round or interruption yield
 7. Compute UserTransactionMonitorScheduleHint for the next cycle.
```

The visible ledger is published atomically with its completeness state.
Automatic and manual sync share the same cap, breadth ordering, and
provider-wide interruption policy.

### Sync Control Loop

File: `src/tasks/jobs/sync/manual_control.rs` — `run_manual_sync_control()`

```
 SYNC CONTROL FLOW (BITGARTH_SYNC_CONTROL=1)
 ═══════════════════════════════════════════

 ┌──────────────────────────────────────────────────────────────┐
 │  UI: Sync Control Card on Account Transactions Page          │
 │  Developer enters: Run [N] sync iteration(s) → Submit        │
 └──────────────────────┬───────────────────────────────────────┘
                        │ POST /_app/user/account/:id/sync-control/run
                        ▼
 ┌──────────────────────────────────────────────────────────────┐
 │  run_manual_sync_control()                                   │
 │                                                              │
 │  1. Validate sync control enabled (else 403)                 │
 │  2. Validate session + account ownership (else 404)          │
 │  3. Confirm account is in an active sync slot                │
 │  4. Load account addresses, entitlements, side inputs        │
 │                                                              │
 │  ┌─ ITERATION LOOP (up to N) ────────────────────────────┐   │
 │  │                                                       │   │
 │  │  Reload canonical account transaction count           │   │
 │  │  Build SyncPlannerInput                               │   │
 │  │    │                                                  │   │
 │  │    ▼                                                  │   │
 │  │  pick_next_address_index()                            │   │
 │  │    │ if None → break (no eligible work)               │   │
 │  │    ▼                                                  │   │
 │  │  Re-check entitlement cap for the chosen address      │   │
 │  │    │                                                  │   │
 │  │    ▼                                                  │   │
 │  │  sync_single_address_with_controls()                  │   │
 │  │    │ same gate + executor as automatic                │   │
 │  │    ▼                                                  │   │
 │  │  refresh_account_integration_sync_state               │   │
 │  │  if a non-empty tx batch reconciled → rebuild ledger  │   │
 │  │  publish events                                       │   │
 │  │                                                       │   │
 │  │  on error or rate-limit → stopped_early = true; break │   │
 │  └───────────────────────────────────────────────────────┘   │
 │                                                              │
 │  5. Return SyncControlInvocationResponse (per-invocation     │
 │     summary: iterations completed, totals, stopped_early,    │
 │     backfill_continuing, optional error message).            │
 └──────────────────────────────────────────────────────────────┘
```

Each loop turn refreshes the planner side inputs from the DB (account transaction count, addresses), so cap fallback to balance-only is observed within the same invocation. Only an iteration reconciling a non-empty transaction batch requests the ordinary ledger rebuild, letting developers inspect partial chain/ledger state between those iterations without waiting for an automatic cycle to converge.

### Executor Layer

File: `src/tasks/jobs/sync/executor.rs`

The `AddressSyncExecutor` trait provides one `sync_one_iteration` test seam. The production `LiveAddressSyncExecutor` owns concrete Mempool and Etherscan integrations and selects between them with an exhaustive `SyncProviderId` match. It:

1. Resolves the chain tip (from cache or fresh API call).
2. Computes the sync plan (backfill vs. incremental).
3. Starts a raw `sync_run` record.
4. Delegates to the provider's sole bounded execution method, `sync_one_iteration`.
5. Persists the observed chain tip for a successful `SyncIterationResult`.
6. Completes the raw `sync_run` with success/failure status and the latest available summary.

The executor owns raw `sync_run` lifecycle — providers do not create or complete `sync_runs` directly. A raw-run completion failure turns a provider success into an error; if provider execution and raw-run completion both fail, the provider error remains primary and the completion error is logged as secondary evidence. The executor retains provider iteration state only while the same provider/address returns `has_more_work`; it clears that state on exhaustion, provider error, raw-run completion failure, or address change.

### Gate Layer

File: `src/tasks/jobs/sync/gate.rs`

Before dispatching an address iteration, the gate layer applies:

- **Cooldown**: per-provider delay after the last sync completion (success or failure). Prevents hammering providers.
- **Rate-limit check**: skips addresses while the user's integration-level limit for that provider is active.
- **Tip-unchanged gate** (Mempool only): if the chain tip has not changed since the last sync and no pending transactions exist, the address is skipped. If pending transactions exist, the address is synced for a pending-refresh pass.

After sync:

- On rate-limit errors: records one user/integration limit with retry-after duration. The attempt remains a scheduling failure but does **not** increment `consecutive_failure_count`.
- On other failures: records failure state and increments `consecutive_failure_count` if the failure is address-scoped.

### Chain tips

File: `src/tasks/jobs/sync/chain_tip.rs`

Each Bitcoin sync run makes one fresh chain-tip request to its configured
Mempool provider and reuses that value throughout the run. Persisted tips from
another provider are scheduling state, not proof telemetry.

```
 BITCOIN / MEMPOOL TIP RESOLUTION
 ════════════════════════════════

 resolve_for_bitcoin_run(network, configured Mempool provider)
      │
      ▼
 ┌─ Fresh API fetch ──────────────────────┐
 │  GET /api/blocks/tip/height            │
 │  Cache only for this Bitcoin run       │
 └────────────────────────────────────────┘
```

Ethereum follows a separate TTL path. It may reuse an in-memory or persisted
Ethereum tip while that value remains fresh under
`chain_tip_cache_ttl_for(Ethereum)`; after the TTL expires, it fetches a fresh
tip through Etherscan.

---

## Canonical Reconciliation

After mapping provider-specific types to `SyncTransactionRecord`, reconciliation upserts into chain-level tables:

| Table | Contents |
|-------|----------|
| `chain_transactions` | One row per unique (asset, network, tx_hash). Status, block height, fees. |
| `transaction_inputs` | One row per owned-address input. |
| `transaction_outputs` | One row per owned-address output. |
| `utxos` | Current unspent transaction outputs. |

Reconciliation is idempotent — re-processing the same data produces the same result.

For Ethereum, the mapper combines normal and internal transactions into unified `SyncTransactionRecord` entries with transfer-level detail (nonce, transfer index).

---

## Derived Ledger Rebuild

File: `src/db/account_transactions/ledger_rebuild.rs` — `rebuild_account_transaction_ledger()`

The `account_transaction_ledger` is the table the UI reads from. It is a derived read model rebuilt from canonical chain tables:

```
 LEDGER REBUILD
 ══════════════

 ┌────────────────────────────────────────────────────────────┐
 │  rebuild_account_transaction_ledger()                      │
 │  (src/db/account_transactions/ledger_rebuild.rs)            │
 │                                                            │
 │  1. Load ALL chain_transactions for this account           │
 │  2. Order deterministically                                │
 │     (block_height, then provider-specific ordering)        │
 │  3. Compute running balances                               │
 │  4. DELETE + re-INSERT atomically:                         │
 │                                                            │
 │  ┌────────────────────────────────────────────────────┐    │
 │  │ account_transaction_ledger  ← THIS IS WHAT THE     │    │
 │  │                                UI READS FROM       │    │
 │  │  account_id, tx_hash, status                       │    │
 │  │  tx_type (receive/send/self_transfer)              │    │
 │  │  value_amount, fee_amount                          │    │
 │  │  resulting_balance  ← running balance after this tx│    │
 │  │  from_addresses_json, to_addresses_json            │    │
 │  │  occurred_at, block_height                         │    │
 │  └────────────────────────────────────────────────────┘    │
 └────────────────────────────────────────────────────────────┘

 Rebuild timing:

   AUTOMATIC SYNC                SYNC CONTROL
   ══════════════                ════════════
   non-HD addr 1 ──┐             iter 1 (tx) ──┐
   non-HD addr 2 ──┤             iter 2 (tx) ──┤
   non-HD addr 3 ──┘             iter N (tx) ──┘
        │                              │
   rebuild after                  rebuild after
   EACH non-HD address            EVERY iteration
   that reconciled txs;           that reconciled a non-empty
   ONCE per HD bundle             transaction batch
   that reconciled txs
        │                              │
        ▼                              ▼
   UI sees results               UI sees results
   per address /                 per iteration
   per HD bundle
```

The full-rebuild approach is correct by construction because running balances depend on the complete ordered transaction set. Partial updates would require detecting the earliest affected row and recomputing all downstream balances.

For sync control with a large HD account, running N iterations that reconcile non-empty transaction batches performs N full account ledger rebuilds. This is acceptable for developer-only visibility, but the cost is real and is logged.

### Future Optimization (Not Implemented)

A narrow append-only fast path is documented as a future optimization:

- Safe when: newly reconciled canonical transactions are provably newer than all existing confirmed ledger rows for the account and do not reorder pending rows.
- Mandatory fallback: full rebuild for every other case (backfill inserts, pending→confirmed transitions, reordering).

This is intentionally deferred because proving the safety preconditions is non-trivial.

---

## Raw Sync Run Lifecycle

File: `src/tasks/jobs/sync/executor.rs`

The `sync_runs` table is **raw-ingestion provenance**. It records one provider/address execution. It is intentionally **not** the conceptual account sync run — that concept is the in-memory loop above plus the per-address `transaction_sync_state` rows.

```
 RAW SYNC RUN LIFECYCLE
 ══════════════════════

 ┌─ start_sync_run() ────────────────────────────────┐
 │  Creates sync_runs row:                           │
 │    integration, scope_kind, scope_address_id,     │
 │    asset_id, network, trigger_kind, started_at    │
 │  Returns: sync_run_id, source_connection_id       │
 └────────────────────┬──────────────────────────────┘
                      │
                      ▼
 ┌─ Provider executes ONE bounded iteration ─────────┐
 │  Each iteration uses:                             │
 │    sync_run_id → links raw observations           │
 │    source_connection_id → scopes raw dedup        │
 └────────────────────┬──────────────────────────────┘
                      │
                      ▼
 ┌─ complete_sync_run() ─────────────────────────────┐
 │  Updates sync_runs row:                           │
 │    status: CompletedSuccess | CompletedFailure    │
 │    completed_at, summary_json (optional)          │
 └───────────────────────────────────────────────────┘

 Scope: ONE sync_run per provider invocation per address.
        A multi-iteration conceptual account run produces
        many sync_runs rows — one per bounded iteration.
```

---

## Event Publication

Sync publishes structured `TransactionSyncEvent` events at key lifecycle points:

- Account sync started (with first-sync flag, expected tx count).
- Account sync progress (fetched count vs. expected total).
- Account sync completed (new/updated tx counts).
- Account sync failed (error message, rate-limit details).
- Integration-level equivalents of the above.

These events fire on every successful iteration, so progress visibility tracks bounded iterations rather than full address completions. They are published for both automatic and manual sync, ensuring the same live-state behavior in the UI.

An automatic run publishes exactly one terminal event: `Failed` when the integration-workset aggregate is `Failure` and at least one address failed, and `Completed` otherwise. Aggregation is by integration workset, not by individual address: mixed success and failure within the sole provider's workset can aggregate to `Failure` and emit `Failed` despite partial address success. A provider error remains the primary iteration result if completing its raw `sync_run` also fails.

---

## Rate Limiting

File: `src/tasks/jobs/sync/rate_limit.rs`

Rate limits are tracked in process with one key per user and integration:
`user:{user_id}:integration:{label}`. A Mempool limit stops the provider-wide
run immediately, persists the current address/round frontier, and honors any
`retry_after` duration. Resume starts at that address before later addresses or
the next breadth round. A hit remains a scheduling failure but is explicitly
not counted as an address failure.

---

## V49 migration and one-off Bitcoin repair

V49 ships with the proof-aware readers and writers as one coordinated,
forward-only release. It preserves canonical transactions, raw observations,
provider balances, manual assertions, and non-Bitcoin rows; invalidates legacy
Bitcoin proof/projection state; and marks the network-managed
`bitcoin_history_full_resync_v1` repair pending.

The repair uses the normal Mempool sync implementation through the existing
backfill lane, but starts eligible legacy accounts from the first address/page,
ignores the normal transaction cap and active-account filter, and requires the
strict tagged evidence contract above. It is resumable across rate limits and
process restarts. Manual/address-scoped normal history is rejected for an
account while its repair lease is active. Accounts return to normal capped
operation individually after proof and ledger publication.

Provider outages leave the repair pending and historical balances unavailable.
Roll-forward is the primary recovery. An older binary cannot open V49;
backward recovery requires restoring the encrypted pre-V49 user-database
backup. No separate repair queue, worker, canary, or proof-provenance model was
added.

---

## Inter-Address Pacing

File: `src/tasks/jobs/sync/context.rs`

Between address syncs within a cycle, a configurable delay is applied per provider:

- Mempool: 250ms
- Etherscan: 500ms

The first address in a cycle skips the delay.

---

## Schedule Hints

After a sync cycle completes, the system computes a `UserTransactionMonitorScheduleHint` for the next cycle:

| Condition | Interval | Urgency |
|-----------|----------|---------|
| Rate limited | `retry_after` duration | Blocked |
| Unfinished work | 60s (minimum) | High |
| Idle (no activity) | 900s | Low |
| Default | 300s | Normal |

Sync control does not produce schedule hints; it returns its summary directly to the developer UI.

---

## Run Budget

Automatic sync currently reuses the existing `MAX_ADDRESSES_PER_ACCOUNT_PER_RUN = 200` budget on HD bundles (per account, per run). The non-HD path is bounded implicitly by the planner's stop conditions plus inter-address pacing and rate-limit/cooldown gates. Sync control's budget is the developer-supplied iteration count; it is not subject to the address-per-account cap.

---

## Key Code Locations

| Concern | File |
|---------|------|
| Sync cycle orchestration | `src/tasks/jobs/sync/automatic.rs` |
| Integration fan-out and reduction | `src/tasks/jobs/sync/parent_cycle.rs` |
| Sync control orchestration | `src/tasks/jobs/sync/manual_control.rs` |
| Shared planner | `src/tasks/jobs/sync/planner.rs` |
| Planner priority, iteration result, and stop types | `src/tasks/jobs/sync/context.rs` |
| Client config helpers | `src/tasks/jobs/sync/client_config.rs` |
| Account event helpers | `src/tasks/jobs/sync/account_events.rs` |
| Executor (sync run lifecycle) | `src/tasks/jobs/sync/executor.rs` |
| Gate (cooldown, rate limit, tip) | `src/tasks/jobs/sync/gate.rs` |
| Cycle accumulator | `src/tasks/jobs/sync/cycle.rs` |
| Chain tip cache | `src/tasks/jobs/sync/chain_tip.rs` |
| Rate limiting | `src/tasks/jobs/sync/rate_limit.rs` |
| Progress publishing | `src/tasks/jobs/sync/progress.rs` |
| HD scanning | `src/tasks/jobs/sync/hd_scan.rs` |
| Error types | `src/tasks/jobs/sync/error.rs` |
| Provider trait + direct provider helpers | `src/tasks/jobs/sync/integrations/mod.rs` |
| Mempool provider | `src/tasks/jobs/sync/integrations/mempool/mod.rs` |
| Mempool mapper | `src/tasks/jobs/sync/integrations/mempool/mapper.rs` |
| Mempool paginator | `src/tasks/jobs/sync/integrations/mempool/paginator.rs` |
| Etherscan provider | `src/tasks/jobs/sync/integrations/etherscan/mod.rs` |
| Etherscan mapper | `src/tasks/jobs/sync/integrations/etherscan/mapper.rs` |
| Raw ingestion | `src/tasks/jobs/raw_ingestion_executor.rs` |
| Canonical reconciliation | `src/db/transactions.rs` |
| Ledger rebuild | `src/db/account_transactions/ledger_rebuild.rs` |
| Sync control config | `src/sync_control.rs` |
| Sync control endpoint | `src/backend/sync_control.rs` |

Provider/address activity is not a transaction-ledger mutation. The current
provider balance can advance while capped historical ledger rows stay unchanged:
an account at `history.max_transactions_per_account` keeps refreshing its
statistics and current balance without fetching another transaction page, and
that refresh must not rewrite closing balances that are already correct.

Ordinary ledger rebuilding follows the transient `ledger_rebuild_required`
signal on `SyncIterationResult`, set only after an integration successfully
reconciles a non-empty transaction batch. `new_tx_count` and `updated_tx_count`
keep their reporting meaning and do not drive rebuilds. Coverage invalidation
and explicit data repair remain separate, independent rebuild causes.

---

## Delete and Re-Add Behavior

When an account is deleted, cascade deletes clean up most data:

| Table | Deleted? | Mechanism |
|-------|----------|-----------|
| `digital_asset_addresses` | Yes | CASCADE on `account_id` |
| `digital_asset_accounts` | Yes | Direct delete |
| `transaction_inputs` | Yes | CASCADE on `address_id` |
| `transaction_outputs` | Yes | CASCADE on `address_id` |
| `utxos` | Yes | CASCADE on `address_id` |
| `account_transaction_ledger` | Yes | CASCADE on `account_id` |
| `transaction_sync_state` | Yes | CASCADE on `address_id` |
| `account_sync_state` | Yes | CASCADE on `account_id` |
| `chain_transactions` | Orphans only | Cleanup removes txs with no remaining inputs/outputs |
| `source_connections` | **No** | Deactivated (status → `inactive`, `address_id` → NULL) |
| Raw transaction versions | **No** | FK to `source_connections` is `ON DELETE RESTRICT` |

When the same address is re-added:

```
 Re-add the same address
 ========================

 Source connection: reused (looked up by normalized_source_key,
                   reactivated if inactive)
 API pages:        re-fetched (sync state was deleted, starts from scratch)
 Raw table:        no duplicates (dedup finds existing rows via reused
                   source_connection_id)
 Chain tables:     re-populated from mapped raw data
 Ledger:           rebuilt after sync completes
```

The API re-fetch is unnecessary work but not harmful. The raw dedup prevents storage waste.

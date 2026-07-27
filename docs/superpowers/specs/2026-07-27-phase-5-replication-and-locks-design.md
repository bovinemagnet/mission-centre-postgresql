# Mission Centre PostgreSQL — Phase 5 Design

**Author:** Paul Snow
**Date:** 2026-07-27
**Version:** 0.0.0
**Status:** Approved — ready for implementation planning
**Licence:** GPL-3.0-or-later
**Parent spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`

---

## 1. Summary

Phase 5 adds the last two pages of the parent spec's page list: **Locks** and **Replication**.

The Locks page answers one question — who is blocked, and by whom — and answers it fast enough to be
worth asking. The Replication page answers a slower one: are the standbys keeping up, and is anything
retaining write-ahead log that nobody will ever consume.

The two pages sit at opposite ends of the cadence range, and that shapes the design more than any
other single fact. Contention is measured in seconds and is usually over before anyone looks at it,
so the blocked tree samples on the fast tier. Replication lag and slot retention move over minutes,
so replication samples on the slow tier. One query — the full lock inventory — is expensive enough
and niche enough that it samples only while its view is on screen, which introduces the first
visibility-gated query in the codebase.

### 1.1 Reinterpreting the parent spec

The parent spec lists Phase 5 in one line: "Replication and Locks pages." It says nothing about what
either contains. This design fills that in and, in one respect, goes further than the line implies:
the Locks page carries the cancel and terminate actions from Phase 4. Finding the backend that is
blocking production and then having to navigate to a different page to stop it is a poor flow, and
the machinery to do it in place already exists.

### 1.2 Prior art

Three earlier decisions are reused rather than reinvented:

- **Availability as a stated condition** (Phase 3). A missing extension or view is never a blank
  cell; it is a message naming what is missing. Phase 5 extends this to version-gated *columns*, not
  just whole features.
- **Per-action capabilities** (Phase 4, `src/connection/probe.rs`). The Locks page gates its actions
  on `Capabilities.signal_backend`. No second privilege notion is introduced.
- **Selection that survives a refresh** (Phase 4, `src/table/mod.rs`). A one-second refresh with a
  selected row is already solved; the Locks page uses the same by-key mechanism.

---

## 2. Scope

### 2.1 In scope

**The Locks page**, in two views:

| View | Source | Cadence |
|---|---|---|
| Blocked tree (default) | `pg_locks` ⋈ `pg_stat_activity` via `pg_blocking_pids()` | fast tier |
| Full inventory | `pg_locks` ⋈ `pg_stat_activity` | only while selected |

with cancel and terminate on the selected backend, reusing the Phase 4 action bar, capability gating,
confirmation dialogs and result toasts unchanged.

**The Replication page**, in four sections, laid out according to the server's own role:

| Section | Source | Shown when |
|---|---|---|
| Connected standbys | `pg_stat_replication` | server is a primary |
| Upstream receiver | `pg_stat_wal_receiver` | server is in recovery |
| Replication slots | `pg_replication_slots` | always |
| Logical replication | `pg_stat_subscription`, `pg_stat_subscription_stats`, `pg_publication` | subscriptions or publications exist |

Supporting work this requires:

- Two new `Snapshot` fields, carried exactly as `statements` and `relations` are today.
- A **visibility-gated query** mechanism for the lock inventory (§6.2).
- A **pure tree builder** for the blocked chains (§4.2).
- Version branching confined to the slot and subscription queries (§3.2).

### 2.2 Explicitly out of scope

Recorded so the decisions are not silently relitigated:

- **Replication in the history store.** Standby lag plotted over hours is genuinely useful and is a
  subsystem in its own right: new tables, new retention policy, new graphs, and a second writer
  against the Phase 3 schema. Phase 5 shows the present moment only.
- **Slot management.** No creating, dropping or advancing slots. `pg_drop_replication_slot()` is
  irreversible, and a slot that looks abandoned may be a standby that is merely down for maintenance.
  Dropping it destroys that standby's ability to catch up. It needs its own capability probe and
  confirmation copy, and belongs in a phase that can give it that attention.
- **Lock acquisition history.** `pg_locks` is instantaneous by construction. Reconstructing "who held
  this lock ten minutes ago" means sampling continuously and storing it — a different problem, and
  one the history store was not shaped for.
- **Deadlock detection.** PostgreSQL detects and resolves deadlocks itself, and logs them. Duplicating
  that here would be a worse implementation of a solved problem. The tree builder must merely not
  break when it samples a cycle mid-flight (§4.2).
- **Synchronous replication configuration.** Reading `sync_state` is in scope; changing
  `synchronous_standby_names` is not.

---

## 3. Data sources and version compatibility

### 3.1 The five queries

One query per section, each independently fallible (§8):

| Query | Returns |
|---|---|
| `locks_blocked` | one row per participating backend — waiters and their blockers |
| `locks_inventory` | one row per lock, bounded by a configurable limit |
| `replication_standbys` | one row per connected standby |
| `replication_slots` | one row per slot |
| `replication_logical` | subscriptions, their statistics where available, and publications |

`pg_stat_wal_receiver` and `pg_is_in_recovery()` are folded into the existing server-settings sample
rather than given a query of their own; both are single-row and cheap.

### 3.2 Version matrix

Verified empirically against `postgres:14` and `postgres:18` containers on 2026-07-27, not recalled:

| View | 14 | 18 | Consequence |
|---|---|---|---|
| `pg_stat_replication` | ✅ identical columns | ✅ | **no version branch needed** |
| `pg_stat_wal_receiver` | ✅ | ✅ | none |
| `pg_locks` | ✅ | ✅ | none |
| `pg_replication_slots` | through `two_phase` | adds `two_phase_at`, `inactive_since`, `conflicting`, `invalidation_reason`, `failover`, `synced` | branch |
| `pg_stat_subscription` | no `worker_type`, no `leader_pid` | both present | branch |
| `pg_stat_subscription_stats` | **absent** | ✅ | whole section conditional |

The slot query is written against the 14-era column set and extends it where the server is newer.
`inactive_since` deserves particular note: it is the column that answers "how long has this slot been
abandoned", which is the single most operationally useful fact about a slot, and it does not exist
before 16. On such servers the column shows an em dash and the section states the requirement. It is
not left blank, because a blank cell reads as "zero" or "unknown to the tool" rather than "your
server cannot report this".

### 3.3 Scoping: what a single connection can see

`pg_subscription` is a **shared** catalogue, so subscriptions are visible cluster-wide from whichever
database the connection happens to be attached to. `pg_publication` is **not** shared, so only the
connected database's publications are ever visible.

This asymmetry must be visible in the UI. The publications list is labelled with the database it came
from; without that label, a server with publications in three other databases reports "no
publications" and the user believes it.

### 3.4 Privileges

A role without `pg_monitor` sees a reduced picture: PostgreSQL masks query text and other backends'
details in `pg_stat_activity`, and restricts parts of the replication views. Phase 4 already
classifies such a role as limited, and Phase 5 reuses that classification rather than introducing a
second one.

Precisely which columns are masked versus which rows disappear, per view and per version, is settled
by observation in the portability tests (§9.2) rather than asserted here. The pages render what the
probe reports; a masked column is treated as "not permitted", which is a distinct state from
"unsupported" and from "failed".

---

## 4. The Locks page

### 4.1 The blocked-tree query

One query returns a flat row per participating backend: pid, the array from `pg_blocking_pids(pid)`,
how long it has been waiting, the lock's mode and target relation, and the joined activity fields —
user, database, state and query.

Two decisions make it correct rather than merely cheap:

**Filter to backends actually waiting on a lock.** `pg_blocking_pids()` is not free — it inspects lock
manager state per call — so it is evaluated only for backends with `wait_event_type = 'Lock'`. On a
healthy server that is zero rows and the query costs almost nothing.

**Include the blockers, even though they are not waiting.** The root of a blocked chain is very often
`idle in transaction`: a session that acquired a lock, stopped doing anything, and is not itself
waiting for anyone. It therefore does not appear in the waiting set. Without deliberately fetching it,
the tree shows a pid at the top with no user, no database and no query — exactly the row the operator
needs in order to decide whether terminating it is safe. The query unions the waiters with the pids
named in their `pg_blocking_pids()` arrays.

### 4.2 The tree builder

A pure function: flat rows in, a forest of blocked chains out. No database access, no GTK types.

Three cases it must handle, none of which can be produced on demand against a live server, and all of
which are trivial to express as fixtures:

- **A cycle.** PostgreSQL's deadlock detector resolves genuine deadlocks, but a sample can catch a
  cycle mid-flight. The builder marks the cycle and stops; it must not recurse indefinitely.
- **A blocker missing from the sample.** Between the two halves of the query a blocker may have
  exited. The waiter is kept and its parent becomes a stub naming the pid alone, rather than the whole
  subtree being dropped.
- **Several waiters on one blocker.** The common shape, and the one worth rendering well: one root
  with N children, not N separate chains.

### 4.3 The inventory view and visibility gating

The full inventory is every row of `pg_locks` joined to its holder, filterable by lock mode and
relation, bounded by a configurable limit in the manner of `relations_limit`. This adds one GSettings
key, `locks-limit`, defaulting to 500, alongside the existing `statements-limit` and
`relations-limit`. When the limit truncates the result, the page says so — "showing 500 of 4,312" —
because a silently truncated list of locks is worse than no list at all.

It samples **only while its view is selected**. On a busy server this query returns thousands of rows
dominated by uncontended `AccessShareLock`s, and running it every second for a page nobody is looking
at is a cost paid for nothing. This is the first visibility-gated query in the codebase, and §6.2
defines the mechanism.

### 4.4 Selection and actions

Selection uses the Phase 4 by-key mechanism, so a selected row survives the one-second refresh. The
action bar is the existing one: `Action::Cancel` and `Action::Terminate`, gated on
`Capabilities.signal_backend`, with the same confirmation dialog naming the pid, user, database and
query, and the same result toasts — including the "no longer running" outcome for a backend that
exited between selection and confirmation, which contention makes more likely here than anywhere else
in the application.

### 4.5 The empty state

No contention is the normal, healthy condition and must read as such: "No blocked sessions". Not an
error, not an empty table with headers, and not a spinner. A user who opens this page during an
incident and sees "No blocked sessions" has learned something valuable, and the page should make that
legible at a glance.

---

## 5. The Replication page

### 5.1 Role-driven layout

`pg_is_in_recovery()` decides what the page shows. A primary and a standby have almost disjoint
interesting facts, and rendering both sets always would leave half the page permanently empty.

### 5.2 On a primary — connected standbys

One row per standby from `pg_stat_replication`: application name, client address, state, sync state,
the three lag intervals (`write_lag`, `flush_lag`, `replay_lag`), and the byte distance from
`sent_lsn` to `replay_lsn`.

Both units are shown deliberately. Seconds answer "how far behind in time is this replica", which is
what a failover decision needs. Bytes answer "how much WAL must be shipped to catch up", which is what
a capacity decision needs. Neither substitutes for the other.

### 5.3 On a standby — the upstream

From `pg_stat_wal_receiver`: the upstream host, the receiver's status, and received-versus-replayed
LSN. Alongside it, `pg_last_xact_replay_timestamp()` gives the more immediately intuitive figure —
"replaying changes from 4 seconds ago" — which is the number most people actually want.

### 5.4 Slots — always

Slot name, type, plugin, database, active flag, `wal_status`, and `safe_wal_size` where reported. On
16 and later, how long an inactive slot has been inactive.

**Inactive slots sort above active ones.** A slot with no consumer retains WAL indefinitely, and the
resulting disk exhaustion takes the server down. It is the one thing on this page that can cause an
outage by itself, and it should be impossible to miss.

### 5.5 Logical replication

Subscriptions, cluster-wide, with their worker state; on 15 and later, apply and sync error counts
from `pg_stat_subscription_stats`. Publications last, labelled with the connected database (§3.3).

Both are hidden entirely when the server has neither, rather than shown as empty tables. Most servers
do not use logical replication, and the page should not imply they are missing something.

---

## 6. Sampling

### 6.1 Tier assignment

| Data | Tier | Reason |
|---|---|---|
| Blocked tree | fast | contention is transient; a ten-second-old picture is worthless |
| Lock inventory | visibility-gated | expensive, and rarely watched |
| All replication | slow | lag and retention move over minutes |

### 6.2 Visibility gating

The collector currently samples everything in a tier on every tick, regardless of which page is
visible. Phase 5 adds a single flag on the sample configuration, set from the window when the lock
inventory view becomes visible and cleared when it stops being visible. The collector consults it
before running that one query.

Deliberately minimal: it is one flag for one query, not a general subscription mechanism. If a later
phase needs several, the pattern generalises; inventing that generality now would be speculative.

The gate must be **fail-closed**: if the flag is unset for any reason, the query does not run. The
inventory view showing stale data for one refresh after becoming visible is a far better failure than
the expensive query running forever because a flag leaked.

---

## 7. Module layout

| File | Contains |
|---|---|
| `src/collector/locks.rs` | lock SQL, flat row types, the pure tree builder and its tests |
| `src/collector/replication.rs` | replication SQL, row types, version branching |
| `src/pages/locks.rs` | two-view page, selection, action bar wiring |
| `src/pages/replication.rs` | sectioned page, role-driven layout |
| `resources/ui/locks_page.blp` | Locks layout |
| `resources/ui/replication_page.blp` | Replication layout |

`src/collector/snapshot.rs` gains two fields; `src/collector/worker.rs` gains the two sample calls and
the visibility flag; `resources/ui/window.blp` gains two `Adw.ViewStackPage` entries after
`relations`, which is the slot Phase 2 deliberately left free.

If `locks.rs` approaches the ~800-line limit observed elsewhere in the project, the tree builder
splits into its own module. It is the natural seam: pure, self-contained and independently tested.

---

## 8. Error handling

Every section fails independently, using the existing `classify_slow` wrapper so that one failure
never discards a whole tick. A server whose slot query is refused on privileges still shows its
standbys.

Three failure states are distinguished on screen, because each demands a different response:

| State | Message | Example |
|---|---|---|
| Unsupported | names the version required | "Inactive duration requires PostgreSQL 16 or later" |
| Not permitted | names the privilege required | "Requires pg_monitor" |
| Failed | the PostgreSQL message itself | "canceling statement due to lock timeout" |

The third is deliberate. Issue #6 records a case from the Phase 4 verification where a generic failure
message left a real, explicable error — a lock timeout caused by an earlier vacuum still running —
looking arbitrary. Surfacing the server's own message costs nothing and turns a dead end into a
diagnosis.

---

## 9. Testing

### 9.1 Unit tests — no database

The tree builder against fixtures: a single chain; one blocker with several waiters; a cycle; a
blocker absent from the sample; the empty case. Plus the sort rule that puts inactive slots first, and
the version-branch selection for slot and subscription columns.

### 9.2 Portability tests — PostgreSQL 14 and 18

All five queries run against both versions, as a superuser **and** as a role without `pg_monitor`. The
plain-role runs are what settle §3.4 by observation: the test records which columns come back masked
and which rows are absent, so the pages can be built against what the server actually does rather than
against what the documentation implies.

Slot tests create a physical slot with `pg_create_physical_replication_slot()` — no standby required —
which gives an inactive slot to assert the sort rule against.

### 9.3 Live walkthrough

Two situations cannot be faked and must be produced for real:

- **Contention.** Two `psql` sessions, one holding a row lock inside an open transaction, a second
  waiting on it, and a third waiting on the second — a three-deep chain with an `idle in transaction`
  root.
- **A standby.** A second container streaming from the first, so `pg_stat_replication` and
  `pg_stat_wal_receiver` have real content, and lag can be observed by pausing replay.

---

## 10. Success criteria

Each is observable, and each maps to a tick in the implementation plan's final task:

1. A row lock held by one session and wanted by another appears as a two-node chain, naming both
   backends' pid, user, database and query, within one refresh of the block starting.
2. A three-deep chain renders as one tree, not three separate rows, with the `idle in transaction`
   root shown as the root.
3. Terminating the root blocker from the Locks page clears the tree on the next sample.
4. With no contention, the page reads "No blocked sessions".
5. The inventory view reports its truncation explicitly when the limit is reached.
6. The inventory query does not run while its view is not selected, confirmed from
   `pg_stat_activity` on the server rather than from the UI.
7. On a primary with a streaming standby, lag is shown in both seconds and bytes, and both move when
   replay is paused.
8. On a standby, the page shows the upstream and how far behind replay is in seconds.
9. An inactive slot sorts above active slots.
10. On PostgreSQL 14, the version-gated slot and subscription columns state their version requirement
    rather than rendering blank.
11. As a role without `pg_monitor`, every section either shows data or states that the privilege is
    required — no section is silently empty.

---

## 11. Open questions

- **Lock inventory filters.** Filtering by mode and relation is specified; whether the filter is a
  search entry, a dropdown, or both is left to the implementation plan.
- **Standby identification.** `application_name` is the conventional label but is operator-set and may
  be absent or duplicated. Falling back to `client_addr` is the obvious answer; whether to show both
  always is a layout question better settled against a real two-node setup.

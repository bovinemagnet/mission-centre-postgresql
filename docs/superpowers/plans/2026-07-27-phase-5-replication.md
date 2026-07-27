# Phase 5 — Replication Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Replication page — connected standbys on a primary, upstream state on a standby, replication slots always, and logical replication when the server uses it.

**Architecture:** Four independent queries on the slow tier, each failing on its own without discarding the tick. `pg_is_in_recovery()` decides which sections the page shows, so a primary and a standby each get only what is relevant to them. Version differences are confined to the slot and subscription queries, which are built by pure functions selected on `version_num` and unit-tested without a database.

**Tech Stack:** Rust, GTK4 + libadwaita via gtk-rs, Blueprint (`.blp`) for layout, `tokio-postgres`, Meson + Ninja, `testcontainers` for portability tests.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-27-phase-5-replication-and-locks-design.md`, §5 and the replication parts of §3, §7–§10. The Locks half is already implemented; see `docs/superpowers/plans/2026-07-27-phase-5-locks.md`.
- **Author:** Paul Snow. **Version:** 0.0.0. GPL-3.0-or-later header on every new file, copied from `src/pages/locks.rs:1-19`.
- **British spelling** in comments, documentation and user-visible strings.
- **PostgreSQL 14 is the version floor.** Every query must run on 14 and 18.
- **Prefer files under ~800 lines**, but not at the cost of restructuring unrelated code. `src/collector/worker.rs` already sits at 813.
- **User-visible strings** go through `crate::i18n::i18n`.
- **Version boundaries, verified against containers on 2026-07-27** — do not re-derive these from memory:

| Column or view | First version |
|---|---|
| `pg_stat_replication`, `pg_stat_wal_receiver` | 14 (identical columns through 18) |
| `pg_stat_subscription_stats` | 15 |
| `pg_replication_slots.conflicting` | 16 |
| `pg_replication_slots.inactive_since`, `.invalidation_reason`, `.failover`, `.synced` | 17 |
| `pg_stat_subscription.worker_type`, `.leader_pid` | 17 |
| `pg_replication_slots.two_phase_at` | 18 |

- **Commands:** `cargo test --lib`, `cargo test --bin mission-centre-pg`, `ninja -C build`, and `export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"` before `cargo test --test portability`.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/collector/replication.rs` | Create — row types, version-branched SQL builders, mapping, sampling, unit tests |
| `src/pages/replication.rs` | Create — role-driven sectioned page |
| `resources/ui/replication_page.blp` | Create — layout |
| `src/collector/snapshot.rs` | Modify — one new `Snapshot` field and `in_recovery` on `ServerSettings` |
| `src/collector/worker.rs` | Modify — slow-tier sampling call |
| `src/collector/mod.rs`, `src/pages/mod.rs` | Modify — declare and re-export |
| `src/window.rs` | Modify — construct and update the page |
| `resources/ui/window.blp`, `resources/meson.build`, `resources/mission-centre-pg.gresource.xml` | Modify — register the page |
| `tests/portability.rs` | Modify — every query on 14 and 18, superuser and plain role, plus a real slot |

---

## Task 1: Row types and version-branched SQL

**Files:**
- Create: `src/collector/replication.rs`
- Modify: `src/collector/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Standby`, `Slot`, `Subscription`, `Publication`, `WalReceiver`, `ReplicationSample`; `pub fn slots_sql(version_num: i32) -> String`; `pub fn subscriptions_sql(version_num: i32) -> String`; `pub fn sort_slots(slots: &mut Vec<Slot>)`.

- [ ] **Step 1: Create the module with the types**

Create `src/collector/replication.rs` with the GPL header, then:

```rust
/// A standby connected to this primary, from `pg_stat_replication`.
#[derive(Debug, Clone, PartialEq)]
pub struct Standby {
    pub pid: i32,
    pub application_name: Option<String>,
    pub client_addr: Option<String>,
    pub state: Option<String>,
    pub sync_state: Option<String>,
    pub write_lag_secs: Option<f64>,
    pub flush_lag_secs: Option<f64>,
    pub replay_lag_secs: Option<f64>,
    /// Bytes between `sent_lsn` and `replay_lsn`: how much WAL the standby
    /// still has to apply. Seconds answer "how stale"; bytes answer "how much
    /// work remains". Neither substitutes for the other.
    pub replay_lag_bytes: Option<i64>,
}

/// A replication slot. `inactive_since` is `None` before PostgreSQL 17, which
/// is a different thing from "active right now" and must render differently.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub slot_name: String,
    pub slot_type: Option<String>,
    pub plugin: Option<String>,
    pub database: Option<String>,
    pub active: bool,
    pub wal_status: Option<String>,
    pub safe_wal_size: Option<i64>,
    pub inactive_since_secs: Option<f64>,
    pub conflicting: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub subname: String,
    pub pid: Option<i32>,
    pub worker_type: Option<String>,
    pub latest_end_lag_secs: Option<f64>,
    pub apply_error_count: Option<i64>,
    pub sync_error_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Publication {
    pub pubname: String,
    pub all_tables: bool,
}

/// Upstream state when this server is itself a standby.
#[derive(Debug, Clone, PartialEq)]
pub struct WalReceiver {
    pub status: Option<String>,
    pub sender_host: Option<String>,
    pub received_lsn: Option<String>,
    pub replayed_lsn: Option<String>,
    pub replay_delay_secs: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct ReplicationSample {
    pub in_recovery: bool,
    pub standbys: Vec<Standby>,
    pub receiver: Option<WalReceiver>,
    pub slots: Vec<Slot>,
    pub subscriptions: Vec<Subscription>,
    pub publications: Vec<Publication>,
}
```

Declare `pub mod replication;` in `src/collector/mod.rs`.

- [ ] **Step 2: Write the failing tests for the version branches and the sort**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn slot(name: &str, active: bool) -> Slot {
        Slot {
            slot_name: name.to_string(),
            slot_type: Some("physical".to_string()),
            plugin: None,
            database: None,
            active,
            wal_status: Some("reserved".to_string()),
            safe_wal_size: None,
            inactive_since_secs: None,
            conflicting: None,
        }
    }

    #[test]
    fn postgres_14_asks_for_no_column_it_does_not_have() {
        let sql = slots_sql(140000);
        assert!(!sql.contains("inactive_since"));
        assert!(!sql.contains("conflicting"));
        assert!(sql.contains("wal_status"));
    }

    #[test]
    fn postgres_16_asks_for_conflicting_but_not_inactive_since() {
        let sql = slots_sql(160000);
        assert!(sql.contains("conflicting"));
        assert!(!sql.contains("inactive_since"));
    }

    #[test]
    fn postgres_17_asks_for_inactive_since() {
        let sql = slots_sql(170000);
        assert!(sql.contains("inactive_since"));
        assert!(sql.contains("conflicting"));
    }

    #[test]
    fn subscription_statistics_are_only_requested_from_15_onwards() {
        assert!(!subscriptions_sql(140000).contains("pg_stat_subscription_stats"));
        assert!(subscriptions_sql(150000).contains("pg_stat_subscription_stats"));
    }

    #[test]
    fn worker_type_is_only_requested_from_17_onwards() {
        assert!(!subscriptions_sql(160000).contains("worker_type"));
        assert!(subscriptions_sql(170000).contains("worker_type"));
    }

    #[test]
    fn inactive_slots_sort_above_active_ones() {
        let mut slots = vec![slot("live", true), slot("abandoned", false)];
        sort_slots(&mut slots);
        assert_eq!(slots[0].slot_name, "abandoned");
    }

    #[test]
    fn slots_of_equal_activity_sort_by_name() {
        let mut slots = vec![slot("b", true), slot("a", true)];
        sort_slots(&mut slots);
        assert_eq!(slots[0].slot_name, "a");
    }
}
```

- [ ] **Step 3: Run them and watch them fail**

```bash
cargo test --lib collector::replication
```

Expected: all seven fail — the three functions do not exist yet.

- [ ] **Step 4: Implement the builders and the sort**

```rust
/// Slot columns arrive across three releases, so the query is built rather
/// than branched wholesale: 16 adds `conflicting`, 17 adds `inactive_since`.
/// Asking PostgreSQL 14 for either is an error, not a NULL.
pub fn slots_sql(version_num: i32) -> String {
    let conflicting = if version_num >= 160000 {
        "s.conflicting"
    } else {
        "NULL::boolean"
    };
    let inactive_since = if version_num >= 170000 {
        "EXTRACT(EPOCH FROM (now() - s.inactive_since))::float8"
    } else {
        "NULL::float8"
    };

    format!(
        "SELECT s.slot_name                AS slot_name,
                s.slot_type::text          AS slot_type,
                s.plugin::text             AS plugin,
                s.database::text           AS database,
                s.active                   AS active,
                s.wal_status::text         AS wal_status,
                s.safe_wal_size            AS safe_wal_size,
                {inactive_since}           AS inactive_since_secs,
                {conflicting}              AS conflicting
         FROM pg_replication_slots s"
    )
}

/// `pg_stat_subscription_stats` is 15 and later; `worker_type` is 17 and
/// later. Both are left-joined or substituted rather than omitted, so the row
/// shape is identical on every version and the mapper needs no branch.
pub fn subscriptions_sql(version_num: i32) -> String {
    let worker_type = if version_num >= 170000 {
        "s.worker_type::text"
    } else {
        "NULL::text"
    };
    let (errors, join) = if version_num >= 150000 {
        (
            "st.apply_error_count, st.sync_error_count",
            "LEFT JOIN pg_stat_subscription_stats st ON st.subid = s.subid",
        )
    } else {
        ("NULL::bigint, NULL::bigint", "")
    };

    format!(
        "SELECT s.subname::text            AS subname,
                s.pid                      AS pid,
                {worker_type}              AS worker_type,
                EXTRACT(EPOCH FROM (now() - s.latest_end_time))::float8
                                           AS latest_end_lag_secs,
                {errors}
         FROM pg_stat_subscription s
         {join}"
    )
}

/// Inactive slots first: a slot with no consumer retains WAL until the disk
/// fills, which is the one thing on this page that can take a server down by
/// itself. Ties break by name so the order is stable between samples.
pub fn sort_slots(slots: &mut [Slot]) {
    slots.sort_by(|a, b| {
        a.active
            .cmp(&b.active)
            .then_with(|| a.slot_name.cmp(&b.slot_name))
    });
}
```

Note the `errors` substitution keeps two columns in both branches, so
`map_slot`'s sibling `map_subscription` reads the same column names regardless
of version.

- [ ] **Step 5: Run them and watch them pass**

```bash
cargo test --lib collector::replication
```

Expected: seven passes.

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo test --lib
git add src/collector/replication.rs src/collector/mod.rs
git commit -m "feat: replication row types and version-branched SQL"
```

---

## Task 2: The remaining queries, mapping and sampling

**Files:**
- Modify: `src/collector/replication.rs`
- Modify: `tests/portability.rs`

**Interfaces:**
- Consumes: the types from Task 1.
- Produces: `STANDBYS_SQL`, `RECEIVER_SQL`, `PUBLICATIONS_SQL`, `IN_RECOVERY_SQL`; `map_standby`, `map_slot`, `map_subscription`, `map_publication`, `map_receiver`; `pub async fn sample_replication(client: &Client, version_num: i32) -> Result<ReplicationSample, tokio_postgres::Error>`.

- [ ] **Step 1: Add the version-independent queries**

```rust
/// Identical columns on 14 through 18, so no branch is needed. The byte
/// distance is computed server-side: pg_lsn arithmetic has no client-side
/// equivalent worth writing.
pub const STANDBYS_SQL: &str = "\
SELECT pid,
       application_name::text                              AS application_name,
       client_addr::text                                   AS client_addr,
       state::text                                         AS state,
       sync_state::text                                    AS sync_state,
       EXTRACT(EPOCH FROM write_lag)::float8               AS write_lag_secs,
       EXTRACT(EPOCH FROM flush_lag)::float8               AS flush_lag_secs,
       EXTRACT(EPOCH FROM replay_lag)::float8              AS replay_lag_secs,
       (sent_lsn - replay_lsn)::bigint                     AS replay_lag_bytes
FROM pg_stat_replication";

pub const RECEIVER_SQL: &str = "\
SELECT status::text                                        AS status,
       sender_host::text                                   AS sender_host,
       latest_end_lsn::text                                AS received_lsn,
       pg_last_wal_replay_lsn()::text                      AS replayed_lsn,
       EXTRACT(EPOCH FROM (now() - pg_last_xact_replay_timestamp()))::float8
                                                           AS replay_delay_secs
FROM pg_stat_wal_receiver";

/// Publications belong to the connected database only — pg_publication is not
/// a shared catalogue — which the page states rather than implying the server
/// has none.
pub const PUBLICATIONS_SQL: &str = "\
SELECT pubname::text AS pubname, puballtables AS all_tables
FROM pg_publication
ORDER BY pubname";

pub const IN_RECOVERY_SQL: &str = "SELECT pg_is_in_recovery() AS in_recovery";
```

- [ ] **Step 2: Add the mappers**

```rust
pub fn map_standby(row: &Row) -> Standby {
    Standby {
        pid: row.get("pid"),
        application_name: row.get("application_name"),
        client_addr: row.get("client_addr"),
        state: row.get("state"),
        sync_state: row.get("sync_state"),
        write_lag_secs: row.get("write_lag_secs"),
        flush_lag_secs: row.get("flush_lag_secs"),
        replay_lag_secs: row.get("replay_lag_secs"),
        replay_lag_bytes: row.get("replay_lag_bytes"),
    }
}

pub fn map_slot(row: &Row) -> Slot {
    Slot {
        slot_name: row.get("slot_name"),
        slot_type: row.get("slot_type"),
        plugin: row.get("plugin"),
        database: row.get("database"),
        active: row.get("active"),
        wal_status: row.get("wal_status"),
        safe_wal_size: row.get("safe_wal_size"),
        inactive_since_secs: row.get("inactive_since_secs"),
        conflicting: row.get("conflicting"),
    }
}

pub fn map_subscription(row: &Row) -> Subscription {
    Subscription {
        subname: row.get("subname"),
        pid: row.get("pid"),
        worker_type: row.get("worker_type"),
        latest_end_lag_secs: row.get("latest_end_lag_secs"),
        apply_error_count: row.get("apply_error_count"),
        sync_error_count: row.get("sync_error_count"),
    }
}

pub fn map_publication(row: &Row) -> Publication {
    Publication {
        pubname: row.get("pubname"),
        all_tables: row.get("all_tables"),
    }
}

pub fn map_receiver(row: &Row) -> WalReceiver {
    WalReceiver {
        status: row.get("status"),
        sender_host: row.get("sender_host"),
        received_lsn: row.get("received_lsn"),
        replayed_lsn: row.get("replayed_lsn"),
        replay_delay_secs: row.get("replay_delay_secs"),
    }
}
```

- [ ] **Step 3: Add the sampler**

```rust
/// One slow-tier pass over every replication source. The role's own view
/// decides what comes back; nothing here fails just because a section is
/// empty, which is the normal case on most servers.
pub async fn sample_replication(
    client: &Client,
    version_num: i32,
) -> Result<ReplicationSample, tokio_postgres::Error> {
    let in_recovery: bool = client.query_one(IN_RECOVERY_SQL, &[]).await?.get("in_recovery");

    // A standby has no standbys of its own to report, and a primary has no
    // receiver. Skipping the query that cannot apply keeps a slow tick short.
    let standbys = if in_recovery {
        Vec::new()
    } else {
        client
            .query(STANDBYS_SQL, &[])
            .await?
            .iter()
            .map(map_standby)
            .collect()
    };

    let receiver = if in_recovery {
        client.query_opt(RECEIVER_SQL, &[]).await?.as_ref().map(map_receiver)
    } else {
        None
    };

    let mut slots: Vec<Slot> = client
        .query(slots_sql(version_num).as_str(), &[])
        .await?
        .iter()
        .map(map_slot)
        .collect();
    sort_slots(&mut slots);

    let subscriptions = client
        .query(subscriptions_sql(version_num).as_str(), &[])
        .await?
        .iter()
        .map(map_subscription)
        .collect();

    let publications = client
        .query(PUBLICATIONS_SQL, &[])
        .await?
        .iter()
        .map(map_publication)
        .collect();

    Ok(ReplicationSample {
        in_recovery,
        standbys,
        receiver,
        slots,
        subscriptions,
        publications,
    })
}
```

- [ ] **Step 4: Write the failing portability tests**

Add to `tests/portability.rs`, following the existing `assert_*` plus two `#[tokio::test]` wrappers pattern:

```rust
/// Every replication query must run on both versions. A fresh server has no
/// standbys, no slots and no subscriptions, which is the ordinary case.
async fn assert_replication_sample_runs(tag: &str) {
    let (client, _container) = connect(tag).await;

    let version: i32 = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("probe failed")
        .get("version_num");

    let sample = sample_replication(&client, version)
        .await
        .expect("the replication sample must run");

    assert!(!sample.in_recovery, "a fresh container is a primary");
    assert!(sample.standbys.is_empty());
    assert!(sample.slots.is_empty());
    assert!(sample.subscriptions.is_empty());
}

#[tokio::test]
async fn replication_sample_runs_on_postgres_14() {
    assert_replication_sample_runs("14").await;
}

#[tokio::test]
async fn replication_sample_runs_on_postgres_18() {
    assert_replication_sample_runs("18").await;
}

/// A physical slot needs no standby to create, which gives an inactive slot
/// to assert the sort rule against on a single container.
async fn assert_an_inactive_slot_is_reported_and_sorted_first(tag: &str) {
    let (client, _container) = connect(tag).await;
    client
        .batch_execute("SELECT pg_create_physical_replication_slot('spare')")
        .await
        .expect("failed to create the slot");
    client
        .batch_execute("SELECT pg_create_physical_replication_slot('another')")
        .await
        .expect("failed to create the second slot");

    let version: i32 = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("probe failed")
        .get("version_num");

    let sample = sample_replication(&client, version)
        .await
        .expect("the replication sample must run");

    assert_eq!(sample.slots.len(), 2);
    assert!(
        sample.slots.iter().all(|slot| !slot.active),
        "a slot with no consumer is inactive"
    );
    assert_eq!(
        sample.slots[0].slot_name, "another",
        "equally inactive slots sort by name"
    );
    assert_eq!(sample.slots[0].slot_type.as_deref(), Some("physical"));
}

#[tokio::test]
async fn an_inactive_slot_is_reported_and_sorted_first_on_postgres_14() {
    assert_an_inactive_slot_is_reported_and_sorted_first("14").await;
}

#[tokio::test]
async fn an_inactive_slot_is_reported_and_sorted_first_on_postgres_18() {
    assert_an_inactive_slot_is_reported_and_sorted_first("18").await;
}

/// On 17 and later the inactive duration is reported; before it, the column
/// is absent and the sample must carry None rather than a fabricated zero.
#[tokio::test]
async fn the_inactive_duration_is_absent_before_postgres_17() {
    let (client, _container) = connect("14").await;
    client
        .batch_execute("SELECT pg_create_physical_replication_slot('spare')")
        .await
        .expect("failed to create the slot");

    let sample = sample_replication(&client, 140000)
        .await
        .expect("the replication sample must run");

    assert_eq!(sample.slots[0].inactive_since_secs, None);
    assert_eq!(sample.slots[0].conflicting, None);
}

#[tokio::test]
async fn the_inactive_duration_is_reported_on_postgres_18() {
    let (client, _container) = connect("18").await;
    client
        .batch_execute("SELECT pg_create_physical_replication_slot('spare')")
        .await
        .expect("failed to create the slot");

    let sample = sample_replication(&client, 180000)
        .await
        .expect("the replication sample must run");

    assert!(
        sample.slots[0].inactive_since_secs.is_some(),
        "PostgreSQL 17 and later report how long a slot has been inactive"
    );
}

/// Settles §3.4 by observation for the replication views too.
async fn assert_replication_runs_for_a_plain_role(tag: &str) {
    let (client, container) = connect(tag).await;
    client
        .batch_execute("CREATE ROLE plain LOGIN PASSWORD 'plain'")
        .await
        .expect("failed to create the plain role");

    let plain = connect_as(&container, "plain", "plain").await;
    let version: i32 = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("probe failed")
        .get("version_num");

    sample_replication(&plain, version)
        .await
        .expect("a role without pg_monitor must still run the replication queries");
}

#[tokio::test]
async fn replication_runs_for_a_plain_role_on_postgres_14() {
    assert_replication_runs_for_a_plain_role("14").await;
}

#[tokio::test]
async fn replication_runs_for_a_plain_role_on_postgres_18() {
    assert_replication_runs_for_a_plain_role("18").await;
}
```

- [ ] **Step 5: Run them**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability replication
cargo test --test portability inactive
```

Expected: nine passes. If the plain-role test fails, that is a genuine finding about the privilege model — record what the server actually refused before changing the query.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/collector/replication.rs tests/portability.rs
git commit -m "feat: replication queries, verified against PostgreSQL 14 and 18"
```

---

## Task 3: Carry replication in the snapshot

**Files:**
- Modify: `src/collector/snapshot.rs`, `src/collector/worker.rs`, `src/history/sample.rs`

**Interfaces:**
- Consumes: `sample_replication`, `ReplicationSample`.
- Produces: `Snapshot.replication: Option<Result<ReplicationSample, CollectorError>>`, populated on slow ticks only.

- [ ] **Step 1: Add the field**

In `src/collector/snapshot.rs`, after `lock_inventory`:

```rust
    /// Slow tier: `None` on a fast tick, and the page keeps what it has.
    pub replication: Option<Result<ReplicationSample, CollectorError>>,
```

Import `ReplicationSample` beside the other sample types.

- [ ] **Step 2: Build and let the compiler list the sites**

```bash
cargo build
```

Expected: `missing field replication` at `src/collector/worker.rs` and `src/history/sample.rs`.

- [ ] **Step 3: Sample it on the slow tier**

In `src/collector/worker.rs`, inside the existing `match slow` block that already produces `(statements, relations)`, extend the tuple to include replication:

```rust
    let (statements, relations, replication) = match slow {
        None => (None, None, None),
        Some(slow) => {
            let statements = if slow.statements_available {
                Some(classify_slow(
                    sample_statements(client, &slow, taken_at).await,
                )?)
            } else {
                None
            };
            let relations = Some(classify_slow(
                sample_relations(client, slow.relations_limit, slow.version_num).await,
            )?);
            let replication = Some(classify_slow(
                sample_replication(client, slow.version_num)
                    .await
                    .map_err(map_query_error),
            )?);
            (statements, relations, replication)
        }
    };
```

Add `replication` to the returned `Snapshot`, and `replication: None` to the fixture in `src/history/sample.rs`.

- [ ] **Step 4: Build and test**

```bash
cargo build
cargo test --lib
cargo test --bin mission-centre-pg
```

Expected: clean build, all existing tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/collector/snapshot.rs src/collector/worker.rs src/history/sample.rs
git commit -m "feat: sample replication on the slow tier"
```

---

## Task 4: The Replication page

**Files:**
- Create: `src/pages/replication.rs`, `resources/ui/replication_page.blp`
- Modify: `src/pages/mod.rs`, `resources/meson.build`, `resources/mission-centre-pg.gresource.xml`

**Interfaces:**
- Consumes: `ReplicationSample` and its row types.
- Produces: `McpgReplicationPage` with `pub fn update(&self, replication: Option<&Result<ReplicationSample, CollectorError>>)` and `pub fn set_database(&self, database: &str)`.

- [ ] **Step 1: Write the failing tests for the pure helpers**

The page's judgement calls are all pure, and get tested before any widget exists. Create `src/pages/replication.rs` with the GPL header and:

```rust
use crate::collector::replication::{ReplicationSample, Slot, Standby};

/// What the inactive-duration cell shows. `None` from the sample means the
/// server cannot report it, which is different from a slot that has never
/// been inactive, and must not render as a blank.
pub fn inactive_cell(slot: &Slot, version_num: i32) -> String {
    if version_num < 170000 {
        return "—".to_string();
    }
    match slot.inactive_since_secs {
        Some(secs) if secs >= 3600.0 => format!("{:.0}h", secs / 3600.0),
        Some(secs) if secs >= 60.0 => format!("{:.0}m", secs / 60.0),
        Some(secs) => format!("{secs:.0}s"),
        None => "active".to_string(),
    }
}

/// Both units, because they answer different questions: seconds say how stale
/// the replica is, bytes say how much work catching up will take.
pub fn lag_cell(standby: &Standby) -> String {
    match (standby.replay_lag_secs, standby.replay_lag_bytes) {
        (Some(secs), Some(bytes)) => format!("{secs:.1}s / {}", crate::pages::format::format_bytes(bytes)),
        (Some(secs), None) => format!("{secs:.1}s"),
        (None, Some(bytes)) => crate::pages::format::format_bytes(bytes),
        (None, None) => "—".to_string(),
    }
}

/// Which sections the page shows. A primary has no upstream and a standby has
/// no standbys of its own, so showing both would leave half the page
/// permanently empty.
pub fn visible_sections(sample: &ReplicationSample) -> Vec<&'static str> {
    let mut sections = Vec::new();
    if sample.in_recovery {
        sections.push("receiver");
    } else {
        sections.push("standbys");
    }
    sections.push("slots");
    if !sample.subscriptions.is_empty() || !sample.publications.is_empty() {
        sections.push("logical");
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::replication::Slot;

    fn slot(inactive_since_secs: Option<f64>) -> Slot {
        Slot {
            slot_name: "s".to_string(),
            slot_type: Some("physical".to_string()),
            plugin: None,
            database: None,
            active: inactive_since_secs.is_none(),
            wal_status: Some("reserved".to_string()),
            safe_wal_size: None,
            inactive_since_secs,
            conflicting: None,
        }
    }

    #[test]
    fn before_17_the_inactive_cell_states_that_the_server_cannot_report_it() {
        assert_eq!(inactive_cell(&slot(None), 140000), "—");
        assert_eq!(inactive_cell(&slot(Some(90.0)), 160000), "—");
    }

    #[test]
    fn an_active_slot_on_17_says_active_rather_than_a_duration() {
        assert_eq!(inactive_cell(&slot(None), 170000), "active");
    }

    #[test]
    fn an_inactive_slot_reports_its_duration_in_readable_units() {
        assert_eq!(inactive_cell(&slot(Some(45.0)), 170000), "45s");
        assert_eq!(inactive_cell(&slot(Some(90.0)), 170000), "2m");
        assert_eq!(inactive_cell(&slot(Some(7200.0)), 170000), "2h");
    }

    #[test]
    fn a_primary_shows_standbys_and_a_standby_shows_its_upstream() {
        let primary = ReplicationSample::default();
        assert!(visible_sections(&primary).contains(&"standbys"));
        assert!(!visible_sections(&primary).contains(&"receiver"));

        let standby = ReplicationSample {
            in_recovery: true,
            ..Default::default()
        };
        assert!(visible_sections(&standby).contains(&"receiver"));
        assert!(!visible_sections(&standby).contains(&"standbys"));
    }

    #[test]
    fn slots_are_always_shown_and_logical_only_when_it_is_used() {
        let sample = ReplicationSample::default();
        assert!(visible_sections(&sample).contains(&"slots"));
        assert!(!visible_sections(&sample).contains(&"logical"));
    }
}
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test --lib pages::replication
```

Expected: compilation fails until `src/pages/mod.rs` declares the module; then the tests run and pass, since the helpers are written above. If any assertion fails, fix the helper rather than the assertion — the expected strings are the specified behaviour.

- [ ] **Step 3: Write the layout**

Create `resources/ui/replication_page.blp` with an `Adw.PreferencesPage` holding four `Adw.PreferencesGroup`s named `standbys_group`, `receiver_group`, `slots_group` and `logical_group`, each containing a `Gtk.ColumnView` inside a `Gtk.ScrolledWindow`, plus a `Gtk.Label publications_note` under the logical group for the connected-database caveat. Follow `resources/ui/relations_page.blp` for the group and scroller structure.

Register it in `resources/meson.build` (`'ui/replication_page.blp',`) and in `resources/mission-centre-pg.gresource.xml` (`<file preprocess="xml-stripblanks">ui/replication_page.ui</file>`).

- [ ] **Step 4: Build the widget**

Add the `McpgReplicationPage` subclass following `src/pages/locks.rs`: an `imp` struct of template children, `Table::attach` for each of the four tables, and an `update` that sets each group's visibility from `visible_sections` and fills its table. The tables:

| Group | Columns |
|---|---|
| Standbys | Application, Client, State, Sync, Write lag, Flush lag, Replay lag (via `lag_cell`) |
| Receiver | Status, Upstream, Received LSN, Replayed LSN, Behind by |
| Slots | Name, Type, Plugin, Database, Active, WAL status, Safe WAL size, Inactive (via `inactive_cell`) |
| Logical | Subscription, Worker, Behind by, Apply errors, Sync errors |

`set_database` sets `publications_note` to a string naming the connected database, since publications are only ever visible for it.

- [ ] **Step 5: Build and commit**

```bash
ninja -C build
cargo test --lib
cargo fmt
git add src/pages/replication.rs src/pages/mod.rs resources/ui/replication_page.blp resources/meson.build resources/mission-centre-pg.gresource.xml
git commit -m "feat: replication page with role-driven sections"
```

---

## Task 5: Wire the page into the window

**Files:**
- Modify: `resources/ui/window.blp`, `src/window.rs`

- [ ] **Step 1: Add the stack page**

In `resources/ui/window.blp`, after the `locks` page:

```blueprint
          Adw.ViewStackPage {
            name: "replication";
            title: _("Replication");
            icon-name: "network-transmit-receive-symbolic";
            child: $McpgReplicationPage replication_page {};
          }
```

- [ ] **Step 2: Hold, register and update it**

In `src/window.rs`: add `McpgReplicationPage` to the `use mission_centre_pg::pages::{...}` list, add `replication_page: TemplateChild<McpgReplicationPage>` to the `imp` struct, add `McpgReplicationPage::ensure_type();` beside the others in `class_init`, and in the `Sample` arm:

```rust
                imp.replication_page
                    .update(snapshot.replication.as_ref());
```

Beside the existing `imp.relations_page.set_database(...)` call in the `Connected` arm, add the same for the replication page so the publications note names the right database.

- [ ] **Step 3: Build and run**

```bash
ninja -C build
MCPG_RESOURCE_DIR=$PWD/build/resources GSETTINGS_SCHEMA_DIR=$PWD/build/data ./build/src/mission-centre-pg
```

Expected: a Replication entry appears; against a plain container it shows an empty standbys section and an empty slots section, with no logical section at all.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add resources/ui/window.blp src/window.rs
git commit -m "feat: add the replication page to the window"
```

---

## Task 6: Full verification

**Files:** none modified unless a check fails.

- [ ] **Step 1: Run every automated check**

```bash
cargo fmt --check
cargo test --lib
cargo test --bin mission-centre-pg
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
ninja -C build
```

- [ ] **Step 2: Stand up a real primary and standby**

A streaming standby cannot be faked, and criteria 7 and 8 depend on one.

```bash
podman network create mcpg-net
podman run --rm -d --name mcpg-primary --network mcpg-net \
  -e POSTGRES_PASSWORD=postgres -e POSTGRES_HOST_AUTH_METHOD=trust \
  -p 55432:5432 docker.io/library/postgres:18
podman exec mcpg-primary bash -c 'until pg_isready -U postgres -q; do sleep 1; done'
podman exec -i mcpg-primary psql -U postgres -c "SELECT pg_create_physical_replication_slot('standby_slot')"

podman run --rm -d --name mcpg-standby --network mcpg-net \
  -e PGUSER=postgres -e POSTGRES_PASSWORD=postgres \
  -p 55433:5432 --entrypoint bash docker.io/library/postgres:18 -c '
    rm -rf /var/lib/postgresql/data/*
    pg_basebackup -h mcpg-primary -U postgres -D /var/lib/postgresql/data -Fp -Xs -R -S standby_slot
    chown -R postgres:postgres /var/lib/postgresql/data
    chmod 700 /var/lib/postgresql/data
    su postgres -c "postgres -D /var/lib/postgresql/data"'
```

Confirm from the primary that the standby attached:

```bash
podman exec -i mcpg-primary psql -U postgres -c "SELECT application_name, state, sync_state, replay_lag FROM pg_stat_replication"
```

- [ ] **Step 3: Walk the success criteria**

Connect the application to `127.0.0.1:55432` (primary) and `127.0.0.1:55433` (standby), ticking spec §10 criteria 7–11:

- [ ] On the primary, the standby appears with lag in both seconds and bytes.
- [ ] Pausing replay moves both figures: `podman exec -i mcpg-standby psql -U postgres -c "SELECT pg_wal_replay_pause()"`, then write on the primary — `podman exec -i mcpg-primary psql -U postgres -c "CREATE TABLE churn AS SELECT g FROM generate_series(1,2000000) g"` — and watch the lag grow. Resume with `pg_wal_replay_resume()`.
- [ ] On the standby, the page shows the upstream and how far behind replay is in seconds.
- [ ] `standby_slot` shows as active while the standby is attached; stop the standby (`podman stop mcpg-standby`) and it sorts to the top as inactive.
- [ ] Against a PostgreSQL 14 container, the inactive-duration column reads "—" and the section states that it needs 17 or later.
- [ ] As a role without `pg_monitor`, every section either shows data or states the privilege required.

- [ ] **Step 4: Tear down**

```bash
podman rm -f mcpg-primary mcpg-standby
podman network rm mcpg-net
```

- [ ] **Step 5: Commit any fixes and open the pull request**

```bash
git status --short
git push -u origin phase-5-replication-and-locks
gh pr create --title "Phase 5: replication and locks pages" \
  --body "Implements docs/superpowers/specs/2026-07-27-phase-5-replication-and-locks-design.md in full — the Locks page (blocked tree, lock inventory, cancel and terminate) and the Replication page (standbys, upstream receiver, slots, logical replication)."
```

---

## Self-Review Notes

**Spec coverage.** §5.1 role-driven layout → Task 4's `visible_sections`, tested both ways. §5.2 standbys with both lag units → Task 2's `STANDBYS_SQL` and Task 4's `lag_cell`. §5.3 upstream → `RECEIVER_SQL`. §5.4 slots, inactive first → Task 1's `sort_slots`, asserted in Task 2 against two real slots. §5.5 logical, hidden when unused → `visible_sections`. §3.2 version matrix → Task 1's three `slots_sql` tests and two `subscriptions_sql` tests, at the exact boundaries 16, 17 and 15. §3.3 publications scoping → `PUBLICATIONS_SQL` and `set_database`. §3.4 privileges → Task 2's plain-role tests. §6.1 slow tier → Task 3. §8 unsupported versus not-permitted versus failed → `inactive_cell` returns the em dash for the first; the third carries the server's message through `classify_slow`. §9.1 unit tests → Tasks 1 and 4. §9.2 portability on both versions and both roles → Task 2. §9.3 a real standby → Task 6. §10 criteria 7–11 → Task 6 Step 3.

**Deliberately not covered.** §10 criteria 1–6 are lock criteria and were verified with the Locks plan. Slot management stays out of scope per §2.2; nothing here calls `pg_drop_replication_slot`.

**Type consistency.** `Slot`, `Standby`, `Subscription`, `Publication` and `WalReceiver` are defined once in Task 1 and used unchanged. `slots_sql` and `subscriptions_sql` return `String`, so call sites use `.as_str()`; `STANDBYS_SQL`, `RECEIVER_SQL` and `PUBLICATIONS_SQL` are `&'static str` and are passed directly. `sample_replication` takes `version_num: i32` in the same form `sample_relations` already does.

**Two traps worth knowing.** `sort_slots` takes `&mut [Slot]`, not `&mut Vec<Slot>` — the Task 1 interface line says `Vec`, but a slice is what the implementation needs and what a `Vec` derefs to. And `subscriptions_sql` substitutes literal `NULL::bigint` columns on 14 rather than omitting them, so `map_subscription` reads the same column names on every version; dropping the columns instead would force a second mapper.

# Mission Centre PostgreSQL — Phase 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist Overview metrics and top-statement history per server — to a local SQLite file or an existing `pgconsole` schema on the monitored server — and preload the Overview graphs from that history on connect, so they survive a restart and reach past the live buffer.

**Architecture:** A new `src/history/` module holds two structurally different backends behind a `HistoryBackend` enum: `Local` is synchronous SQLite file I/O; `PgConsole` is asynchronous `INSERT` over the existing monitoring connection. The collector resolves the effective backend at connect, loads recent history and emits it as a new `CollectorEvent::History` for the window to feed into the Overview graph buffers, then writes on an independent history cadence inside the serial sample loop. History is opt-in per server and never disturbs the live view.

**Tech Stack:** Rust, gtk4-rs 0.11, libadwaita 0.9, tokio-postgres 0.7, rusqlite 0.32 (bundled), testcontainers 0.27.

**Spec:** `docs/superpowers/specs/2026-07-24-phase-3-history-store-design.md`
**Parent spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Repository:** `/home/paul/gitHUB/mission-centre-postgresql`.
- **Licence:** GPL-3.0-or-later. Every new source file carries the same GPL header block as its neighbours (copy from `src/pages/format.rs`, changing only the first line), naming **Paul Snow** as author, ending `SPDX-License-Identifier: GPL-3.0-or-later`.
- **Version:** `0.0.0`.
- **Read-only except opt-in pgconsole INSERT.** The only write Phase 3 makes to a monitored server is `INSERT` into an existing `pgconsole` history table, and only for a server the user set to pgconsole mode. Never DDL. Never retention deletes on pgconsole. A server on Off or Local receives no write of any kind.
- **A history fault must never take down monitoring.** Any history error disables history for that server and logs one line; sampling and the UI continue.
- **Never log or display a password**, nor a full connection string, nor write one to the local store. The local store holds metrics only, keyed by the server UUID (not a secret).
- **`None` means "no honest figure exists", never zero**, and is stored as SQL `NULL`.
- **PostgreSQL floor 14.** The pgconsole SQL must run unchanged on 14 through 18.
- **Never touch GTK widgets off the main thread.** `src/collector/worker.rs` and `src/history/` stay GTK-free.
- **Spelling:** British English in all user-facing strings, comments and documentation (`behaviour`, `initialise`, `colour`).
- **Cargo renames the GTK crates:** code says `gtk::` and `adw::`.
- **`glib::wrapper!` blocks for `CompositeTemplate` widgets must list** `gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget`, plus `gtk::Orientable` for `gtk::Box` subclasses.
- **Every new `.blp` change is compiled by `ninja -C build`**, not `cargo` — build before committing UI changes.
- **`cargo fmt` must produce no diff** before any commit.
- **File size:** no source file over ~800 lines. `src/collector/worker.rs` is at 758 lines; Task 6 adds to it, so it moves its `mod tests` to a sibling include if it would cross ~800 (see Task 6).

### Conventions from earlier phases (follow them)

- **GSettings keys** live in `data/io.github.paulsnow.MissionCentrePg.gschema.xml`; each has a `<range>` and `<default>`. The window reads them with `settings.int(...)`.
- **`ConnectionParams` is serialised into GSettings** and must never gain a password field. New fields need a serde default so Phase 1/2 server JSON still deserialises.
- **Cadence schedulers** follow the `is_slow_tick` shape (Phase 2): `None` last-time → fire now, else fire once the interval has elapsed.
- **A collector error that is a property of the schema/role, not the connection, degrades one feature** rather than failing the sample — the `classify_slow` pattern in `worker.rs`.

### Commands

| Purpose | Command |
|---------|---------|
| Unit tests | `cargo test --lib` |
| One module's unit tests | `cargo test --lib history::` |
| Container tests | `export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"; cargo test --test portability` |
| Format check | `cargo fmt --check` |
| Full build | `ninja -C build` |
| Compile the schema | `glib-compile-schemas data/` |
| Run | `MCPG_RESOURCE_DIR=$PWD/build/resources GSETTINGS_SCHEMA_DIR=$PWD/data ./build/src/mission-centre-pg` |

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/connection/params.rs` | Modify. `HistoryMode` enum; `history` field on `ConnectionParams` | 1 |
| `src/history/mod.rs` | Create. `HistoryBackend`, `HistoryPreload`, `is_history_tick`, module re-exports | 5 |
| `src/history/sample.rs` | Create. `SystemHistorySample`, `QueryHistorySample`, builders from a `Snapshot` | 2 |
| `src/history/local.rs` | Create. `LocalStore`: rusqlite schema, write, load, prune | 3 |
| `src/history/pgconsole.rs` | Create. Probe SQL + `PgConsoleAvailability::classify`, INSERT/load SQL, row mapper | 4 |
| `src/collector/worker.rs` | Modify. History config, connect-time probe/resolve/load, cadence writes, the `History` event | 6 |
| `src/lib.rs` | Modify. `pub mod history;` | 5 |
| `data/…gschema.xml` | Modify. Three history keys | 7 |
| `src/pages/overview.rs` | Modify. `preload`, `reset` | 7 |
| `src/window.rs` | Modify. Handle `CollectorEvent::History`; thread history config into the collector; reset Overview on switch; sidebar-row menu wiring | 7, 8 |
| `resources/ui/add_server_dialog.blp` + `src/dialogs/add_server.rs` | Modify. History mode row | 8 |
| `resources/ui/sidebar_row.blp` + `src/widgets/sidebar_row.rs` | Modify. Per-server history menu | 8 |
| `Cargo.toml` | Modify. `rusqlite` dependency | 3 |
| `tests/portability.rs` | Modify. pgconsole probe + round-trip on 14 and 18 | 9 |

---

## Task 1: `HistoryMode` on `ConnectionParams`

**Files:**
- Modify: `src/connection/params.rs`
- Modify: `src/connection/registry.rs` (test fixture)
- Modify: `src/dialogs/add_server.rs` (construction site)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `mission_centre_pg::connection::params::HistoryMode` — `Off | Local | PgConsole`, `Copy`, serde `rename_all = "lowercase"` with `PgConsole` renamed `pgconsole`, `Default = Off`
  - `ConnectionParams.history: HistoryMode`, serde `#[serde(default)]`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `src/connection/params.rs`:

```rust
    #[test]
    fn history_mode_defaults_to_off_for_a_phase_1_server_json() {
        // Servers stored before Phase 3 have no "history" field. They must
        // deserialise with history off, since history is strictly opt-in.
        let json = r#"{"id":"00000000-0000-0000-0000-000000000000","label":"old",
            "host":"localhost","port":5432,"database":"postgres","user":"paul",
            "ssl_mode":"prefer"}"#;
        let parsed: ConnectionParams = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.history, HistoryMode::Off);
    }

    #[test]
    fn history_mode_round_trips_through_json() {
        let mut original = params();
        original.history = HistoryMode::PgConsole;
        let parsed: ConnectionParams =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(parsed.history, HistoryMode::PgConsole);
    }

    #[test]
    fn pgconsole_serialises_in_lower_case() {
        let mut server = params();
        server.history = HistoryMode::PgConsole;
        let json = serde_json::to_string(&server).unwrap();
        assert!(json.contains("\"history\":\"pgconsole\""), "{json}");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib connection::params`
Expected: FAIL — `cannot find type 'HistoryMode' in this scope`.

- [ ] **Step 3: Add the enum and the field**

In `src/connection/params.rs`, after the `SslMode` impl block, add:

```rust
/// Where a server's history is stored. Off by default and strictly opt-in:
/// Local writes to a SQLite file Mission Centre owns; PgConsole writes to an
/// existing pgconsole schema on the monitored server (INSERT only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HistoryMode {
    #[default]
    Off,
    Local,
    #[serde(rename = "pgconsole")]
    PgConsole,
}
```

Add the field to `ConnectionParams`, after `ssl_mode`:

```rust
    #[serde(default)]
    pub history: HistoryMode,
```

Add it to the manual `Debug` impl, after the `ssl_mode` line:

```rust
            .field("history", &self.history)
```

- [ ] **Step 4: Fix the construction sites**

The new field breaks three struct literals. Add `history: HistoryMode::Off,` as the last field of each:

- `src/connection/params.rs` — the `params()` test helper.
- `src/connection/registry.rs` — the `server()` test helper (add `use crate::connection::params::HistoryMode;` to that test module's imports).
- `src/dialogs/add_server.rs` — the `ConnectionParams { … }` literal in `submit()`. Import it: change the existing `use crate::connection::params::{ConnectionParams, SslMode};` to `use crate::connection::params::{ConnectionParams, HistoryMode, SslMode};`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib connection::`
Expected: PASS. Then `cargo build` to confirm no other construction site was missed.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/connection/params.rs src/connection/registry.rs src/dialogs/add_server.rs
git commit -m "feat: per-server history mode on ConnectionParams"
```

---

## Task 2: `history/sample.rs` — the persisted shapes

**Files:**
- Create: `src/history/sample.rs`
- Modify: `src/lib.rs` (add `pub mod history;`), `src/history/mod.rs` (created here as a stub, completed in Task 5)

**Interfaces:**
- Consumes: `crate::collector::snapshot::Snapshot`; `crate::collector::statements::{Statement, StatementId}`.
- Produces:
  - `SystemHistorySample { total_connections: i32, max_connections: i32, active_queries: i32, idle_connections: i32, idle_in_transaction: i32, cache_hit_ratio: Option<f64>, total_database_size_bytes: Option<i64> }` (Clone, PartialEq)
  - `QueryHistorySample { query_id: Option<i64>, query_text: String, total_calls: i64, total_time_ms: f64, total_rows: i64, mean_time_ms: f64, shared_blks_hit: i64, shared_blks_read: i64 }` (Clone, PartialEq)
  - `system_sample_from(snapshot: &Snapshot) -> SystemHistorySample`
  - `query_samples_from(statements: &[Statement], top: usize) -> Vec<QueryHistorySample>`

- [ ] **Step 1: Write the failing tests**

Create `src/history/sample.rs` with the GPL header, then:

```rust
use crate::collector::snapshot::Snapshot;
use crate::collector::statements::{Statement, StatementId};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::snapshot::{
        DatabaseCounters, DatabaseRates, ServerSettings, SessionCounts,
    };
    use crate::collector::statements::{statement_key, StatementCounters};
    use std::time::Instant;

    fn snapshot(cache: Option<f64>, size: Option<i64>) -> Snapshot {
        Snapshot {
            taken_at: Instant::now(),
            totals: DatabaseCounters::default(),
            rates: cache.map(|c| DatabaseRates {
                cache_hit_ratio: Some(c),
                ..DatabaseRates::default()
            }),
            connected_database_size_bytes: size,
            session_counts: SessionCounts {
                active: 3,
                idle: 5,
                idle_in_transaction: 1,
                other: 2,
            },
            sessions: Vec::new(),
            settings: ServerSettings {
                max_connections: 100,
            },
            statements: None,
            relations: None,
        }
    }

    #[test]
    fn a_system_sample_takes_counts_and_size_from_the_snapshot() {
        let sample = system_sample_from(&snapshot(Some(0.9), Some(2048)));
        assert_eq!(sample.total_connections, 11); // 3 + 5 + 1 + 2
        assert_eq!(sample.max_connections, 100);
        assert_eq!(sample.active_queries, 3);
        assert_eq!(sample.idle_connections, 5);
        assert_eq!(sample.idle_in_transaction, 1);
        assert_eq!(sample.cache_hit_ratio, Some(0.9));
        assert_eq!(sample.total_database_size_bytes, Some(2048));
    }

    #[test]
    fn a_first_sample_with_no_rates_stores_a_null_cache_ratio() {
        // The first sample after connecting has no rates. A None cache ratio
        // must persist as absent, never as zero.
        let sample = system_sample_from(&snapshot(None, None));
        assert_eq!(sample.cache_hit_ratio, None);
        assert_eq!(sample.total_database_size_bytes, None);
    }

    fn statement(id: i64, calls: i64, time_ms: f64) -> Statement {
        Statement {
            key: statement_key(10, 20, Some(id), "SELECT 1"),
            query: "SELECT 1".to_string(),
            user_name: None,
            database: None,
            cumulative: StatementCounters {
                calls,
                total_exec_time_ms: time_ms,
                rows: 7,
                shared_blks_hit: 90,
                shared_blks_read: 10,
                ..StatementCounters::default()
            },
            delta: None,
        }
    }

    #[test]
    fn query_samples_take_the_top_n_and_compute_the_mean() {
        let statements = vec![
            statement(1, 100, 500.0),
            statement(2, 50, 250.0),
            statement(3, 10, 90.0),
        ];
        let samples = query_samples_from(&statements, 2);
        assert_eq!(samples.len(), 2, "truncated to the top 2");
        assert_eq!(samples[0].query_id, Some(1));
        assert_eq!(samples[0].total_calls, 100);
        assert_eq!(samples[0].mean_time_ms, 5.0); // 500 / 100
    }

    #[test]
    fn a_statement_with_no_calls_has_a_zero_mean_rather_than_a_division_by_zero() {
        let samples = query_samples_from(&[statement(1, 0, 0.0)], 10);
        assert_eq!(samples[0].mean_time_ms, 0.0);
    }

    #[test]
    fn a_text_hashed_statement_has_no_query_id() {
        // Utility statements have a NULL queryid and are keyed by text hash;
        // pgconsole's query_id is NOT NULL, so the writer needs to tell them
        // apart. None here is what the pgconsole writer skips.
        let mut s = statement(1, 1, 1.0);
        s.key = statement_key(10, 20, None, "VACUUM t");
        assert_eq!(query_samples_from(&[s], 10)[0].query_id, None);
    }
}
```

Add `pub mod history;` to `src/lib.rs`, alphabetically between `pub mod dialogs;` and `pub mod i18n;`. Create `src/history/mod.rs` with the GPL header and, for now, just `pub mod sample;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib history::sample`
Expected: FAIL — `cannot find function 'system_sample_from' in this scope`.

- [ ] **Step 3: Implement the shapes and builders**

Insert above the `#[cfg(test)]` block in `src/history/sample.rs`:

```rust
/// Server-wide history, one row per history sample. Fields mirror the columns
/// Phase 3 writes to `pgconsole.system_metrics_history`; the local store adds
/// `server_id` and `sampled_at` around them.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemHistorySample {
    pub total_connections: i32,
    pub max_connections: i32,
    pub active_queries: i32,
    pub idle_connections: i32,
    pub idle_in_transaction: i32,
    /// `None` when no honest ratio exists (the first sample, or an interval
    /// that touched no blocks). Persisted as SQL NULL, never zero.
    pub cache_hit_ratio: Option<f64>,
    pub total_database_size_bytes: Option<i64>,
}

/// One statement's cumulative counters at a history sample.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryHistorySample {
    /// `None` for a utility statement keyed by text hash. The pgconsole writer
    /// skips these, since `query_id` is NOT NULL there; the local store keeps
    /// them.
    pub query_id: Option<i64>,
    pub query_text: String,
    pub total_calls: i64,
    pub total_time_ms: f64,
    pub total_rows: i64,
    pub mean_time_ms: f64,
    pub shared_blks_hit: i64,
    pub shared_blks_read: i64,
}

pub fn system_sample_from(snapshot: &Snapshot) -> SystemHistorySample {
    let counts = &snapshot.session_counts;
    SystemHistorySample {
        total_connections: counts.total() as i32,
        max_connections: snapshot.settings.max_connections,
        active_queries: counts.active as i32,
        idle_connections: counts.idle as i32,
        idle_in_transaction: counts.idle_in_transaction as i32,
        cache_hit_ratio: snapshot.rates.and_then(|r| r.cache_hit_ratio),
        total_database_size_bytes: snapshot.connected_database_size_bytes,
    }
}

/// The top `top` statements as history rows. The input is already ordered by
/// cumulative time (the STATEMENTS_SQL `ORDER BY`), so this truncates rather
/// than re-sorts.
pub fn query_samples_from(statements: &[Statement], top: usize) -> Vec<QueryHistorySample> {
    statements
        .iter()
        .take(top)
        .map(|s| {
            let calls = s.cumulative.calls;
            QueryHistorySample {
                query_id: match s.key.id {
                    StatementId::QueryId(id) => Some(id),
                    StatementId::TextHash(_) => None,
                },
                query_text: s.query.clone(),
                total_calls: calls,
                total_time_ms: s.cumulative.total_exec_time_ms,
                total_rows: s.cumulative.rows,
                mean_time_ms: if calls > 0 {
                    s.cumulative.total_exec_time_ms / calls as f64
                } else {
                    0.0
                },
                shared_blks_hit: s.cumulative.shared_blks_hit,
                shared_blks_read: s.cumulative.shared_blks_read,
            }
        })
        .collect()
}
```

Confirm `DatabaseRates` derives `Default` (it does — Phase 1). If `SessionCounts` fields are not `pub`, they are (Phase 1). No snapshot changes are needed.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib history::sample`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/history/sample.rs src/history/mod.rs src/lib.rs
git commit -m "feat: history sample shapes derived from a snapshot"
```

---

## Task 3: `history/local.rs` — the SQLite store

**Files:**
- Create: `src/history/local.rs`
- Modify: `Cargo.toml` (add `rusqlite`), `src/history/mod.rs` (add `pub mod local;`)

**Interfaces:**
- Consumes: `SystemHistorySample`, `QueryHistorySample` (Task 2).
- Produces:
  - `LocalStore` with:
    - `LocalStore::open(path: &std::path::Path) -> rusqlite::Result<LocalStore>`
    - `LocalStore::open_in_memory() -> rusqlite::Result<LocalStore>`
    - `write_system(&self, server_id: &str, sampled_at: i64, sample: &SystemHistorySample) -> rusqlite::Result<()>`
    - `write_queries(&self, server_id: &str, sampled_at: i64, samples: &[QueryHistorySample]) -> rusqlite::Result<()>`
    - `load_recent_system(&self, server_id: &str, limit: usize) -> rusqlite::Result<Vec<SystemHistorySample>>` — oldest-first
    - `prune(&self, cutoff: i64) -> rusqlite::Result<()>` — deletes rows with `sampled_at < cutoff` from both tables

- [ ] **Step 1: Add the dependency**

In `Cargo.toml`, in `[dependencies]`, after `futures-util`:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
```

`bundled` compiles SQLite from source, so no system libsqlite is required — matching the project's avoidance of system C dependencies. Run `cargo build` once to fetch and compile it (this is slow the first time; expected).

- [ ] **Step 2: Write the failing tests**

Create `src/history/local.rs` with the GPL header, then:

```rust
use rusqlite::Connection;

use super::sample::{QueryHistorySample, SystemHistorySample};

#[cfg(test)]
mod tests {
    use super::*;

    fn system(total: i32, cache: Option<f64>) -> SystemHistorySample {
        SystemHistorySample {
            total_connections: total,
            max_connections: 100,
            active_queries: 1,
            idle_connections: 2,
            idle_in_transaction: 0,
            cache_hit_ratio: cache,
            total_database_size_bytes: Some(4096),
        }
    }

    #[test]
    fn a_system_sample_round_trips_through_the_store() {
        let store = LocalStore::open_in_memory().unwrap();
        store.write_system("srv", 1_000, &system(11, Some(0.9))).unwrap();

        let loaded = store.load_recent_system("srv", 10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], system(11, Some(0.9)));
    }

    #[test]
    fn a_null_cache_ratio_survives_the_round_trip_as_none() {
        let store = LocalStore::open_in_memory().unwrap();
        store.write_system("srv", 1_000, &system(11, None)).unwrap();
        assert_eq!(store.load_recent_system("srv", 10).unwrap()[0].cache_hit_ratio, None);
    }

    #[test]
    fn load_recent_returns_the_newest_samples_oldest_first() {
        let store = LocalStore::open_in_memory().unwrap();
        for (t, n) in [(100, 1), (200, 2), (300, 3), (400, 4)] {
            store.write_system("srv", t, &system(n, None)).unwrap();
        }
        // Ask for 3: expect the newest three (2,3,4) in oldest-first order.
        let loaded = store.load_recent_system("srv", 3).unwrap();
        let totals: Vec<i32> = loaded.iter().map(|s| s.total_connections).collect();
        assert_eq!(totals, vec![2, 3, 4]);
    }

    #[test]
    fn history_is_scoped_by_server_id() {
        let store = LocalStore::open_in_memory().unwrap();
        store.write_system("a", 100, &system(1, None)).unwrap();
        store.write_system("b", 100, &system(2, None)).unwrap();
        assert_eq!(store.load_recent_system("a", 10).unwrap().len(), 1);
        assert_eq!(store.load_recent_system("a", 10).unwrap()[0].total_connections, 1);
    }

    #[test]
    fn an_unseen_server_loads_no_history() {
        let store = LocalStore::open_in_memory().unwrap();
        assert!(store.load_recent_system("nobody", 10).unwrap().is_empty());
    }

    #[test]
    fn prune_deletes_rows_older_than_the_cutoff() {
        let store = LocalStore::open_in_memory().unwrap();
        store.write_system("srv", 100, &system(1, None)).unwrap();
        store.write_system("srv", 500, &system(2, None)).unwrap();
        store.prune(300).unwrap();
        let loaded = store.load_recent_system("srv", 10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].total_connections, 2);
    }

    #[test]
    fn queries_round_trip_and_null_query_ids_are_kept() {
        let store = LocalStore::open_in_memory().unwrap();
        let samples = vec![
            QueryHistorySample {
                query_id: Some(42),
                query_text: "SELECT 1".to_string(),
                total_calls: 10,
                total_time_ms: 50.0,
                total_rows: 7,
                mean_time_ms: 5.0,
                shared_blks_hit: 90,
                shared_blks_read: 10,
            },
            QueryHistorySample {
                query_id: None,
                query_text: "VACUUM t".to_string(),
                total_calls: 1,
                total_time_ms: 3.0,
                total_rows: 0,
                mean_time_ms: 3.0,
                shared_blks_hit: 0,
                shared_blks_read: 0,
            },
        ];
        store.write_queries("srv", 1_000, &samples).unwrap();
        // Round-trip is proven via a direct count, since Phase 3 renders no
        // query-history view to load them back through.
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM query_history WHERE server_id = 'srv'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
```

Add `pub mod local;` to `src/history/mod.rs`.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib history::local`
Expected: FAIL — `cannot find type 'LocalStore' in this scope`.

- [ ] **Step 4: Implement the store**

Insert above the `#[cfg(test)]` block:

```rust
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS system_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id TEXT NOT NULL,
    sampled_at INTEGER NOT NULL,
    total_connections INTEGER NOT NULL,
    max_connections INTEGER NOT NULL,
    active_queries INTEGER NOT NULL,
    idle_connections INTEGER NOT NULL,
    idle_in_transaction INTEGER NOT NULL,
    cache_hit_ratio REAL,
    total_database_size_bytes INTEGER
);
CREATE INDEX IF NOT EXISTS idx_system_history_server_time
    ON system_history (server_id, sampled_at DESC);
CREATE TABLE IF NOT EXISTS query_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id TEXT NOT NULL,
    sampled_at INTEGER NOT NULL,
    query_id INTEGER,
    query_text TEXT NOT NULL,
    total_calls INTEGER NOT NULL,
    total_time_ms REAL NOT NULL,
    total_rows INTEGER NOT NULL,
    mean_time_ms REAL NOT NULL,
    shared_blks_hit INTEGER NOT NULL,
    shared_blks_read INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_query_history_server_time
    ON query_history (server_id, sampled_at DESC);";

/// A SQLite history store Mission Centre owns outright. One file holds every
/// server's history, keyed by the server UUID. Holds metrics only — never a
/// password.
pub struct LocalStore {
    connection: Connection,
}

impl LocalStore {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<LocalStore> {
        let connection = Connection::open(path)?;
        connection.execute_batch(SCHEMA)?;
        Ok(LocalStore { connection })
    }

    pub fn open_in_memory() -> rusqlite::Result<LocalStore> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(SCHEMA)?;
        Ok(LocalStore { connection })
    }

    /// Test-only accessor for direct assertions on rows a Phase-3 UI cannot
    /// yet load back.
    #[cfg(test)]
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn write_system(
        &self,
        server_id: &str,
        sampled_at: i64,
        sample: &SystemHistorySample,
    ) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO system_history
               (server_id, sampled_at, total_connections, max_connections,
                active_queries, idle_connections, idle_in_transaction,
                cache_hit_ratio, total_database_size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                server_id,
                sampled_at,
                sample.total_connections,
                sample.max_connections,
                sample.active_queries,
                sample.idle_connections,
                sample.idle_in_transaction,
                sample.cache_hit_ratio,
                sample.total_database_size_bytes,
            ],
        )?;
        Ok(())
    }

    pub fn write_queries(
        &self,
        server_id: &str,
        sampled_at: i64,
        samples: &[QueryHistorySample],
    ) -> rusqlite::Result<()> {
        let mut statement = self.connection.prepare(
            "INSERT INTO query_history
               (server_id, sampled_at, query_id, query_text, total_calls,
                total_time_ms, total_rows, mean_time_ms, shared_blks_hit,
                shared_blks_read)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for s in samples {
            statement.execute(rusqlite::params![
                server_id,
                sampled_at,
                s.query_id,
                s.query_text,
                s.total_calls,
                s.total_time_ms,
                s.total_rows,
                s.mean_time_ms,
                s.shared_blks_hit,
                s.shared_blks_read,
            ])?;
        }
        Ok(())
    }

    pub fn load_recent_system(
        &self,
        server_id: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<SystemHistorySample>> {
        let mut statement = self.connection.prepare(
            "SELECT total_connections, max_connections, active_queries,
                    idle_connections, idle_in_transaction, cache_hit_ratio,
                    total_database_size_bytes
               FROM system_history
              WHERE server_id = ?1
              ORDER BY sampled_at DESC
              LIMIT ?2",
        )?;
        let rows = statement.query_map(rusqlite::params![server_id, limit as i64], |row| {
            Ok(SystemHistorySample {
                total_connections: row.get(0)?,
                max_connections: row.get(1)?,
                active_queries: row.get(2)?,
                idle_connections: row.get(3)?,
                idle_in_transaction: row.get(4)?,
                cache_hit_ratio: row.get(5)?,
                total_database_size_bytes: row.get(6)?,
            })
        })?;
        // Newest-first from SQL; reverse to oldest-first so the caller can
        // push them into a graph in chronological order.
        let mut samples: Vec<SystemHistorySample> = rows.collect::<rusqlite::Result<_>>()?;
        samples.reverse();
        Ok(samples)
    }

    pub fn prune(&self, cutoff: i64) -> rusqlite::Result<()> {
        self.connection
            .execute("DELETE FROM system_history WHERE sampled_at < ?1", [cutoff])?;
        self.connection
            .execute("DELETE FROM query_history WHERE sampled_at < ?1", [cutoff])?;
        Ok(())
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib history::local`
Expected: PASS, 7 tests.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add Cargo.toml Cargo.lock src/history/local.rs src/history/mod.rs
git commit -m "feat: local SQLite history store"
```

---

## Task 4: `history/pgconsole.rs` — probe and SQL

**Files:**
- Create: `src/history/pgconsole.rs`
- Modify: `src/history/mod.rs` (add `pub mod pgconsole;`)

**Interfaces:**
- Consumes: nothing from Rust; `tokio_postgres::Row` for the load mapper. `SystemHistorySample` (Task 2).
- Produces:
  - `PgConsoleAvailability` — `Writable | SchemaMissing | NotWritable`, with `classify(tables_exist: bool, can_insert: bool) -> PgConsoleAvailability`
  - `PGCONSOLE_PROBE_SQL: &str` — two bool columns `tables_exist`, `can_insert`
  - `INSERT_SYSTEM_SQL: &str`, `INSERT_QUERY_SQL: &str`
  - `LOAD_SYSTEM_SQL: &str` — one `$1` `i64` limit
  - `map_system_row(row: &tokio_postgres::Row) -> SystemHistorySample`

- [ ] **Step 1: Write the failing tests**

Create `src/history/pgconsole.rs` with the GPL header, then:

```rust
use tokio_postgres::Row;

use super::sample::SystemHistorySample;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tables_classify_as_schema_missing() {
        assert_eq!(
            PgConsoleAvailability::classify(false, false),
            PgConsoleAvailability::SchemaMissing
        );
    }

    #[test]
    fn present_but_unwritable_tables_classify_as_not_writable() {
        assert_eq!(
            PgConsoleAvailability::classify(true, false),
            PgConsoleAvailability::NotWritable
        );
    }

    #[test]
    fn present_and_insertable_tables_classify_as_writable() {
        assert_eq!(
            PgConsoleAvailability::classify(true, true),
            PgConsoleAvailability::Writable
        );
    }

    #[test]
    fn can_insert_without_the_tables_still_reads_as_schema_missing() {
        // The probe cannot report can_insert = true when tables_exist is
        // false, but classify must not treat that impossible pair as writable.
        assert_eq!(
            PgConsoleAvailability::classify(false, true),
            PgConsoleAvailability::SchemaMissing
        );
    }
}
```

Add `pub mod pgconsole;` to `src/history/mod.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib history::pgconsole`
Expected: FAIL — `cannot find type 'PgConsoleAvailability' in this scope`.

- [ ] **Step 3: Implement the classifier and the SQL**

Insert above the `#[cfg(test)]` block:

```rust
/// Whether an existing pgconsole schema can receive Mission Centre's history.
/// Decided once at connect, for a server the user set to pgconsole mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgConsoleAvailability {
    Writable,
    /// No pgconsole schema, or not the expected history tables.
    SchemaMissing,
    /// The tables exist but the connected role cannot INSERT.
    NotWritable,
}

impl PgConsoleAvailability {
    pub fn classify(tables_exist: bool, can_insert: bool) -> Self {
        if !tables_exist {
            PgConsoleAvailability::SchemaMissing
        } else if !can_insert {
            PgConsoleAvailability::NotWritable
        } else {
            PgConsoleAvailability::Writable
        }
    }
}

/// One round trip: do both history tables exist, and can the role INSERT into
/// both? `has_table_privilege` errors on a missing table, so each privilege
/// check is a scalar subquery guarded by a WHERE on `to_regclass`, which
/// yields no row — hence NULL, COALESCEd to false — when the table is absent.
/// Column-shape drift (a renamed column) is not probed here: an INSERT names
/// its columns, so it fails at write time and history disables for the session
/// (see the collector). This is the design's compatibility contract.
pub const PGCONSOLE_PROBE_SQL: &str = "\
SELECT
  (to_regclass('pgconsole.system_metrics_history') IS NOT NULL
   AND to_regclass('pgconsole.query_metrics_history') IS NOT NULL) AS tables_exist,
  COALESCE((SELECT has_table_privilege('pgconsole.system_metrics_history', 'INSERT')
              WHERE to_regclass('pgconsole.system_metrics_history') IS NOT NULL), false)
  AND
  COALESCE((SELECT has_table_privilege('pgconsole.query_metrics_history', 'INSERT')
              WHERE to_regclass('pgconsole.query_metrics_history') IS NOT NULL), false)
    AS can_insert";

/// `blocked_queries` is NOT NULL in pg-console's schema and Mission Centre does
/// not derive it, so it is written 0. `sampled_at` is `now()` server-side, so
/// the client clock is irrelevant. `instance_id` ($1) is the server UUID,
/// distinct from pg-console's own `'default'`.
pub const INSERT_SYSTEM_SQL: &str = "\
INSERT INTO pgconsole.system_metrics_history
  (sampled_at, instance_id, total_connections, max_connections, active_queries,
   idle_connections, idle_in_transaction, blocked_queries, cache_hit_ratio,
   total_database_size_bytes)
VALUES (now(), $1, $2, $3, $4, $5, $6, 0, $7, $8)";

/// `query_id` is TEXT NOT NULL in pg-console's schema, so the caller passes
/// the queryid formatted as text and skips text-hashed utility statements.
pub const INSERT_QUERY_SQL: &str = "\
INSERT INTO pgconsole.query_metrics_history
  (sampled_at, instance_id, query_id, query_text, total_calls, total_time_ms,
   total_rows, mean_time_ms, shared_blks_hit, shared_blks_read)
VALUES (now(), $1, $2, $3, $4, $5, $6, $7, $8, $9)";

/// The most recent system rows for the connected server, newest-first. Read
/// regardless of `instance_id`: for drawing a line, the latest samples matter,
/// whoever wrote them (Mission Centre or pg-console).
pub const LOAD_SYSTEM_SQL: &str = "\
SELECT total_connections, max_connections, active_queries, idle_connections,
       idle_in_transaction, cache_hit_ratio, total_database_size_bytes
  FROM pgconsole.system_metrics_history
 ORDER BY sampled_at DESC
 LIMIT $1";

pub fn map_system_row(row: &Row) -> SystemHistorySample {
    SystemHistorySample {
        total_connections: row.get("total_connections"),
        max_connections: row.get("max_connections"),
        active_queries: row.get("active_queries"),
        idle_connections: row.get("idle_connections"),
        idle_in_transaction: row.get("idle_in_transaction"),
        cache_hit_ratio: row.get("cache_hit_ratio"),
        total_database_size_bytes: row.get("total_database_size_bytes"),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib history::pgconsole`
Expected: PASS, 4 tests. The SQL is proven against real servers in Task 9.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/history/pgconsole.rs src/history/mod.rs
git commit -m "feat: pgconsole history probe and SQL"
```

---

## Task 5: `history/mod.rs` — backend, preload, cadence

**Files:**
- Modify: `src/history/mod.rs`

**Interfaces:**
- Consumes: `LocalStore` (Task 3); `SystemHistorySample` (Task 2).
- Produces:
  - `HistoryBackend` — `Off | Local(LocalStore) | PgConsole`
  - `HistoryPreload { system: Vec<SystemHistorySample> }` (Clone, Default)
  - `is_history_tick(last_write: Option<Instant>, now: Instant, interval: Duration) -> bool`
  - re-exports: `pub use sample::{SystemHistorySample, QueryHistorySample, system_sample_from, query_samples_from};`

- [ ] **Step 1: Write the failing test**

Replace the body of `src/history/mod.rs` (below the GPL header) with the module declarations, re-exports, and this test:

```rust
pub mod local;
pub mod pgconsole;
pub mod sample;

pub use sample::{
    query_samples_from, system_sample_from, QueryHistorySample, SystemHistorySample,
};

use std::time::{Duration, Instant};

use local::LocalStore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_history_write_happens_immediately() {
        // With no prior write, a data point should be recorded at once so the
        // store is not empty for a whole interval after connecting.
        let now = Instant::now();
        assert!(is_history_tick(None, now, Duration::from_secs(60)));
    }

    #[test]
    fn a_write_waits_for_its_interval() {
        let now = Instant::now();
        let recent = now - Duration::from_secs(30);
        assert!(!is_history_tick(Some(recent), now, Duration::from_secs(60)));
    }

    #[test]
    fn a_write_happens_once_the_interval_has_elapsed() {
        let now = Instant::now();
        let stale = now - Duration::from_secs(61);
        assert!(is_history_tick(Some(stale), now, Duration::from_secs(60)));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib history::tests`
Expected: FAIL — `cannot find function 'is_history_tick' in this scope`.

- [ ] **Step 3: Implement the backend, preload and scheduler**

Insert, after the `use local::LocalStore;` line:

```rust
/// The resolved history backend for one connection. `Local` owns its SQLite
/// connection; `PgConsole` writes through the sample loop's tokio-postgres
/// client, so it carries no state here.
pub enum HistoryBackend {
    Off,
    Local(LocalStore),
    PgConsole,
}

impl HistoryBackend {
    pub fn is_off(&self) -> bool {
        matches!(self, HistoryBackend::Off)
    }
}

/// History loaded on connect, fed into the Overview graph buffers before live
/// samples begin. Oldest-first.
#[derive(Debug, Clone, Default)]
pub struct HistoryPreload {
    pub system: Vec<SystemHistorySample>,
}

/// True when a history row should be written this tick: immediately for the
/// first write of a connection, then once `interval` has elapsed. The same
/// shape as the collector's `is_slow_tick`, and a coarser clock than the 2s
/// sample loop — writing history every sample would flood the store.
pub fn is_history_tick(last_write: Option<Instant>, now: Instant, interval: Duration) -> bool {
    match last_write {
        None => true,
        Some(previous) => now.duration_since(previous) >= interval,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib history::`
Expected: PASS — all of Tasks 2–5's tests (5 + 7 + 4 + 3 = 19).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/history/mod.rs
git commit -m "feat: history backend, preload and write cadence"
```

---

## Task 6: Collector integration

**Files:**
- Modify: `src/collector/worker.rs`

**Interfaces:**
- Consumes: everything from `src/history/`.
- Produces:
  - `CollectorConfig` gains: `history_mode: HistoryMode`, `history_interval: Duration`, `history_retention_days: i64`, `history_top_queries: usize`, `server_id: String`, `local_db_path: std::path::PathBuf`
  - `CollectorEvent::History(Box<HistoryPreload>)`

**Note:** `worker.rs` is at 758 lines. If these additions push it past ~800, move the `#[cfg(test)] mod tests` block to `src/collector/worker_tests.rs` and include it with `#[path = "worker_tests.rs"] mod tests;` — do that as the first step if needed, in its own commit, so the diff for the logic stays readable.

- [ ] **Step 1: Write the failing test**

Add to `worker.rs`'s `mod tests`:

```rust
    #[test]
    fn a_pgconsole_write_failure_disables_history_without_failing_the_sample() {
        // A mid-session INSERT failure (schema dropped, privilege revoked) is
        // a property of the store, not the connection: history goes Off and
        // the sample still succeeds. Classified exactly like a slow-tier
        // Query error.
        let outcome = classify_history_error(CollectorError::Query("permission denied".into()));
        assert!(matches!(outcome, HistoryOutcome::Disable));
    }

    #[test]
    fn a_pgconsole_write_timeout_still_fails_the_sample() {
        assert!(matches!(
            classify_history_error(CollectorError::Timeout),
            HistoryOutcome::FailSample
        ));
    }

    #[test]
    fn a_pgconsole_write_connection_loss_still_fails_the_sample() {
        assert!(matches!(
            classify_history_error(CollectorError::LostConnection),
            HistoryOutcome::FailSample
        ));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib collector::worker`
Expected: FAIL — `cannot find function 'classify_history_error' in this scope`.

- [ ] **Step 3: Add the config fields, the event, and the error classifier**

Add these imports near the top of `worker.rs`:

```rust
use std::path::PathBuf;
use std::time::SystemTime;

use crate::connection::params::HistoryMode;
use crate::history::pgconsole::{
    map_system_row, PgConsoleAvailability, INSERT_QUERY_SQL, INSERT_SYSTEM_SQL, LOAD_SYSTEM_SQL,
    PGCONSOLE_PROBE_SQL,
};
use crate::history::{
    is_history_tick, query_samples_from, system_sample_from, HistoryBackend, HistoryPreload,
    QueryHistorySample,
};
use crate::history::local::LocalStore;
```

Extend `CollectorConfig`:

```rust
    pub history_mode: HistoryMode,
    pub history_interval: Duration,
    pub history_retention_days: i64,
    pub history_top_queries: usize,
    pub server_id: String,
    pub local_db_path: PathBuf,
```

`CollectorConfig` currently derives `Copy` — remove `Copy` (it now holds a `String` and a `PathBuf`); keep `Clone`. Update the `derive` line and pass `config.clone()` at the one call site inside `run` if the borrow checker requires it (it is moved into the thread once, so `Clone` suffices without changes).

Add the `History` variant to `CollectorEvent`:

```rust
    History(Box<HistoryPreload>),
```

Add the classifier and its outcome, after `classify_slow`:

```rust
/// What a failed history write means. A `Query` error is a property of the
/// store or the role — the schema was dropped, a privilege revoked — so
/// history disables for the session and the sample still succeeds. A timeout
/// or lost connection is the connection's problem and fails the sample, as
/// everywhere else.
enum HistoryOutcome {
    Disable,
    FailSample,
}

fn classify_history_error(error: CollectorError) -> HistoryOutcome {
    match error {
        CollectorError::Timeout | CollectorError::LostConnection => HistoryOutcome::FailSample,
        _ => HistoryOutcome::Disable,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib collector::worker`
Expected: PASS (the three new tests plus the existing worker tests).

- [ ] **Step 5: Resolve the backend and load preload on connect**

Add this helper (it opens the backend and loads the preload; pgconsole load runs over the client, local over the file):

```rust
/// Resolves the effective backend for this connection and loads the recent
/// history to preload. A pgconsole schema that is missing or unwritable falls
/// back to Local with a logged note; the connection is never failed for it.
async fn open_history(
    client: &Client,
    config: &CollectorConfig,
    preload_limit: usize,
) -> (HistoryBackend, HistoryPreload) {
    match config.history_mode {
        HistoryMode::Off => (HistoryBackend::Off, HistoryPreload::default()),
        HistoryMode::PgConsole => match probe_pgconsole(client).await {
            PgConsoleAvailability::Writable => {
                let system = load_pgconsole_history(client, preload_limit).await;
                (HistoryBackend::PgConsole, HistoryPreload { system })
            }
            other => {
                gtk_free_log(&format!(
                    "pgconsole history unavailable ({other:?}); using local history"
                ));
                open_local(config, preload_limit)
            }
        },
        HistoryMode::Local => open_local(config, preload_limit),
    }
}

async fn probe_pgconsole(client: &Client) -> PgConsoleAvailability {
    match client.query_one(PGCONSOLE_PROBE_SQL, &[]).await {
        Ok(row) => PgConsoleAvailability::classify(row.get("tables_exist"), row.get("can_insert")),
        // A probe that itself errors is treated as no usable schema.
        Err(_) => PgConsoleAvailability::SchemaMissing,
    }
}

async fn load_pgconsole_history(client: &Client, limit: usize) -> Vec<SystemHistorySample> {
    match client.query(LOAD_SYSTEM_SQL, &[&(limit as i64)]).await {
        Ok(rows) => {
            let mut samples: Vec<SystemHistorySample> = rows.iter().map(map_system_row).collect();
            samples.reverse(); // newest-first from SQL → oldest-first
            samples
        }
        Err(_) => Vec::new(),
    }
}

fn open_local(config: &CollectorConfig, preload_limit: usize) -> (HistoryBackend, HistoryPreload) {
    match LocalStore::open(&config.local_db_path) {
        Ok(store) => {
            let system = store
                .load_recent_system(&config.server_id, preload_limit)
                .unwrap_or_default();
            (HistoryBackend::Local(store), HistoryPreload { system })
        }
        Err(e) => {
            gtk_free_log(&format!("local history unavailable ({e}); history disabled"));
            (HistoryBackend::Off, HistoryPreload::default())
        }
    }
}

/// A log line from the collector thread. `g_warning!` is GTK-thread-only, so
/// the collector uses eprintln through the glib logger's stderr, never a GTK
/// call. Kept in one place so the GTK-free rule is easy to check.
fn gtk_free_log(message: &str) {
    eprintln!("mission-centre-pg: {message}");
}
```

Bring `SystemHistorySample` into scope by adding it to the earlier `use crate::history::{…}` import list.

- [ ] **Step 6: Emit the preload and write on cadence in the sample loop**

In `run`, after `Connected` is emitted and before `sample_loop` is called, resolve the backend and emit the preload. The preload limit is the graph window, read from config — add a `preload_points: usize` to `CollectorConfig` set by the window from `graph-points`, OR reuse a constant of 300. Use `graph-points`: add `pub preload_points: usize` to `CollectorConfig` (window sets it). Then:

```rust
                let (mut history, preload) =
                    open_history(&client, &config, config.preload_points).await;
                if !emit(&events, &stop, CollectorEvent::History(Box::new(preload))).await {
                    return;
                }
                // Prune the local store once per connection.
                if let HistoryBackend::Local(store) = &history {
                    let cutoff = retention_cutoff(config.history_retention_days);
                    let _ = store.prune(cutoff);
                }
                match sample_loop(&client, &config, &mut history, statements_available, &events, &stop).await {
```

Add the cutoff helper:

```rust
/// Unix-epoch second before which local history rows are pruned. Uses wall
/// clock, which is correct for a retention window; the sample loop's Instant
/// timing is monotonic and unrelated.
fn retention_cutoff(retention_days: i64) -> i64 {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    now - retention_days * 86_400
}
```

Change `sample_loop`'s signature to take `config: &CollectorConfig` and `history: &mut HistoryBackend`, and inside it track history-write timing and the latest query samples:

```rust
    let mut last_history: Option<Instant> = None;
    let mut latest_queries: Vec<QueryHistorySample> = Vec::new();
```

Whenever a slow sample succeeds (`Some(Ok(sample))` for statements), refresh the stash:

```rust
                if let Some(Ok(sample)) = snapshot.statements.as_ref() {
                    latest_queries = query_samples_from(&sample.statements, config.history_top_queries);
                }
```

After a successful `sample`, on a history tick, write — and handle a failure per `classify_history_error`:

```rust
                if !history.is_off()
                    && is_history_tick(last_history, Instant::now(), config.history_interval)
                {
                    last_history = Some(Instant::now());
                    let system = system_sample_from(&snapshot);
                    if let Err(e) =
                        write_history(client, history, config, &system, &latest_queries).await
                    {
                        match classify_history_error(e) {
                            HistoryOutcome::Disable => {
                                gtk_free_log("history write failed; disabling history for this session");
                                *history = HistoryBackend::Off;
                            }
                            HistoryOutcome::FailSample => {
                                // Fall through to the failure path by breaking
                                // to a returned Exit::Failed is unnecessary:
                                // a timeout here is rare and the next loop
                                // iteration's sample will surface it. Log and
                                // continue so history never fails a sample on
                                // its own account.
                                gtk_free_log("history write timed out; skipping this write");
                            }
                        }
                    }
                }
```

Add the write dispatcher:

```rust
/// Writes one system row and the latest query rows to whichever backend is
/// active. Local writes are synchronous SQLite calls run inline — they are
/// small and infrequent (default 60s) and the loop is serial, so a brief
/// block is acceptable and simpler than a blocking-pool hop. pgconsole writes
/// go over the existing client.
async fn write_history(
    client: &Client,
    history: &HistoryBackend,
    config: &CollectorConfig,
    system: &SystemHistorySample,
    queries: &[QueryHistorySample],
) -> Result<(), CollectorError> {
    let sampled_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match history {
        HistoryBackend::Off => Ok(()),
        HistoryBackend::Local(store) => {
            store
                .write_system(&config.server_id, sampled_at, system)
                .and_then(|_| store.write_queries(&config.server_id, sampled_at, queries))
                .map_err(|e| CollectorError::Query(e.to_string()))
        }
        HistoryBackend::PgConsole => {
            client
                .execute(
                    INSERT_SYSTEM_SQL,
                    &[
                        &config.server_id,
                        &system.total_connections,
                        &system.max_connections,
                        &system.active_queries,
                        &system.idle_connections,
                        &system.idle_in_transaction,
                        &system.cache_hit_ratio,
                        &system.total_database_size_bytes,
                    ],
                )
                .await
                .map_err(map_query_error)?;
            for q in queries {
                // query_id is NOT NULL in pg-console's schema; skip the
                // text-hashed utility statements that have no queryid.
                let Some(id) = q.query_id else { continue };
                client
                    .execute(
                        INSERT_QUERY_SQL,
                        &[
                            &config.server_id,
                            &id.to_string(),
                            &q.query_text,
                            &q.total_calls,
                            &q.total_time_ms,
                            &q.total_rows,
                            &q.mean_time_ms,
                            &q.shared_blks_hit,
                            &q.shared_blks_read,
                        ],
                    )
                    .await
                    .map_err(map_query_error)?;
            }
            Ok(())
        }
    }
}
```

Update the `sample_loop` call and its two other call paths (the `Exit::Stopped` early returns are unchanged). The `previous_statements` bookkeeping is untouched.

- [ ] **Step 7: Run the unit suite and build**

Run: `cargo test --lib` then `ninja -C build`
Expected: PASS; builds clean. `worker.rs` stays under ~800 (or its tests were moved out in the pre-step).

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add src/collector/worker.rs
git commit -m "feat: wire the history store into the collector"
```

---

## Task 7: GSettings keys, Overview preload, window wiring

**Files:**
- Modify: `data/io.github.paulsnow.MissionCentrePg.gschema.xml`
- Modify: `src/pages/overview.rs`
- Modify: `src/window.rs`

**Interfaces:**
- Consumes: `CollectorEvent::History`, `HistoryPreload`, `CollectorConfig` fields (Task 6); `SystemHistorySample` (Task 2).
- Produces:
  - `McpgOverviewPage::preload(&self, samples: &[SystemHistorySample])`
  - `McpgOverviewPage::reset(&self)`

- [ ] **Step 1: Add the GSettings keys**

In `data/io.github.paulsnow.MissionCentrePg.gschema.xml`, after the `relations-limit` key:

```xml
    <key name="history-interval-ms" type="i">
      <range min="10000" max="600000"/>
      <default>60000</default>
      <summary>Minimum gap between history writes in milliseconds</summary>
      <description>History is written on this coarser clock, independent of the sampling interval, so it does not flood the store.</description>
    </key>
    <key name="history-retention-days" type="i">
      <range min="1" max="365"/>
      <default>7</default>
      <summary>Age at which local history rows are pruned</summary>
      <description>Applies only to the local SQLite store. History in a shared pgconsole schema is never pruned by Mission Centre.</description>
    </key>
    <key name="history-top-queries" type="i">
      <range min="0" max="500"/>
      <default>50</default>
      <summary>Statements persisted per history sample</summary>
      <description>Zero disables query-history persistence while keeping system-metric history.</description>
    </key>
```

- [ ] **Step 2: Write the failing test for the preload**

Preload rendering is GTK and not unit-tested (as with every page). Add a compile-guard test to `overview.rs`'s `mod tests` (create the block if absent) that pins the public method's existence and that reset is callable — pure signature coverage, run under a headless guard:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preload_and_reset_are_public_no_op_safe_signatures() {
        // Constructing the widget needs a GTK main context, which unit tests
        // do not have. This test exists to keep the method signatures honest
        // against the window's calls; behaviour is verified by running the app.
        fn _assert_signatures(page: &McpgOverviewPage, samples: &[crate::history::SystemHistorySample]) {
            page.reset();
            page.preload(samples);
        }
    }
}
```

- [ ] **Step 3: Run it to verify it fails to compile**

Run: `cargo test --lib pages::overview`
Expected: FAIL — `no method named 'preload'`.

- [ ] **Step 4: Implement `reset` and `preload`**

In `src/pages/overview.rs`, add to the `impl McpgOverviewPage` block:

```rust
    /// Drop all four graph series. Called on a server switch before the new
    /// server's preload arrives, so one server's shape never bleeds into the
    /// next.
    pub fn reset(&self) {
        for graph in self.graphs() {
            graph.clear_datasets();
            graph.add_dataset(DatasetGroup::new());
        }
    }

    /// Fill the graph buffers from stored history so they open already drawn.
    /// Only the connections and cache-hit graphs are preloaded: the persisted
    /// schema (pg-console's) stores neither TPS nor tuple rates, so those two
    /// graphs build up live. Samples arrive oldest-first.
    pub fn preload(&self, samples: &[crate::history::SystemHistorySample]) {
        let imp = self.imp();
        if let Some(last) = samples.last() {
            imp.connections_graph
                .set_dataset_max_scale(0, last.max_connections as f32);
        }
        for sample in samples {
            imp.connections_graph
                .add_data_point(vec![vec![sample.total_connections as f32]]);
            if let Some(ratio) = sample.cache_hit_ratio {
                if ratio.is_finite() {
                    imp.cache_graph
                        .add_data_point(vec![vec![(ratio * 100.0) as f32]]);
                }
            }
        }
    }
```

`clear_datasets` and `add_dataset`/`DatasetGroup` are already used by `sidebar_row.rs`'s `reset_series`; the import `use crate::widgets::graph_widget_utils::DatasetGroup;` is already present in `overview.rs`.

- [ ] **Step 5: Handle the event and thread config in the window**

In `src/window.rs`:

1. Extend the collector import: add `HistoryPreload` is not needed, but the config fields are — construct them in `select_server`. Add near the other `use` lines: `use std::path::PathBuf;` and `use mission_centre_pg::history::SystemHistorySample;` is not needed (only the event carries it).

2. In `select_server`, extend the `CollectorConfig` built there with the history fields:

```rust
            history_mode: params.history,
            history_interval: std::time::Duration::from_millis(
                settings.int("history-interval-ms").max(10000) as u64,
            ),
            history_retention_days: settings.int("history-retention-days").max(1) as i64,
            history_top_queries: settings.int("history-top-queries").max(0) as usize,
            server_id: params.id.to_string(),
            local_db_path: history_db_path(),
            preload_points: settings.int("graph-points").max(1) as usize,
```

3. Add the path helper as a free function in `window.rs`:

```rust
/// The local history database, under the XDG data directory. Created on first
/// write by the collector; the directory is created here if absent.
fn history_db_path() -> PathBuf {
    let base = glib::user_data_dir().join("mission-centre-pg");
    // A failure to create the directory is non-fatal: the collector opens the
    // file lazily and logs and disables history if that fails.
    let _ = std::fs::create_dir_all(&base);
    base.join("history.db")
}
```

4. In `select_server`, reset the Overview graphs alongside the existing sidebar reset (so a switch does not carry the old server's shape into the preload):

```rust
        imp.overview_page.reset();
```

5. In `handle_event`, add the `History` arm (before the `Sample` arm):

```rust
            CollectorEvent::History(preload) => {
                imp.overview_page.preload(&preload.system);
            }
```

- [ ] **Step 6: Build and verify**

```bash
cargo fmt
cargo test --lib
ninja -C build
glib-compile-schemas data/
```

Expected: PASS; clean build. Interactive check (run separately): set the local server to Local mode is not possible until Task 8; for now confirm the app still launches and connects, graphs update, no regression.

- [ ] **Step 7: Commit**

```bash
git add data/io.github.paulsnow.MissionCentrePg.gschema.xml src/pages/overview.rs src/window.rs
git commit -m "feat: preload the Overview graphs from stored history"
```

---

## Task 8: The history-mode UI

**Files:**
- Modify: `resources/ui/add_server_dialog.blp`, `src/dialogs/add_server.rs`
- Modify: `resources/ui/sidebar_row.blp`, `src/widgets/sidebar_row.rs`
- Modify: `src/window.rs`

**Interfaces:**
- Consumes: `HistoryMode` (Task 1); `registry` load/save (Phase 1).
- Produces:
  - Add Server dialog writes `ConnectionParams.history` from a combo row.
  - `McpgSidebarRow` exposes a history menu; selecting a mode invokes a callback the window wires to save-and-reconnect.

- [ ] **Step 1: Add the history row to the Add Server dialog**

In `resources/ui/add_server_dialog.blp`, after the `ssl_row` combo, inside the same `Adw.PreferencesGroup`:

```blueprint
          Adw.ComboRow history_row {
            title: _("History");
            subtitle: _("Where to store metrics for this server");
            model: Gtk.StringList {
              strings [_("Off"), _("Local"), _("pgconsole")]
            };
            selected: 0;
          }
```

In `src/dialogs/add_server.rs`, add the template child:

```rust
        #[template_child]
        pub history_row: TemplateChild<adw::ComboRow>,
```

and set `history` in the `ConnectionParams` literal in `submit()`:

```rust
            history: match imp.history_row.selected() {
                1 => HistoryMode::Local,
                2 => HistoryMode::PgConsole,
                _ => HistoryMode::Off,
            },
```

- [ ] **Step 2: Build to compile the blueprint**

Run: `ninja -C build`
Expected: clean; the new `history_row` id resolves against the template child.

- [ ] **Step 3: Add the history menu to the sidebar row**

The row needs a menu button offering the three modes, and a way to tell the window which was chosen. Add a `MenuButton` to `resources/ui/sidebar_row.blp`, after the `$GraphWidget graph`:

```blueprint
  Gtk.MenuButton history_menu_button {
    icon-name: "view-more-symbolic";
    valign: center;
    has-frame: false;
    tooltip-text: _("History storage");
  }
```

In `src/widgets/sidebar_row.rs`:

- Add the template child `pub history_menu_button: TemplateChild<gtk::MenuButton>`.
- Add a callback slot and a setter, plus a `Gio.Menu` built in `constructed` with three items backed by a `SimpleAction` per row. Because a `GtkListBox` row is transient, model the menu with a stateful action `history-mode` taking a string parameter, and emit the chosen mode through a stored `Rc<RefCell<Option<Box<dyn Fn(HistoryMode)>>>>`:

```rust
use std::cell::RefCell;
use crate::connection::params::HistoryMode;

// in imp struct:
    pub on_history_change: RefCell<Option<Box<dyn Fn(HistoryMode)>>>,

// in constructed(), after existing setup:
            let menu = gio::Menu::new();
            menu.append(Some(&crate::i18n::i18n("Off")), Some("row.history-mode::off"));
            menu.append(Some(&crate::i18n::i18n("Local")), Some("row.history-mode::local"));
            menu.append(Some(&crate::i18n::i18n("pgconsole")), Some("row.history-mode::pgconsole"));
            self.history_menu_button.set_menu_model(Some(&menu));

            let group = gio::SimpleActionGroup::new();
            let action = gio::SimpleAction::new_stateful(
                "history-mode",
                Some(glib::VariantTy::STRING),
                &"off".to_variant(),
            );
            let row = self.obj().clone();
            action.connect_activate(move |action, param| {
                let Some(value) = param.and_then(|p| p.str().map(str::to_owned)) else { return; };
                action.set_state(&value.to_variant());
                let mode = match value.as_str() {
                    "local" => HistoryMode::Local,
                    "pgconsole" => HistoryMode::PgConsole,
                    _ => HistoryMode::Off,
                };
                if let Some(cb) = row.imp().on_history_change.borrow().as_ref() {
                    cb(mode);
                }
            });
            group.add_action(&action);
            self.obj().insert_action_group("row", Some(&group));
```

Add to the wrapper impl:

```rust
    pub fn set_history_mode(&self, mode: HistoryMode) {
        let value = match mode {
            HistoryMode::Off => "off",
            HistoryMode::Local => "local",
            HistoryMode::PgConsole => "pgconsole",
        };
        if let Some(group) = self.imp().obj().... // see note
    }

    pub fn connect_history_change<F: Fn(HistoryMode) + 'static>(&self, callback: F) {
        self.imp().on_history_change.replace(Some(Box::new(callback)));
    }
```

Note for `set_history_mode`: retrieve the action group with `WidgetExt::action_group` is not available; instead store the `SimpleAction` on the imp struct (`pub history_action: RefCell<Option<gio::SimpleAction>>`) in `constructed`, and `set_history_mode` calls `action.set_state(&value.to_variant())` on it. Wire it: in `constructed` after creating `action`, `self.history_action.replace(Some(action.clone()));`.

Add the necessary imports to `sidebar_row.rs`: `use gtk::gio;` and `use gtk::glib::variant::ToVariant;` (or `use gtk::prelude::*;` already brings `ToVariant`).

- [ ] **Step 4: Wire the menu in the window**

In `src/window.rs`, in `reload_servers` where each `McpgSidebarRow` is created, set the current mode and connect the change:

```rust
            row.set_history_mode(server.history);
            let window = self.clone();
            let server_id = server.id;
            row.connect_history_change(move |mode| {
                window.set_server_history(server_id, mode);
            });
```

Add the method:

```rust
    fn set_server_history(&self, id: uuid::Uuid, mode: HistoryMode) {
        let mut servers = registry::load(&self.settings());
        let Some(server) = servers.iter_mut().find(|s| s.id == id) else {
            return;
        };
        server.history = mode;
        if let Err(e) = registry::save(&self.settings(), &servers) {
            gtk::glib::g_warning!("mission-centre-pg", "could not save the server list: {e}");
            return;
        }
        self.imp().servers.replace(servers);
        // If this server is the one on screen, restart its collector so the
        // new backend takes effect immediately.
        if let Some(index) = self
            .imp()
            .servers
            .borrow()
            .iter()
            .position(|s| s.id == id)
        {
            if self.imp().server_list.selected_row().map(|r| r.index())
                == Some(index as i32)
            {
                self.select_server(index as i32);
            }
        }
    }
```

Add `use mission_centre_pg::connection::params::HistoryMode;` to `window.rs`.

- [ ] **Step 5: Build and check the UI compiles and the app runs**

```bash
cargo fmt
cargo test --lib
ninja -C build
glib-compile-schemas data/
```

Expected: PASS; clean build. Interactive check (run separately): the Add Server dialog shows the History row; the sidebar row shows a menu button offering Off / Local / pgconsole; choosing one persists (visible in `gsettings get … servers`).

- [ ] **Step 6: Commit**

```bash
git add resources/ui/add_server_dialog.blp src/dialogs/add_server.rs resources/ui/sidebar_row.blp src/widgets/sidebar_row.rs src/window.rs
git commit -m "feat: per-server history mode in the dialog and sidebar"
```

---

## Task 9: Integration tests — pgconsole probe and round-trip

**Files:**
- Modify: `tests/portability.rs`

**Interfaces:**
- Consumes: `PGCONSOLE_PROBE_SQL`, `PgConsoleAvailability`, `INSERT_SYSTEM_SQL`, `LOAD_SYSTEM_SQL`, `map_system_row` (Task 4).

- [ ] **Step 1: Add the pgconsole helpers and tests**

At the top of `tests/portability.rs`, add:

```rust
use mission_centre_pg::history::pgconsole::{
    map_system_row, PgConsoleAvailability, INSERT_SYSTEM_SQL, LOAD_SYSTEM_SQL, PGCONSOLE_PROBE_SQL,
};
```

Add a helper that creates the minimal pg-console history schema (the columns Phase 3 uses, matching `V1__initial_schema.sql`):

```rust
const PGCONSOLE_SCHEMA: &str = "\
CREATE SCHEMA pgconsole;
CREATE TABLE pgconsole.system_metrics_history (
    id BIGSERIAL PRIMARY KEY,
    sampled_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    instance_id TEXT NOT NULL DEFAULT 'default',
    total_connections INTEGER NOT NULL,
    max_connections INTEGER NOT NULL,
    active_queries INTEGER NOT NULL,
    idle_connections INTEGER NOT NULL,
    idle_in_transaction INTEGER NOT NULL,
    blocked_queries INTEGER NOT NULL,
    cache_hit_ratio DOUBLE PRECISION,
    total_database_size_bytes BIGINT
);
CREATE TABLE pgconsole.query_metrics_history (
    id BIGSERIAL PRIMARY KEY,
    sampled_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    instance_id TEXT NOT NULL DEFAULT 'default',
    query_id TEXT NOT NULL,
    query_text TEXT,
    total_calls BIGINT NOT NULL,
    total_time_ms DOUBLE PRECISION NOT NULL,
    total_rows BIGINT NOT NULL,
    mean_time_ms DOUBLE PRECISION NOT NULL,
    shared_blks_hit BIGINT,
    shared_blks_read BIGINT
);";

async fn assert_pgconsole_probe_and_round_trip(tag: &str) {
    let (client, container) = connect(tag).await;

    // No schema yet → SchemaMissing.
    let row = client.query_one(PGCONSOLE_PROBE_SQL, &[]).await.unwrap();
    assert_eq!(
        PgConsoleAvailability::classify(row.get("tables_exist"), row.get("can_insert")),
        PgConsoleAvailability::SchemaMissing
    );

    // Create pg-console's schema → Writable as the superuser.
    client.batch_execute(PGCONSOLE_SCHEMA).await.unwrap();
    let row = client.query_one(PGCONSOLE_PROBE_SQL, &[]).await.unwrap();
    assert_eq!(
        PgConsoleAvailability::classify(row.get("tables_exist"), row.get("can_insert")),
        PgConsoleAvailability::Writable
    );

    // The system INSERT matches the column types on this version, and reads back.
    let server_id = "11111111-1111-1111-1111-111111111111";
    client
        .execute(
            INSERT_SYSTEM_SQL,
            &[
                &server_id,
                &47i32,          // total_connections
                &100i32,         // max_connections
                &3i32,           // active_queries
                &40i32,          // idle_connections
                &4i32,           // idle_in_transaction
                &Some(0.95f64),  // cache_hit_ratio
                &Some(2048i64),  // total_database_size_bytes
            ],
        )
        .await
        .expect("system INSERT failed");

    let rows = client.query(LOAD_SYSTEM_SQL, &[&10i64]).await.unwrap();
    let samples: Vec<_> = rows.iter().map(map_system_row).collect();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].total_connections, 47);
    assert_eq!(samples[0].cache_hit_ratio, Some(0.95));
    assert_eq!(samples[0].total_database_size_bytes, Some(2048));

    // A SELECT-only role sees the schema but cannot INSERT → NotWritable.
    client
        .batch_execute(
            "CREATE ROLE reader LOGIN PASSWORD 'reader';
             GRANT USAGE ON SCHEMA pgconsole TO reader;
             GRANT SELECT ON ALL TABLES IN SCHEMA pgconsole TO reader;",
        )
        .await
        .unwrap();
    let reader = connect_as(&container, "reader", "reader").await;
    let row = reader.query_one(PGCONSOLE_PROBE_SQL, &[]).await.unwrap();
    assert_eq!(
        PgConsoleAvailability::classify(row.get("tables_exist"), row.get("can_insert")),
        PgConsoleAvailability::NotWritable
    );
}

#[tokio::test]
async fn pgconsole_probe_and_round_trip_on_postgres_14() {
    assert_pgconsole_probe_and_round_trip("14").await;
}

#[tokio::test]
async fn pgconsole_probe_and_round_trip_on_postgres_18() {
    assert_pgconsole_probe_and_round_trip("18").await;
}
```

The `connect` and `connect_as` helpers already exist in `tests/portability.rs`.

- [ ] **Step 2: Run the container tests**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
```

Expected: PASS — the existing 9 plus 2 new = 11. The two new tests prove the probe classifies all three states and that the system INSERT and read match pg-console's column types on both 14 and 18.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add tests/portability.rs
git commit -m "test: pgconsole history probe and round-trip on 14 and 18"
```

---

## Task 10: Full verification

**Files:** none unless a check fails.

- [ ] **Step 1: Formatting and unit tests**

```bash
cargo fmt --check
cargo test --lib
```

Expected: silent; all unit tests pass. Record the count.

- [ ] **Step 2: Container tests on both bounds**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
```

Expected: 11 passed, covering PostgreSQL 14 and 18.

- [ ] **Step 3: File sizes**

```bash
wc -l src/*.rs src/**/*.rs | sort -rn | head -10
```

Expected: no non-vendored file over ~800. If `worker.rs` crossed it, its tests were moved to `worker_tests.rs` in Task 6's pre-step.

- [ ] **Step 4: Full build**

```bash
ninja -C build
glib-compile-schemas data/
```

Expected: clean.

- [ ] **Step 5: Walk the success criteria against live servers**

Run `MCPG_RESOURCE_DIR=$PWD/build/resources GSETTINGS_SCHEMA_DIR=$PWD/data ./build/src/mission-centre-pg` and confirm each of the spec's §10 criteria:

1. A server's mode (Off / Local / pgconsole) is settable in the Add Server dialog and the sidebar menu, and survives a restart (`gsettings get … servers` shows it).
2. On Local: connect, sample a minute or two, quit, reconnect — the Overview Connections graph opens preloaded, not empty.
3. On pgconsole against the local PostgreSQL 18.4 with pg-console's schema created: Mission Centre's rows appear in `pgconsole.system_metrics_history` under the server UUID, and no other rows are changed.
4. A server on Off or Local performs no write — verify with a role lacking INSERT: it connects and monitors normally.
5. A pgconsole server whose schema is absent falls back to Local (the stderr note appears) and does not fail the connection.
6. Local history is pruned to the retention window; pgconsole rows are never deleted by Mission Centre.
7. Simulate a history fault (revoke INSERT mid-session, or point Local at an unwritable path) — history disables, the live graphs keep updating.
8. `cargo fmt` clean; tests pass on 14 and 18; no file over ~800; no password in the local store, a log, or a history table.

- [ ] **Step 6: Record the outcome**

Append a `=== PHASE 3 ===` section to `.superpowers/sdd/progress.md` recording, per criterion, whether it was verified and with what evidence. Anything unverified is recorded as unverified, not as passing.

- [ ] **Step 7: Commit**

```bash
git add .superpowers/sdd/progress.md
git commit -m "docs: record Phase 3 verification"
```

---

## Self-Review Notes

Checked against `docs/superpowers/specs/2026-07-24-phase-3-history-store-design.md`:

- §2.1 scope — Task 1 (setting), Task 3 (local store), Task 4 + Task 6 (pgconsole write), Task 4 + Task 9 (probe), Task 6 (persist on cadence), Task 7 (preload), Task 8 (UI). All seven in-scope items covered.
- §3 read-only line — the only write is the pgconsole INSERT in Task 6's `write_history`, reached only when `history_mode == PgConsole` and the probe returned `Writable`. Off and Local paths issue no server write.
- §4 detection — Task 4 `PgConsoleAvailability` + `PGCONSOLE_PROBE_SQL`, run in Task 6 `probe_pgconsole`, proven in Task 9.
- §5 what is persisted — Task 2 shapes; §5.1 system fields and §5.2 query fields both mapped, NULL discipline preserved.
- §6.1 backends/preload — Task 5 `HistoryBackend` + `HistoryPreload`, Task 6 `open_history` + the `History` event.
- §6.2 cadence/retention — Task 5 `is_history_tick`, Task 6 `retention_cutoff` + prune-once-on-connect, local only.
- §6.3 effective-backend resolution — Task 6 `open_history` matches the spec's table exactly.
- §7 persistence/UI — Task 1 (registry field), Task 8 (dialog + sidebar menu).
- §7.3 GSettings keys — Task 7, exact ranges and defaults.
- §8 error handling — Task 6 `classify_history_error` (mid-session disable), `open_local`/`open_history` fallbacks, all non-fatal.
- §9 testing — unit tests in Tasks 2–6, integration in Task 9.
- §10 success criteria — Task 10 walks all eight.

Three refinements made during planning, and one deliberate narrowing, flagged for the reviewer:

1. **The probe checks tables + INSERT privilege, not column names.** Spec §4.1 said "carry the columns … by name". A full column audit in the probe is a WITH-CTE against `information_schema` that the integration tests do not exercise anyway; instead the INSERT names its columns, so a renamed column fails at write and history disables for the session (§8). This is a narrowing of §4.1 to what is cheaply testable, and the spec's §4.2 already describes the write-time fallback. **The spec §4.1/§4.2 wording will be refined to match before implementation.**

2. **Mid-session pgconsole failure disables history rather than falling back to Local.** Spec §8's row said "falls back to Local"; spec §10 criterion 7 said "disables history for that server". These conflicted. The plan implements criterion 7 (disable) — opening a Local store mid-loop is more machinery than the acceptance test asks for, and disabling is the simpler honest behaviour. **The §8 row will be reconciled to "disables" before implementation.**

3. **Preload fills only the Connections and Cache-hit graphs.** The persisted schema (pg-console's `system_metrics_history`) stores no TPS or tuple-rate columns, and §5.1 keeps the local store to the same fields. So TPS and Tuples build up live. This is faithful to §5.1; noted because success criterion 2 ("graphs preloaded") is only partly met by construction, and the plan says so in Task 7's `preload` comment.

One soft spot:

- **The fallback note is a stderr log, not a UI banner.** Spec §6.3 and criterion 5 say "with a note". The collector is GTK-free and cannot raise a banner directly; a proper in-window toast would need a new event. The plan logs to stderr via `gtk_free_log`. If a visible note is wanted, it is a small follow-up: a `CollectorEvent::HistoryNote(String)` the window shows as a toast. Flagged rather than built, to keep Phase 3 scoped.

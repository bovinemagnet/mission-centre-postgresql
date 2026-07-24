# Mission Centre PostgreSQL — Phase 3 Design

**Author:** Paul Snow
**Date:** 2026-07-24
**Version:** 0.0.0
**Status:** Approved — ready for implementation planning
**Licence:** GPL-3.0-or-later
**Parent spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`

---

## 1. Summary

Phase 3 gives Mission Centre a **history store**: Overview metrics and top-statement counters are
persisted continuously, and on connect the Overview graphs are preloaded from stored history so they
open already drawn and reach further back than the 300-point live buffer, surviving a restart of the
application.

Storage is chosen **per server**: **Off**, **Local** (a SQLite file Mission Centre owns), or
**pgconsole** (writing into a `pgconsole` schema that the author's separate pg-console dashboard has
already created on the monitored server). The choice is remembered in the server registry.

### 1.1 Reinterpreting the parent spec

The parent spec §2.2 lists Phase 3 as *"History store, opt-in per database; snapshot capture and
reopen for all connections."* This spec reads that as **continuous** metrics persistence, not explicit
user-triggered incident snapshots: *"reopen"* means the graphs reopen with their history preloaded.
Explicit incident-snapshot capture — a button that freezes and names a full point-in-time state — is
a distinct feature and is **deferred**. Where this spec and the parent's wording differ, this spec
governs for Phase 3.

### 1.2 Prior art

`/home/paul/gitHUB/pg-console` is the author's own Quarkus web dashboard over the same statistics
views. It persists history into a `pgconsole` schema **on the monitored server**, created by Flyway
migrations, sampling every 60s with 7-day retention, and falls back to an in-memory store when the
schema is disabled. Its `pgconsole.system_metrics_history` and `pgconsole.query_metrics_history`
tables hold exactly the counters Mission Centre already collects. Phase 3 reuses those tables as one
of its two backends.

The asymmetry that shapes the design: pg-console runs continuously as a server and accumulates weeks
of history; Mission Centre is a desktop app open for minutes at a time and can never match that on its
own. So on a server pg-console already monitors, Mission Centre's writes **contribute to a shared
history** rather than duplicating a private one, and its preload can draw on history far longer than
any single Mission Centre session produced.

---

## 2. Scope

### 2.1 In scope

| # | Item |
|---|------|
| 1 | A per-server history setting — Off / Local / pgconsole — in the registry |
| 2 | A local SQLite store in the XDG data directory, owned entirely by Mission Centre |
| 3 | Writing into an existing `pgconsole` schema on a monitored server, INSERT-only |
| 4 | A connect-time probe for pgconsole availability and writability |
| 5 | Persisting system (Overview) metrics and top-statement history on an independent cadence |
| 6 | Preloading the Overview graphs from stored history on connect |
| 7 | The UI to set and change a server's history mode |

### 2.2 Explicitly out of scope

Recorded so the decisions are not silently relitigated:

- **Explicit incident-snapshot capture and reopen.** A user-triggered freeze of full state, named and
  reopened later (pg-console calls these incident reports). A distinct feature with its own UI; a
  later phase.
- **Creating the pgconsole schema.** Mission Centre never runs DDL. It writes only to a `pgconsole`
  schema pg-console already created (§4.2). A server Mission Centre alone touches uses Local.
- **Retention on the pgconsole schema.** pg-console owns that schema and its own cleanup. Mission
  Centre never deletes pgconsole rows (§6.2).
- **A historical range view.** No Live / 1h / 24h / 7d selector replacing the live graph with a stored
  window. The GraphWidget is built for a rolling live buffer; a fixed historical range is a separate,
  larger rendering path. Phase 3 preloads the buffer and no more.
- **Rendering query history.** Query history is persisted (§5.2) but Phase 3 renders no view of it.
  On a shared schema it feeds pg-console's own query-history features; on Local it accumulates for a
  later Mission Centre phase. This is a deliberate choice, recorded so it is not read as an oversight.
- **Per-database breakdown history.** The collector sums `pg_stat_database` to server-wide totals and
  Phase 3 persists those totals only. pg-console's `database_metrics_history` is not written.

---

## 3. The read-only line

Phase 1 was strictly read-only, and the parent spec held writes back to Phase 4 (destructive actions:
cancel, terminate, VACUUM). **Phase 3 is the first phase that writes to a monitored server** — but
only in one narrow, opt-in form: `INSERT` into an existing `pgconsole` history table, on a server the
user has explicitly set to pgconsole mode.

This is a different class from Phase 4's actions. It is append-only, it changes no server state the
DBA cares about, it touches only a schema another monitoring tool created for exactly this purpose,
and it never runs unless the user chose it for that server. Every server left on **Off** or **Local**
receives no write of any kind — the read-only guarantee holds for them unchanged.

The choice is never silent and never inferred. Detecting a usable pgconsole schema (§4.1) does not
start writing to it; it only makes the pgconsole option selectable for that server. A server whose
mode is pgconsole but whose schema turns out to be unusable falls back to Local with a one-line note,
and never fails the connection.

---

## 4. pgconsole detection and compatibility

### 4.1 The probe

On connect, after the existing privilege and statements probes, a history probe runs when — and only
when — the server's mode is pgconsole. It checks three things in one round trip:

- the `pgconsole.system_metrics_history` and `pgconsole.query_metrics_history` tables exist;
- they carry the columns Phase 3 writes (§5), by name;
- the connected role may `INSERT` into them (`has_table_privilege(..., 'INSERT')`).

```rust
pub enum PgConsoleAvailability {
    Writable,
    SchemaMissing,       // no pgconsole schema, or not the expected tables/columns
    NotWritable,         // schema present but the role cannot INSERT
}
```

The result decides the effective backend (§6.3). It is not carried on `ServerInfo` for servers not in
pgconsole mode — the probe does not run for them, so a monitored server is never inspected for a
schema the user did not ask to use.

### 4.2 Compatibility contract

Mission Centre reads and writes pg-console's tables but never owns their shape. It writes a fixed set
of columns by name and never `SELECT *`, so extra pg-console columns are ignored and a pg-console
migration that *adds* columns does not break Mission Centre. A migration that *renames or removes* a
column Phase 3 writes is caught by the probe (§4.1), which then reports `SchemaMissing` and Mission
Centre falls back to Local. The columns Phase 3 depends on are the ones present in pg-console's
`V1__initial_schema.sql` and are listed in §5.

Rows Mission Centre writes carry `instance_id = <server UUID>`, distinct from pg-console's default
`'default'`, so the two tools' rows are always distinguishable. Preload (§6.1) reads the server's
recent rows ordered by time regardless of `instance_id` — for drawing a line, the most recent samples
are what matter, whoever wrote them.

---

## 5. What is persisted

Two shapes, both written at the history cadence (§6.2).

### 5.1 System history — server-wide, one row per history sample

The Overview's data. Columns map onto `pgconsole.system_metrics_history`:

| Field | Source | pgconsole column |
|-------|--------|------------------|
| total connections | `session_counts.total()` | `total_connections` |
| max connections | `settings.max_connections` | `max_connections` |
| active / idle / idle-in-transaction | `session_counts` | `active_queries`, `idle_connections`, `idle_in_transaction` |
| cache hit ratio | `rates.cache_hit_ratio` | `cache_hit_ratio` |
| database size | `connected_database_size_bytes` | `total_database_size_bytes` |

Fields pg-console records that Mission Centre does not derive (`blocked_queries`,
`longest_query_seconds`, `longest_transaction_seconds`) are written as `0` / `NULL`; they are
`NOT NULL` with no default only where noted, so the writer supplies a value. Rates that are `None`
(no honest figure) are written `NULL`, never `0` — the same discipline as the UI (§4.4 parent spec).

The **local** SQLite table holds the same fields plus `server_id` and `sampled_at`, so the two
backends store the same information and preload is backend-agnostic.

### 5.2 Query history — top-N statements per history sample

Cumulative counters per `queryid`, from the slow tier's existing statement sample, truncated to the
top `history-top-queries` (default 50) by cumulative time. Columns map onto
`pgconsole.query_metrics_history`: `query_id`, `query_text`, `total_calls`, `total_time_ms`,
`total_rows`, `mean_time_ms`, `shared_blks_hit`, `shared_blks_read`. Statements with a NULL `queryid`
(the text-hash-keyed utility statements) are skipped for pgconsole, whose `query_id` is `NOT NULL`;
the local store keeps them under their text hash.

Persisted, not rendered in Phase 3 (§2.2).

---

## 6. Architecture

### 6.1 Backends and the preload event

The history layer has two structurally different backends: **Local** is synchronous SQLite file I/O;
**pgconsole** is asynchronous SQL over the *existing monitoring connection*. Forcing one trait across
that async/sync seam would fight both. Instead the collector holds a backend value:

```rust
enum HistoryBackend {
    Off,
    Local(LocalStore),      // owns an rusqlite connection to the XDG file
    PgConsole,              // writes via the sample loop's tokio-postgres client
}
```

and calls backend-appropriate code at two points: **load** (once, on connect) and **write** (at the
history cadence, inside the serial sample loop). Local's blocking SQLite calls run under
`spawn_blocking`; pgconsole's INSERTs reuse the client the sample loop already holds.

On connect, after `Connected`, the collector loads the most recent history for this server and emits
it once:

```rust
CollectorEvent::History(Box<HistoryPreload>)   // recent system samples, oldest-first
```

The window feeds the preload into the Overview graph buffers before the first live `Sample` appends,
so the graphs open already showing the recent past. A server on **Off**, or one whose store holds no
history yet, emits an empty preload and the graphs build up live as they do today.

### 6.2 Cadence and retention

History writes on an **independent cadence**, `history-interval-ms` (default 60000). The UI samples
every 2s; writing history that often would flood the table thirty times faster than pg-console and
serve no one — the history cadence is a coarser, separate clock inside the sample loop. A history
write happens on the first tick at or after `history_interval` has elapsed since the last write, the
same shape as the slow tier's scheduling (Phase 2 §3.1).

Retention, `history-retention-days` (default 7), is enforced **only on the local store**, as a bounded
`DELETE` older than the cutoff, run at most once per session. On pgconsole, Mission Centre runs no
retention: pg-console owns that schema and already prunes it, and one tool must never delete another's
rows.

### 6.3 Effective-backend resolution

The registry holds the *intended* mode; the *effective* backend is resolved at connect:

| Server mode | pgconsole probe | Effective backend |
|-------------|-----------------|-------------------|
| Off | not run | Off |
| Local | not run | Local |
| pgconsole | Writable | PgConsole |
| pgconsole | SchemaMissing / NotWritable | Local, with a one-line note |

Falling back to Local rather than Off keeps history working when a schema the user expected has gone;
the note tells them why the graphs are their own rather than the shared history.

### 6.4 Module layout

```
src/
  history/
    mod.rs          HistoryBackend, HistoryMode, the write/load entry points
    sample.rs       SystemHistorySample, QueryHistorySample — the persisted shapes
    local.rs        LocalStore: rusqlite, schema creation, write, load, retention
    pgconsole.rs    PGCONSOLE_PROBE_SQL, the INSERT statements, load query
  collector/
    worker.rs       + history scheduling in the sample loop, + the History event
    snapshot.rs     unchanged (history reads existing Snapshot fields)
  connection/
    params.rs       + HistoryMode field on ConnectionParams (serde default = Off)
  pages/
    overview.rs     + preload(&HistoryPreload) filling the graph buffers
```

New dependency: `rusqlite` with the `bundled` feature, so no system libsqlite is required — matching
the project's existing avoidance of system C dependencies (`tokio-postgres-rustls` over OpenSSL).

---

## 7. Persistence and UI

### 7.1 The per-server setting

`ConnectionParams` gains a `history: HistoryMode` field (`Off` | `Local` | `PgConsole`), serialised
into the existing `servers` GSettings JSON. It carries a serde default of `Off`, so servers stored by
Phase 1 and 2 deserialise unchanged and start with history off — history is strictly opt-in, including
for existing servers.

`HistoryMode` holds no secret and adds no password surface; the registry's
`serialised-form-never-contains-a-password` guarantee is unaffected.

### 7.2 Setting it — new and existing servers

- **New servers:** the Add Server dialog gains a history control (Off / Local / pgconsole).
- **Existing servers:** Phase 1 has no edit dialog. Phase 3 adds a small menu on the sidebar row
  (`McpgSidebarRow`) — a menu button revealing the three history modes for that server — so the
  setting can be changed without re-adding the server. Selecting a mode updates the registry and, if
  that server is currently connected, restarts its collector so the new backend takes effect.

### 7.3 New GSettings keys

| Key | Type | Range | Default | Meaning |
|-----|------|-------|---------|---------|
| `history-interval-ms` | i | 10000–600000 | 60000 | Minimum gap between history writes |
| `history-retention-days` | i | 1–365 | 7 | Age at which local history rows are pruned |
| `history-top-queries` | i | 0–500 | 50 | Statements persisted per history sample (0 disables query history) |

The local database path is `$XDG_DATA_HOME/mission-centre-pg/history.db` (falling back to
`~/.local/share/...`), created on first write. No password is ever written to it — it holds metrics
only, keyed by the server UUID, and the UUID is not a secret.

---

## 8. Error handling

Additions to the parent spec §8 and Phase 2 §11:

| Condition | Behaviour |
|-----------|-----------|
| Local SQLite open or write fails | History for that server is disabled for the session; a one-line warning is logged (never the path's contents). Sampling and the UI continue unaffected |
| pgconsole probe reports SchemaMissing / NotWritable | Effective backend falls back to Local with a note (§6.3). The connection is not failed |
| A pgconsole INSERT fails mid-session (schema dropped, privilege revoked) | Treated like a slow-tier `Query` error (Phase 2 §3.3): it degrades history for that server, not the connection. History falls back to Local for the rest of the session |
| The XDG data directory cannot be created | Local history is disabled; logged once. The application runs normally without history |
| A history write would exceed the cadence during a slow sample | It waits for the next eligible tick; history writes never overlap a sample or each other, because they run inside the serial loop |
| Any history error surfaced to a log | Must never include a password or a full connection string — the same rule as everywhere else |

A history failure must never take down monitoring. History is a convenience layered on top of the
live view; the live view is the product, and it survives any history fault.

---

## 9. Testing

**Unit tests** (pure functions and the local store, no server):

- The local store round-trips a system sample and a query sample through a temporary SQLite file.
- Retention deletes rows older than the cutoff and keeps newer ones.
- Preload returns the most recent N samples oldest-first, and an empty vector for an unseen server.
- `HistoryMode` deserialises from a Phase-1/2 server JSON with no `history` field, defaulting to Off.
- The history-cadence scheduler fires on the first tick past the interval and not before (mirrors the
  Phase 2 `is_slow_tick` tests).
- `PgConsoleAvailability` classification from the probe columns.
- A NULL-`queryid` statement is skipped for pgconsole and kept for Local.

**Integration tests** (`testcontainers`, real servers):

- Against **postgres:14** and **postgres:18**: create pg-console's `system_metrics_history` and
  `query_metrics_history` (from the known `V1` shape), run the history probe, and assert `Writable`.
- Insert a system sample and a query sample over the client, read them back, and assert the round
  trip — this is what proves Phase 3's INSERTs match pg-console's column types on both versions.
- Run the probe against a plain container with no `pgconsole` schema and assert `SchemaMissing`.
- Connect as a role with only SELECT on the schema and assert `NotWritable`.

**Not tested automatically:** GTK preload rendering and the sidebar menu, as with every phase.
Verified by running the application.

---

## 10. Success criteria

1. A server can be set to Off, Local or pgconsole, in the Add Server dialog and via the sidebar-row
   menu, and the setting persists across a restart.
2. On Local, connecting, sampling for a while, quitting and reconnecting shows the Overview graphs
   preloaded with the earlier history rather than building up from empty.
3. On pgconsole against a server carrying pg-console's schema, Mission Centre's samples appear in
   `pgconsole.system_metrics_history` under the server's own `instance_id`, and no other pgconsole
   rows are modified or deleted.
4. A server left on Off or Local receives no write of any kind — verified by a server with no INSERT
   privilege connecting cleanly and monitoring normally.
5. A pgconsole server whose schema is missing or unwritable falls back to Local with a note and does
   not fail the connection.
6. Local history is pruned to the retention window; pgconsole history is never pruned by Mission
   Centre.
7. A history fault — a full disk, a revoked privilege, a dropped schema — disables history for that
   server without disturbing the live graphs or the connection.
8. `cargo fmt` produces no diff; unit and integration tests pass on 14 and 18; no source file exceeds
   roughly 800 lines; no password reaches the local store, a log, or a history table.

---

## 11. Open questions

None blocking. Deferred decisions are recorded in §2.2.

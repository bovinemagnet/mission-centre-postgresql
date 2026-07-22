# Mission Centre PostgreSQL — Design

**Author:** Paul Snow
**Date:** 2026-07-22
**Version:** 0.0.0
**Status:** Approved — Phase 1 ready for implementation planning
**Licence:** GPL-3.0-or-later

---

## 1. Summary

Mission Centre PostgreSQL is a GTK4/libadwaita desktop monitor for PostgreSQL servers, in the
visual and interaction style of [Mission Center](https://gitlab.com/mission-center-devs/mission-center).
Where Mission Center shows what a machine is doing, this shows what a database is doing: live
connection load, transaction throughput, cache behaviour, and the sessions currently running.

It targets **ad-hoc connections to arbitrary servers** — you point it at whatever needs looking at,
rather than maintaining a curated fleet.

### Relationship to Mission Center

This is a **new project**, not a fork. It is derived from Mission Center only in that it vendors three
UI widget source files (see §9) and follows its shell layout and background-polling pattern. It is
therefore GPL-3.0-or-later, as Mission Center is.

Mission Center's defining architecture — a separate `magpie` gatherer process speaking nng+protobuf
to the UI — is deliberately **not** carried over. That split exists because system metrics require a
separate, sometimes privileged, process reading `/proc`, IOKit and SMC. Here the gatherer is
PostgreSQL itself, reached over libpq. An IPC layer would add a protobuf schema, a process spawner
and a socket lifecycle for no benefit.

---

## 2. Scope

### 2.1 Subsystems

| # | Subsystem | Responsibility |
|---|-----------|----------------|
| 1 | Connection layer | Server registry, credentials, connect/disconnect, version and privilege detection |
| 2 | Collector | Periodic sampling of catalog and `pg_stat_*` views into immutable snapshots |
| 3 | History | Opt-in per-database persistence; snapshot capture and reopen |
| 4 | Shell | Window, sidebar, page stack, preferences |
| 5 | Pages | Overview, Sessions, Queries, Tables & Indexes, Replication, Locks |
| 6 | Actions | Cancel/terminate, VACUUM/ANALYZE, stat resets, config reload |

### 2.2 Phasing

The full dashboard is too large for a single implementation plan. Each phase gets its own plan.

- **Phase 1 (this spec)** — Subsystems 1, 2 and 4, plus the **Overview** and **Sessions** pages.
  Strictly read-only. In-memory only. This is the vertical slice that proves the whole stack:
  connect to an arbitrary server, sample it, render it, and look right doing it.
- **Phase 2** — Queries page (`pg_stat_statements`); Tables & Indexes page (bloat, sequential scans,
  index usage).
- **Phase 3** — History store, opt-in per database; snapshot capture and reopen for all connections.
- **Phase 4** — Actions, with the privilege model: cancel/terminate backend, VACUUM/ANALYZE,
  `pg_stat_statements_reset()`, `pg_reload_conf()`.
- **Phase 5** — Replication and Locks pages.

### 2.3 Explicitly out of scope for Phase 1

Both of these are real subsystems deferred on YAGNI grounds, recorded here so the decision is not
silently relitigated:

- **SSH tunnels.** Tunnel management means key handling, port allocation, child-process lifecycle and
  a new class of failure modes. Phase 1 assumes the user runs `ssh -L` themselves and points the app
  at `localhost:<port>`. Revisit once the application is worth tunnelling to.
- **Multi-server concurrent polling.** The sidebar is designed for N servers, but Phase 1 samples only
  the *selected* server. Polling every configured server in the background is a distinct performance
  and connection-budget problem.

---

## 3. Architecture

Single process, two threads.

```
main thread (GTK)                    collector thread (tokio runtime)
─────────────────                    ───────────────────────────────
Application                          ConnectionManager
  └─ Window                            └─ tokio-postgres client
      ├─ Sidebar (servers)                  │
      └─ Stack                              │ every N seconds:
          ├─ OverviewPage  ◄────────┐       │   pg_stat_database,
          └─ SessionsPage  ◄────────┤       │   pg_stat_activity,
                                    │       │   pg_settings
                     async_channel ─┴── CollectorEvent
```

The collector owns a tokio runtime on a dedicated thread and sends immutable events down an
`async_channel`. The GTK side consumes them with `glib::spawn_future_local` on the main loop. This
mirrors Mission Center's poll-thread-and-marshal pattern with the socket removed, so its rule holds
unchanged: **never touch GTK widgets off the main thread**.

### 3.1 Technology choices

| Choice | Rationale |
|--------|-----------|
| Rust + gtk4-rs + libadwaita | Matches Mission Center; lets the vendored widgets compile unmodified |
| Blueprint (`.blp`) for UI | Same as Mission Center; compiled to `.ui` and bundled as GResource |
| Meson driving Cargo | Compiles Blueprint, bundles GResource, installs the GSettings schema. Cargo alone cannot do these. The toolchain is already proven on macOS |
| `tokio-postgres` | Queries are selected at runtime by server version, so `sqlx`'s compile-time verification cannot apply — it would add macro and offline-metadata machinery for no benefit |
| `tokio-postgres-rustls` | TLS without an OpenSSL system dependency |
| `keyring` | Credentials to Secret Service (Linux) / Keychain (macOS), never to a config file |

### 3.2 Module layout

```
src/
  main.rs              entry point, gettext setup, GResource loading
  application.rs       AdwApplication subclass, owns GSettings
  window.rs            main window, sidebar/stack wiring, privilege banner
  i18n.rs              gettext wrappers (i18n, i18n_f, ni18n_f)
  config.rs.in         meson-templated build config

  connection/
    mod.rs             ConnectionManager: connect, disconnect, reconnect/backoff
    params.rs          ConnectionParams (host, port, database, user, sslmode)
    credentials.rs     keyring integration
    probe.rs           server_version_num and privilege detection at connect

  collector/
    mod.rs             sampling loop, rate derivation, tokio runtime ownership
    snapshot.rs        CollectorEvent, Snapshot, DatabaseStats, Session, ServerSettings
    queries/
      database.rs      pg_stat_database SQL, version-gated
      activity.rs      pg_stat_activity SQL, version-gated
      settings.rs      pg_settings SQL

  pages/
    overview.rs        graphs: connections, TPS, cache hit, tuple throughput
    sessions.rs        pg_stat_activity table

  widgets/
    graph_widget.rs        vendored from Mission Center (verbatim)
    graph_widget_utils.rs  vendored from Mission Center (two edits, see §9)
    sidebar_row.rs         ours: sparkline row composing GraphWidget

  table/
    mod.rs             ColumnView-based table, modelled on Mission Center's but our own types
    columns/           one file per column type
```

No source file should grow past roughly 800 lines. Mission Center's
`src/performance_page/mod.rs` reached 2,990 lines by accumulating per-device-type plumbing in one
place; the sidebar/stack coordination here must not repeat that.

---

## 4. Data model

### 4.1 Channel payload

Connection state is as much a thing the UI renders as the metrics are, so the channel carries an
event, not a bare snapshot.

```rust
enum CollectorEvent {
    Connecting,
    Connected(ServerInfo),      // version, server name, uptime, privilege level
    Sample(Box<Snapshot>),
    Error(CollectorError),      // auth failed, timeout, connection lost, query failed
    Disconnected,
}

struct Snapshot {
    taken_at: Instant,
    databases: Vec<DatabaseStats>,   // raw counters and derived rates
    sessions: Vec<Session>,
    settings: ServerSettings,        // max_connections etc., sampled rarely
}
```

### 4.2 State ownership

- The **collector** is stateless apart from the previous sample's counters, which it needs to derive
  rates.
- The **UI** owns the bounded ring buffers that feed the graph widget. Buffer length is derived from
  the configured graph window (§7.1), defaulting to 300 points.

Sampling is **serial**: the next sample begins only once the previous one has completed or timed out.
The configured interval is therefore a minimum gap, not a guaranteed cadence — with a 2s interval and
a 5s `statement_timeout`, a slow server yields samples further apart rather than overlapping queries
piling up on one connection. Snapshots carry `taken_at`, so rate derivation uses the true elapsed
time rather than the nominal interval.

Keeping the buffers in the UI means reconnecting does not wipe the graphs, and keeps the collector
cheap to reason about.

### 4.3 Phase 1 metrics

**Overview page.** `pg_stat_database` returns one row per database. The Overview page shows
**server-wide totals**, summed across all rows, because "how loaded is this server" is the question it
answers. `Snapshot.databases` retains the per-database breakdown for later phases; Phase 1 simply
does not render it. Database size is the exception — it is reported for the connected database only,
since a server-wide total is not a meaningful number.

| Metric | Source | Derivation |
|--------|--------|------------|
| Connections by state | `pg_stat_activity` | Count grouped by `state`, against `max_connections` |
| Transactions/sec | `pg_stat_database` | Δ(`xact_commit` + `xact_rollback`) / Δt |
| Cache hit ratio | `pg_stat_database` | Δ`blks_hit` / (Δ`blks_hit` + Δ`blks_read`) |
| Tuple throughput | `pg_stat_database` | Δ of `tup_returned`, `tup_fetched`, `tup_inserted`, `tup_updated`, `tup_deleted` |
| Deadlocks | `pg_stat_database` | Δ`deadlocks` |
| Temp bytes | `pg_stat_database` | Δ`temp_bytes` |
| Database size | `pg_database_size()` | Absolute |

**Sessions page columns**

`pid`, `usename`, `application_name`, `client_addr`, `datname`, `state`, `wait_event_type`,
`wait_event`, `backend_type`, duration (derived from `query_start`), `query`.

### 4.4 Rates must be deltas, not cumulative

`pg_stat_database` counters are cumulative since the last statistics reset. Computing cache hit ratio
from the raw totals — as most naive dashboards do — yields a number pinned near 99% on any
long-running server, which conveys nothing. Every rate in §4.3 is computed per sampling interval.

Consequences to handle:

- The **first sample after connecting has no rates**. The UI shows an empty graph, not zeroes.
- **Counter resets go backwards.** If any counter is lower than the previous sample (someone called
  `pg_stat_reset()`, or the server restarted), the interval is discarded rather than emitting a
  negative or absurd rate.

---

## 5. Version support

**Floor: PostgreSQL 14.** Connection is permitted at 14 and above.

| Version | Monitoring-relevant change |
|---------|----------------------------|
| 14 | `query_id` in `pg_stat_activity` via `compute_query_id` |
| 15 | Cumulative statistics moved to shared memory; stats collector process removed |
| 16 | `pg_stat_io` — I/O by backend type, context and object |
| 17 | `pg_stat_checkpointer`; `pg_stat_bgwriter` split |
| 18 | `pg_stat_io` extended to WAL; per-backend I/O statistics |

14 is the honest floor: PG 13 is past end-of-life, and the views Phase 1 uses
(`pg_stat_database`, `pg_stat_activity`) are stable across 14 through 18 for every column consumed.
Version gating is required regardless — 16, 17 and 18 each add views later phases will want — so once
`sql_for(version)` exists, supporting 14 costs almost nothing.

**Gate at the page level, never at the connection level.** A future I/O page renders
"Requires PostgreSQL 16 or later — this server is 14.11" in place of its content. A connection
refusal would be the wrong failure for a tool whose purpose is meeting whatever server is in front of
it, and page-level gating stops the floor creeping every time a page is added.

`probe.rs` reads `server_version_num` once at connect and stores it on `ServerInfo`.

**Phase 1 needs no per-version SQL.** Every column Phase 1 consumes from `pg_stat_database` and
`pg_stat_activity` is present and unchanged across 14 through 18, so each query module holds a single
`&'static str`. Building a `sql_for(version)` selector now would be machinery that always returns the
same string. It arrives in Phase 2 or later, at the first page that genuinely branches — and the
integration tests against 14 and 18 (§10) are what prove the single statement really is portable.

---

## 6. Privilege detection

A role lacking `pg_monitor` membership (or superuser) sees `NULL` query text for every backend but its
own, and little useful from the statistics views. Rendering a screen of blanks would leave the user
unable to tell a quiet server from a broken application.

At connect, `probe.rs` evaluates:

```sql
SELECT current_setting('server_version_num')::int AS version,
       pg_has_role(current_user, 'pg_monitor', 'member') AS is_monitor,
       (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) AS is_superuser;
```

If neither holds, the window shows a persistent banner:

> Connected without `pg_monitor` — query text and statistics for other users' sessions are hidden.

The banner is **window-level, not page-level**: it is a property of the connection and must persist
across page switches. The same probe result gates the Phase 4 action buttons.

---

## 7. User interface

```
┌─────────────────────┬──────────────────────────────────────┐
│ SERVERS             │  ⚠ Connected without pg_monitor …    │
│  ● prod-aplus       ├──────────────────────────────────────┤
│    ├ Overview   ▁▃▅ │   Connections        47 / 200        │
│    └ Sessions       │   ┌────────────────────────────────┐ │
│  ○ staging          │   │      ▁▂▃▅▆▅▃▂▁▂▄▆█▆▄▂          │ │
│  ○ localhost        │   └────────────────────────────────┘ │
│                     │   Transactions/sec   1,284           │
│  + Add server       │   ┌────────────────────────────────┐ │
└─────────────────────┴───┴────────────────────────────────┴─┘
```

`AdwApplicationWindow` → `AdwNavigationSplitView`, sidebar left, page stack right.

- **Sidebar rows carry live sparklines** via `SidebarRow`, our own thin wrapper composing the
  vendored `GraphWidget` (§9) — the visual signature the project is after.
- **Connection state shows per server**: ● connected, ○ disconnected, ⟳ connecting, ⚠ error. Because
  Phase 1 polls only the selected server, the others honestly show as disconnected rather than
  implying live data.
- **Sessions page** is the `ColumnView` table: sortable columns, a filter entry, and *hide idle
  sessions* enabled by default — idle connections are the majority of rows on any real server and
  almost never why the application was opened.
- **Add Server** is an `AdwDialog` taking host, port, database, user, password and SSL mode.

### 7.1 Persistence

- **Server list** — GSettings, as a JSON array. Contains no password.
- **Credentials** — system secret store via `keyring`, keyed by a stable per-server UUID.
- **Preferences** — GSettings: sampling interval (default 2s), hide-idle-sessions default, graph
  window length.

---

## 8. Error handling

| Condition | Behaviour |
|-----------|-----------|
| Connect fails (auth, host unreachable, TLS) | `CollectorEvent::Error`; sidebar ⚠; page shows the error with a Retry button. Never retried silently in a tight loop |
| Query exceeds `statement_timeout` (5s) | Treated as a failed *sample*, not a disconnect. The graph gaps rather than resetting |
| 3 consecutive failed samples | Transition to `Disconnected`; begin reconnect with exponential backoff (1s, 2s, 4s … capped at 30s) |
| Counter goes backwards | Interval discarded; no rate emitted (see §4.4) |
| Unexpected `NULL` in an optional column | Modelled as `Option<T>`. Parsing must never panic on server output |
| Any error surfaced to the user or a log | Must never include the password, nor the full connection string |

The `statement_timeout` on the monitoring connection is not optional: without it a wedged server
hangs the sampler indefinitely, and the application looks frozen for a reason that is entirely the
server's fault.

---

## 9. Vendored code and attribution

Two files are copied from Mission Center, taken from commit `050213c`:

| File | Lines | State |
|------|-------|-------|
| `src/performance_page/widgets/graph_widget.rs` | 777 | Verbatim. Verified free of any `magpie` or crate-local coupling; pure GTK/GSK |
| `src/performance_page/widgets/graph_widget_utils.rs` | 722 | Two edits only: define `MAX_POINTS = 600` and `MIN_POINTS = 10` locally (they come from `crate::preferences` upstream), and adjust the `GraphWidget` import path |

Each retains its original copyright header, with an added note recording the upstream project, the
source path and the commit it was taken from. `README.md` credits Mission Center. The project licence
is GPL-3.0-or-later, as required.

**`summary_graph.rs` is *not* vendored.** Inspection showed it is not the clean lift it appears to
be: it depends on `magpie_types::network::ConnectionKind` (a `DeviceType::from_connection_kind`
constructor), on `crate::settings`, and on `SidebarDropHint` — a further 143 lines implementing
drag-and-drop sidebar reordering that Phase 1 does not want. Stripping those out of 413 lines leaves
little worth inheriting. We instead write our own `SidebarRow` (§7) that composes the vendored
`GraphWidget` directly, which is both smaller and free of foreign coupling.

Mission Center's `src/table_view/` is likewise **not** vendored: it is coupled to `magpie` types
through `models.rs` and `mod.rs`. It serves as a reference for the column architecture only.

---

## 10. Testing

Mission Center itself has no test suite. This project does, for the parts where tests are cheap and
the logic is genuinely fallible.

**Unit tests** (pure functions, no database):

- Rate derivation from counter pairs — including the counter-reset case, the first-sample case, and
  division by zero when `Δblks_hit + Δblks_read == 0`.
- `sql_for(version)` selection across the 14–18 range.
- Duration derivation from `query_start` against `taken_at`.
- Privilege probe result mapping to banner state.

**Integration tests** (`testcontainers`, real servers):

- The generated SQL executes successfully against **postgres:14** and **postgres:18** images. This is
  the test that matters most — it is the one that catches a column that does not exist on the floor
  version, which no amount of unit testing will find.
- Connect, sample, and parse a snapshot end to end.
- Connect as a role without `pg_monitor` and assert the privilege probe reports it.

**Not tested automatically:** GTK widget rendering and interaction. Verified by running the
application, as with Mission Center.

Development follows TDD where practical: the rate-derivation logic and query generation are written
test-first.

---

## 11. Phase 1 success criteria

1. `meson setup build && ninja -C build` produces a running binary.
2. The application connects to an arbitrary PostgreSQL 14–18 server given host, port, database, user,
   password and SSL mode.
3. Credentials persist to the system secret store; the server list persists to GSettings; neither a
   config file nor a log ever contains a password.
4. The Overview page renders live graphs for connections, TPS, cache hit ratio and tuple throughput,
   updating at the configured interval.
5. The Sessions page lists `pg_stat_activity` in a sortable, filterable table, hiding idle sessions by
   default.
6. Connecting without `pg_monitor` shows the privilege banner and does not render a screen of blanks.
7. Killing the server mid-session moves the UI to a visible error state and reconnects with backoff
   once the server returns — without crashing, and without losing the graph history.
8. `cargo fmt` produces no diff; unit and integration tests pass.

---

## 12. Open questions

None blocking Phase 1. Deferred decisions are recorded in §2.3.

# Mission Centre PostgreSQL — Phase 2 Design

**Author:** Paul Snow
**Date:** 2026-07-24
**Version:** 0.0.0
**Status:** Approved — ready for implementation planning
**Licence:** GPL-3.0-or-later
**Parent spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`

---

## 1. Summary

Phase 2 adds the two pages named in the parent spec §2.2: a **Queries** page fed by
`pg_stat_statements`, and a **Tables & Indexes** page fed by `pg_stat_user_tables` and
`pg_stat_user_indexes`.

Both are read-only and in-memory. History (Phase 3) and actions such as VACUUM, ANALYZE and
`pg_stat_statements_reset()` (Phase 4) remain out of scope.

Phase 1 proved the vertical slice: connect, sample, render. Phase 2 is where the sampling loop stops
being uniform. The two new data sources are an order of magnitude more expensive than
`pg_stat_activity`, one of them depends on an extension that may not be installed, and one of them
can fail for reasons that must not be mistaken for a lost connection. Those three facts drive most of
what follows.

### 1.1 Prior art

`/home/paul/gitHUB/pg-console` is the author's own Quarkus web dashboard over the same statistics
views. Three of its decisions are adopted here, and one is deliberately not:

- **Adopted:** probe `pg_extension.extversion` to decide whether `pg_stat_statements` is usable,
  rather than issuing the query and interpreting the failure.
- **Adopted:** exclude primary-key and unique-constraint indexes when reporting unused indexes.
  Without that exclusion every primary key on the server is reported as unused, which is noise that
  destroys the report's credibility.
- **Adopted:** estimate table bloat from the dead-tuple ratio rather than from column statistics.
  pg-console reached the same conclusion independently, and documents the estimate's limits.
- **Not adopted:** pg-console selects `'unknown' as user, 'current' as database` rather than
  resolving `pg_stat_statements.userid` and `dbid`. Phase 2 joins `pg_roles` and `pg_database` and
  shows the real names.

---

## 2. Scope

### 2.1 In scope

| # | Item |
|---|------|
| 1 | A second, slower sampling cadence in the collector, carrying both new data sources |
| 2 | `pg_stat_statements` availability probing, gated at the page |
| 3 | Queries page: top statements, cumulative and per-interval, sortable and filterable |
| 4 | Tables & Indexes page: two tables behind an inner switcher |
| 5 | `src/table/`, the shared `ColumnView` machinery the parent spec §3.2 planned; Sessions migrates onto it |

### 2.2 Explicitly out of scope

Recorded so the decisions are not silently relitigated:

- **Query detail and EXPLAIN.** Running `EXPLAIN` against a normalised statement means reconstructing
  parameter values that `pg_stat_statements` has deliberately discarded. A detail view over the
  columns already sampled may arrive later; plan generation is a separate feature with its own
  privilege and safety questions.
- **Index and vacuum recommendations.** Deciding that an index is redundant or a table needs
  `VACUUM FULL` is advice, not measurement. Phase 2 shows the numbers that advice would be derived
  from. Advice belongs with the actions that follow from it, in Phase 4.
- **Statement resets and baselines.** `pg_stat_statements_reset()` is a write, and belongs to Phase 4.
  Period-over-period comparison needs the history store from Phase 3.
- **Per-database drill-down.** `pg_stat_user_tables` reports the connected database only. Sampling
  every database means a connection per database, which is the multi-server polling problem in
  another costume (parent spec §2.3).
- **Delta counters on Tables & Indexes.** See §6.4.

---

## 3. Collector: the slow tier

### 3.1 Cadence

The sample loop gains a second cadence controlled by a new GSettings key,
`slow-sample-interval-ms` (default 10000). A tick is *also* a slow tick when no slow sample has been
taken yet for this connection, or when `slow_interval` has elapsed since the last one.

The first tick after connecting is always a slow tick, so both pages populate immediately rather than
showing nothing for the first ten seconds.

Sampling stays **serial**. The heavy statements run inside the same sample as the light ones, on the
same connection, under the same `statement_timeout`. Nothing overlaps, and the slow tier cannot cause
the connection to accumulate concurrent work.

### 3.2 Snapshot additions

```rust
pub struct Snapshot {
    // … Phase 1 fields unchanged …

    /// `None` on a fast tick — the page keeps its previous contents.
    /// `Err` carries the reason the page renders inline.
    pub statements: Option<Result<StatementsSample, CollectorError>>,
    pub relations:  Option<Result<RelationsSample, CollectorError>>,
}
```

`None` and `Err` mean different things and must not be collapsed: `None` is "not sampled this tick",
`Err` is "sampled and failed". A page seeing `None` leaves its table alone; a page seeing `Err`
replaces its content with the reason.

The two payloads are plain row collections; the derivation that needs cross-sample state has already
happened in the collector by the time they reach the UI:

```rust
pub struct StatementsSample { pub statements: Vec<Statement> }
pub struct RelationsSample  { pub tables: Vec<TableStats>, pub indexes: Vec<IndexStats> }
```

The collector holds `previous_statements: Option<(HashMap<StatementKey, StatementCounters>, Instant)>`
across slow samples, which is the only state the slow tier adds — the same shape as Phase 1's
`previous` counters for `pg_stat_database`.

### 3.3 Failure isolation

This is the part that must not be got wrong. Phase 1's rule is that three consecutive failed samples
declare the connection lost (parent spec §8). A `permission denied for view pg_stat_statements` would,
under that rule, disconnect a perfectly healthy server every thirty seconds.

So slow-tier errors are classified, not uniformly propagated:

| Error from a slow-tier query | Treatment |
|------------------------------|-----------|
| `CollectorError::LostConnection` | Propagates. The sample fails, exactly as in Phase 1 |
| `CollectorError::Timeout` | Propagates. A slow tier that cannot finish inside `statement_timeout` is a real problem with the server |
| `CollectorError::Query(_)` | Captured into the snapshot as `Err`. The sample still succeeds |

`Query` covers the cases that are properties of the schema or the role rather than of the connection:
insufficient privilege, an extension dropped while connected, and a relation dropped between the
catalogue read and `pg_total_relation_size` evaluating on it. One failing view degrades one page.

### 3.4 Cost control

Every slow-tier statement is bounded by `ORDER BY … LIMIT`, with the limit read from GSettings
(§8). Query text is truncated server-side to 2000 characters, so a pathological generated statement
cannot dominate the payload.

---

## 4. Availability probing

`PROBE_SQL` gains one scalar:

```sql
(SELECT extversion FROM pg_extension WHERE extname = 'pg_stat_statements') AS statements_version
```

which is `NULL` when the extension is not installed in the connected database.

```rust
pub enum StatementsAvailability {
    Available { version: String },
    TooOld { version: String },
    NotInstalled,
}
```

`ServerInfo` carries the result, and `classify(extversion: Option<&str>)` maps to it.

**The floor is extension version 1.8.** `total_exec_time` and `mean_exec_time` were introduced in
1.8, shipped with PostgreSQL 13; before that the columns are `total_time` and `mean_time`. A server
at or above the project's PostgreSQL 14 floor can still be running extension 1.7 if it was carried
through `pg_upgrade` without `ALTER EXTENSION pg_stat_statements UPDATE`, and in that case the query
fails on a missing column, not a missing view. This is Phase 2's only genuine version branch, and it
is on the **extension** version, not the server version.

Version comparison is numeric per component, never lexical: released versions include 1.8, 1.9, 1.10
and 1.11, and as text `"1.10" < "1.8"`. This gets its own unit test.

Gating is at the page, never at the connection (parent spec §5). Each state renders an
`AdwStatusPage` in place of the table:

| State | Message |
|-------|---------|
| `NotInstalled` | *pg_stat_statements is not installed in the database `<name>`. Add `pg_stat_statements` to `shared_preload_libraries`, restart the server, then run `CREATE EXTENSION pg_stat_statements`.* |
| `TooOld { version }` | *pg_stat_statements `<version>` is installed; version 1.8 or later is required. Run `ALTER EXTENSION pg_stat_statements UPDATE`.* |

Neither state raises the error banner: an absent extension is a fact about the server, not a fault in
the connection.

---

## 5. Queries page

### 5.1 Identity and matching

Rows are keyed by `(userid, dbid, queryid)`. `queryid` is nullable — utility statements have none
when `track_utility` is on — so a row with a NULL `queryid` falls back to a hash of its query text.
Without a stable key, no row can be matched to its previous sample and no delta can be derived.

```rust
pub enum StatementId { QueryId(i64), TextHash(u64) }

pub struct StatementKey { pub user_oid: i64, pub db_oid: i64, pub id: StatementId }
```

### 5.2 Cumulative and per-interval

Each row carries the cumulative counters and an `Option<StatementDelta>`:

```rust
pub struct Statement {
    pub key: StatementKey,
    pub query: String,
    /// `None` when the recorded role or database has since been dropped.
    pub user_name: Option<String>,
    pub database: Option<String>,
    pub cumulative: StatementCounters,
    pub delta: Option<StatementDelta>,
}

pub struct StatementCounters {
    pub calls: i64,
    pub total_exec_time_ms: f64,
    pub rows: i64,
    pub shared_blks_hit: i64,
    pub shared_blks_read: i64,
    pub shared_blks_dirtied: i64,
    pub shared_blks_written: i64,
    pub temp_blks_read: i64,
    pub temp_blks_written: i64,
    pub wal_bytes: f64,
}

pub struct StatementDelta {
    pub calls_per_sec: f64,
    /// Milliseconds of execution time accrued per second — a value of 1000
    /// means this statement kept one core busy for the whole interval.
    pub exec_time_ms_per_sec: f64,
    /// Mean over the interval, not over the statement's lifetime.
    /// `None` when the statement was not called during the interval.
    pub mean_exec_time_ms: Option<f64>,
    pub rows_per_sec: f64,
    /// `None` when no shared blocks were touched in the interval.
    pub cache_hit_ratio: Option<f64>,
}
```

The delta is `None` when the key was absent from the previous slow sample, and `None` when any
counter went backwards. Backwards covers two distinct causes with the same remedy: someone called
`pg_stat_statements_reset()`, or the entry was evicted when `pg_stat_statements.max` was reached and
its slot reused. Both make the interval meaningless, so no rate is emitted — the same rule as parent
spec §4.4.

A **Last interval / Since reset** toggle switches what every numeric column means. Last interval is
the default; the table shows the cumulative view until the first delta exists.

### 5.3 Columns

| Column | Since reset | Last interval |
|--------|-------------|---------------|
| Query | normalised text, whitespace collapsed | same |
| Calls | `calls` | calls/sec |
| Total time | `total_exec_time` | ms per second |
| Mean time | `total_exec_time / calls` | interval mean |
| Rows | `rows` | rows/sec |
| Cache hit | `hit / (hit + read)` | interval ratio |
| User | `pg_roles.rolname` | same |
| Database | `pg_database.datname` | same |

Numeric columns sort numerically; a `None` ratio renders as an em dash, as Phase 1 established for
non-finite rates.

### 5.4 SQL

```sql
SELECT s.queryid,
       s.userid::int8            AS user_oid,
       s.dbid::int8              AS db_oid,
       r.rolname::text           AS user_name,
       d.datname::text           AS database,
       left(s.query, 2000)       AS query,
       s.calls,
       s.total_exec_time,
       s.rows,
       s.shared_blks_hit,
       s.shared_blks_read,
       s.shared_blks_dirtied,
       s.shared_blks_written,
       s.temp_blks_read,
       s.temp_blks_written,
       s.wal_bytes::float8       AS wal_bytes
  FROM pg_stat_statements s
  LEFT JOIN pg_roles    r ON r.oid = s.userid
  LEFT JOIN pg_database d ON d.oid = s.dbid
 WHERE s.query NOT LIKE '%pg_stat_statements%'
 ORDER BY s.total_exec_time DESC
 LIMIT $1
```

Three details that are easier to record than to rediscover:

- **`wal_bytes` is `numeric`.** Read without the `::float8` cast it needs a decimal crate for one
  column that is displayed as an approximate size anyway.
- **The `NOT LIKE` filter excludes our own monitoring statement** from the report it produces. It
  also excludes genuine user queries that mention the view by name. That trade is accepted; the
  alternative is the monitor permanently ranking itself.
- **`rolname` and `datname` are `LEFT JOIN`ed** because a role or database dropped after the
  statement was recorded leaves the OID dangling. Both map to `Option<String>`.

### 5.5 Known limitation: the ranking window

`LIMIT` is applied to a ranking by **cumulative** `total_exec_time`. On a server with long uptime, a
statement that has just become hot can sit outside the top 200 and therefore be invisible in the
delta view, which is precisely the view meant to surface it.

Mitigations: the limit is a setting rather than a constant (default 200), and the limitation is
documented rather than hidden. A proper fix — ranking by delta, which requires fetching the full
entry set — is deferred until there is evidence it matters, and would arrive alongside the Phase 3
history store that would make it affordable.

---

## 6. Tables & Indexes page

### 6.1 Layout and scope

One entry in the top-level `ViewSwitcher`, containing an inner `Adw.ViewSwitcher` over two flat
tables. Keeping it to one top-level entry leaves room for the Replication and Locks pages of Phase 5.

`pg_stat_user_tables` and `pg_stat_user_indexes` report the **connected database only**. The page
says so in its subtitle rather than leaving the user to infer it from numbers that look too small.

### 6.2 Tables

```sql
SELECT t.schemaname::text  AS schema_name,
       t.relname::text     AS table_name,
       t.seq_scan,
       t.seq_tup_read,
       COALESCE(t.idx_scan, 0)      AS idx_scan,
       COALESCE(t.idx_tup_fetch, 0) AS idx_tup_fetch,
       t.n_tup_ins,
       t.n_tup_upd,
       t.n_tup_del,
       t.n_live_tup,
       t.n_dead_tup,
       EXTRACT(EPOCH FROM (now() - GREATEST(t.last_vacuum, t.last_autovacuum)))::float8
         AS secs_since_vacuum,
       pg_total_relation_size(t.relid)::int8 AS total_bytes
  FROM pg_stat_user_tables t
 ORDER BY total_bytes DESC
 LIMIT $1
```

`idx_scan` is NULL for a table with no indexes; `COALESCE` to zero is correct, since a table with no
indexes has had no index scans. `GREATEST` over the two vacuum timestamps returns NULL only when the
table has never been vacuumed by either route, which is itself the interesting answer.

Derived, as pure functions with their own tests:

- **Dead-tuple percentage** — `n_dead_tup / (n_live_tup + n_dead_tup)`, and `None` when the table has
  no tuples at all. Reporting 0% for an empty table would be a lie of the same shape as the
  cache-hit ratio that parent spec §4.4 rejects.
- **Sequential-scan ratio** — `seq_scan / (seq_scan + idx_scan)`, and `None` when the table has never
  been scanned by either route.

Columns: Schema, Table, Size, Live, Dead, Dead %, Seq scans, Index scans, Seq %, Inserts, Updates,
Deletes, Last vacuum.

**The Dead % column is named for what it measures.** It is not called Bloat. Dead tuples are the
component of bloat that a statistic can see and the one that actually drives a vacuum decision;
calling the number "bloat" would claim an accuracy that only `pgstattuple` — a full table scan —
can deliver.

### 6.3 Indexes

```sql
SELECT i.schemaname::text   AS schema_name,
       i.relname::text      AS table_name,
       i.indexrelname::text AS index_name,
       COALESCE(i.idx_scan, 0)      AS idx_scan,
       COALESCE(i.idx_tup_read, 0)  AS idx_tup_read,
       COALESCE(i.idx_tup_fetch, 0) AS idx_tup_fetch,
       pg_relation_size(i.indexrelid)::int8 AS bytes,
       x.indisprimary,
       x.indisunique,
       x.indisvalid
  FROM pg_stat_user_indexes i
  JOIN pg_index x ON x.indexrelid = i.indexrelid
 ORDER BY bytes DESC
 LIMIT $1
```

Columns: Schema, Table, Index, Size, Scans, Tuples read, Tuples fetched, Kind.

Kind renders as `primary`, `unique`, `index`, with `invalid` appended when `indisvalid` is false — a
failed `CREATE INDEX CONCURRENTLY` leaves an invalid index that consumes space and answers no
queries, and is worth seeing.

An **Unused only** toggle filters to `idx_scan = 0 AND NOT indisprimary AND NOT indisunique`. The
exclusion is the point: a primary key with zero scans is not a candidate for removal, and including
those rows makes the list useless.

### 6.4 Cumulative only

Both tables show counters cumulative since the last statistics reset, with no delta toggle. They
answer "what shape is this schema in", not "what is happening this second"; a per-interval sequential
scan count read at a ten-second cadence is mostly noise. The `table/` module makes adding a toggle
later cheap if that judgement turns out to be wrong.

The columns are therefore labelled as since-reset totals rather than left ambiguous.

### 6.5 Relations dropped mid-query

`pg_total_relation_size` and `pg_relation_size` raise an error if the relation disappears between the
catalogue row being read and the size function being evaluated. This is rare and self-correcting.
Under §3.3 it surfaces as `CollectorError::Query`, so the page shows the message for one slow tick
and recovers on the next. No special handling beyond that classification.

---

## 7. The shared `table/` module

Phase 1 built the Sessions table inline: store, filter, sorter, factory and per-column comparator,
about 200 lines. Phase 2 adds three more tables. Parent spec §3.2 planned `src/table/` and it was
never built; it is built now, and Sessions moves onto it.

### 7.1 Shape

```rust
pub struct Column<T> {
    pub title: &'static str,
    pub render: fn(&T) -> String,
    /// Present for columns whose values are numbers, so header clicks sort
    /// numerically rather than lexically.
    pub sort_key: Option<fn(&T) -> f64>,
    pub expand: bool,
}

pub struct Table<T> { /* store, filter, sorter */ }

impl<T: Clone + 'static> Table<T> {
    pub fn attach(view: &gtk::ColumnView, columns: &[Column<T>], matches: impl Fn(&T) -> bool + 'static) -> Self;
    pub fn update(&self, rows: &[T]);
    pub fn refilter(&self);
}
```

### 7.2 One row object, not four

GObject subclasses cannot be generic, so the obvious routes are a macro generating a row type per
table, or one row type erasing the payload. Phase 2 takes the second: a single `McpgRowObject`
holding `Rc<dyn Any>`, with `Table<T>` performing the downcast. Callers never see `Any` — the type
parameter on `Table<T>` keeps the API typed — and all four tables share one registered GObject type
instead of four near-identical ones.

The `set_incremental(false)` workaround for the upstream GTK sort/filter crash then exists in exactly
one place, which is the main practical reason to do this at all.

### 7.3 Migration order

Sessions migrates **first**, before any new page depends on the module. Its three existing comparator
tests are the evidence that the extraction preserved behaviour; migrating it last would mean building
three tables on unproven machinery.

`f64` sort keys carry integers exactly to 2^53, far beyond any counter these tables display.

---

## 8. Persistence

New GSettings keys:

| Key | Type | Range | Default | Meaning |
|-----|------|-------|---------|---------|
| `slow-sample-interval-ms` | i | 2000–300000 | 10000 | Minimum gap between slow-tier samples |
| `statements-limit` | i | 10–1000 | 200 | Rows fetched from `pg_stat_statements` per slow sample |
| `relations-limit` | i | 10–1000 | 200 | Rows fetched from each of the two relation views |

No new credential or server-list state. Nothing here can contain a password.

---

## 9. Module layout

```
src/
  collector/
    statements.rs   STATEMENTS_SQL, row mapping, StatementKey, delta derivation
    relations.rs    TABLES_SQL, INDEXES_SQL, row mapping, ratio derivation
    snapshot.rs     + the two Option<Result<…>> fields
    worker.rs       + slow-tier scheduling and error classification
    queries.rs      unchanged
  connection/
    probe.rs        + StatementsAvailability
  pages/
    queries.rs      Queries page
    relations.rs    Tables & Indexes page
  table/
    mod.rs          shared ColumnView machinery
```

Phase 1's fast-tier SQL stays in `collector/queries.rs`. Slow-tier SQL lives beside the logic that
consumes it, because the delta derivation and the statement that feeds it are one unit of reasoning
and splitting them across files serves nobody. The inconsistency is deliberate and recorded here so
it is not read as an oversight.

The parent spec's ~800-line ceiling holds. `pages/queries.rs` is the file most at risk; if the
availability status pages push it over, the status-page construction splits out.

---

## 10. Version support

Every column Phase 2 reads from `pg_stat_statements` 1.8+, `pg_stat_user_tables` and
`pg_stat_user_indexes` is present and unchanged across PostgreSQL 14 through 18. Each statement is a
single `&'static str`, and `sql_for(version)` is still not built.

Parent spec §5 predicted Phase 2 would be where a version selector became necessary. **That
prediction was wrong**, and this spec supersedes it. Later versions added views (`pg_stat_io` in 16,
`pg_stat_checkpointer` in 17) but changed no column Phase 2 consumes. The gating that did materialise
is on the `pg_stat_statements` extension version (§4), which is a different axis entirely — a
server's PostgreSQL version tells you nothing about which extension version is installed on it.

---

## 11. Error handling

Additions to parent spec §8:

| Condition | Behaviour |
|-----------|-----------|
| `pg_stat_statements` not installed | Queries page shows a status page with the remedy. No error banner. Other pages unaffected |
| Extension older than 1.8 | Queries page shows a status page naming the installed version and the `ALTER EXTENSION` remedy |
| Slow-tier query fails with `Query` | That page renders the reason inline. The sample still succeeds; the failure counter does not advance |
| Slow-tier query times out or the connection drops | Treated as a failed sample, exactly as Phase 1 |
| Statement entry absent from the previous sample | No delta for that row; cumulative values still shown |
| Statement counters went backwards | No delta for that row — reset or cache eviction |
| Relation dropped between catalogue read and size call | Surfaces as a `Query` error for one tick; recovers on the next |
| Role lacks `pg_monitor` | Query text reads `<insufficient privilege>`; the existing window banner already explains it |

---

## 12. Testing

**Unit tests** (pure functions, no database):

- Statement delta derivation: a new entry yields no delta; backwards counters yield no delta; a
  normal interval yields the expected per-second values; zero elapsed time yields no delta.
- Interval mean is `None` when the statement was not called during the interval.
- `StatementKey` falls back to a text hash when `queryid` is NULL, and two different texts do not
  collide onto one key.
- `StatementsAvailability::classify` across absent, 1.7, 1.8, 1.9, 1.10 and 1.11 — the 1.10-versus-1.8
  case is the one that catches lexical comparison.
- Dead-tuple percentage: normal, all-dead, and `None` for a table with no tuples.
- Sequential-scan ratio: normal, and `None` for a never-scanned table.
- The migrated Sessions comparator tests, unchanged, now exercising `table/`.

**Integration tests** (`testcontainers`, real servers), extending `tests/portability.rs`:

- Start **postgres:14** and **postgres:18** with `-c shared_preload_libraries=pg_stat_statements`,
  run `CREATE EXTENSION pg_stat_statements`, then execute all three new statements and map every row.
  This is the test that catches a column missing on the floor version.
- Probe a plain container — no extension — and assert `NotInstalled`, proving the gate does not
  depend on a failed query.
- Execute a known statement, take two slow samples, and assert a delta is derived for it.

**Not tested automatically:** GTK rendering and interaction, as in Phase 1. Verified by running the
application.

Development follows TDD where practical. The delta derivation, the availability classification and
the ratio helpers are written test-first.

---

## 13. Success criteria

1. `meson setup build && ninja -C build` produces a running binary, as before.
2. The Queries page lists top statements against a live server, sortable and filterable, with the
   **Last interval / Since reset** toggle changing the numbers.
3. Connecting to a server without `pg_stat_statements` shows the status page with its remedy, raises
   no error banner, and leaves Overview and Sessions working normally.
4. The Tables tab lists tables with size, live and dead tuples, dead percentage and scan counts; the
   Indexes tab lists indexes, and **Unused only** excludes primary-key and unique indexes.
5. The slow tier does not disturb the fast tier: Overview graphs and the Sessions table keep updating
   at the configured fast interval while the slow tier runs.
6. Sessions behaves identically after migrating to `table/` — sortable columns, filter entry, hide
   idle by default.
7. Killing `pg_stat_statements` privileges mid-session degrades only the Queries page; the connection
   survives and the other pages keep sampling.
8. `cargo fmt` produces no diff; unit and portability tests pass on 14 and 18; no source file exceeds
   roughly 800 lines.

---

## 14. Open questions

None blocking. Deferred decisions are recorded in §2.2, §5.5 and §6.4.

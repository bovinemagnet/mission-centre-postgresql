/* portability.rs
 *
 * Copyright 2026 Paul Snow
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! Proves the Phase 1 SQL runs unchanged on the version floor and the newest
//! supported release. If a column is missing on PostgreSQL 14, this is what
//! catches it.

use std::time::Duration;

use mission_centre_pg::actions::sql::plan_for;
use mission_centre_pg::actions::{Action, MaintenanceKind};
use mission_centre_pg::collector::locks::{build_forest, map_participant, BLOCKED_SQL};
use mission_centre_pg::collector::queries::{
    count_sessions, map_database_counters, map_session, map_settings, ACTIVITY_SQL,
    DATABASE_SIZE_SQL, DATABASE_STATS_SQL, SETTINGS_SQL,
};
use mission_centre_pg::collector::relations::{
    map_index_stats, map_table_stats, tables_sql, INDEXES_SQL,
};
use mission_centre_pg::collector::snapshot::DatabaseCounters;
use mission_centre_pg::collector::statements::{
    apply_deltas, counters_by_key, map_statement, STATEMENTS_SQL,
};
use mission_centre_pg::connection::probe::{
    map_server_info, PrivilegeLevel, StatementsAvailability, PROBE_SQL,
};
use mission_centre_pg::history::pgconsole::{
    map_system_row, PgConsoleAvailability, INSERT_QUERY_SQL, INSERT_SYSTEM_SQL, LOAD_SYSTEM_SQL,
    PGCONSOLE_PROBE_SQL,
};
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

async fn start_container(tag: &str) -> testcontainers::ContainerAsync<Postgres> {
    Postgres::default()
        .with_tag(tag)
        .start()
        .await
        .expect("failed to start the PostgreSQL container")
}

/// Opens a connection to an already-running container as the given role.
/// Kept separate from `connect` so a second, differently-authenticated
/// connection can be opened against the same container.
async fn connect_as(
    container: &testcontainers::ContainerAsync<Postgres>,
    user: &str,
    password: &str,
) -> tokio_postgres::Client {
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("failed to read the mapped port");

    let (client, connection) = tokio_postgres::Config::new()
        .host("127.0.0.1")
        .port(port)
        .user(user)
        .password(password)
        .dbname("postgres")
        .connect(tokio_postgres::NoTls)
        .await
        .expect("failed to connect to the container");

    tokio::spawn(async move {
        let _ = connection.await;
    });

    client
}

async fn connect(
    tag: &str,
) -> (
    tokio_postgres::Client,
    testcontainers::ContainerAsync<Postgres>,
) {
    let container = start_container(tag).await;
    let client = connect_as(&container, "postgres", "postgres").await;
    (client, container)
}

/// A container with pg_stat_statements preloaded and the extension created.
/// The library must be in shared_preload_libraries before the server starts;
/// CREATE EXTENSION alone is not enough.
async fn connect_with_statements(
    tag: &str,
) -> (
    tokio_postgres::Client,
    testcontainers::ContainerAsync<Postgres>,
) {
    let container = Postgres::default()
        .with_tag(tag)
        .with_cmd([
            "postgres",
            "-c",
            "shared_preload_libraries=pg_stat_statements",
        ])
        .start()
        .await
        .expect("failed to start the PostgreSQL container");
    let client = connect_as(&container, "postgres", "postgres").await;
    client
        .batch_execute("CREATE EXTENSION pg_stat_statements")
        .await
        .expect("failed to create the extension");
    (client, container)
}

/// Retries `attempt` until it returns `Some`, or gives up after five seconds.
/// Used where a PostgreSQL stats view can lag the DML that produced it.
async fn wait_for<F, Fut, T>(mut attempt: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    for _ in 0..25 {
        if let Some(value) = attempt().await {
            return Some(value);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

async fn assert_all_statements_run(tag: &str) {
    let (client, _container) = connect(tag).await;

    let probe = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("probe failed");
    let info = map_server_info(&probe);
    assert!(
        info.version_num >= 140000,
        "unexpected server version {}",
        info.version_display
    );
    assert_eq!(
        info.privilege,
        PrivilegeLevel::Superuser,
        "the container's postgres role should be a superuser"
    );

    let rows = client
        .query(DATABASE_STATS_SQL, &[])
        .await
        .expect("pg_stat_database query failed");
    assert!(!rows.is_empty(), "pg_stat_database returned no rows");
    let counters: Vec<DatabaseCounters> = rows.iter().map(map_database_counters).collect();
    let totals = DatabaseCounters::sum(&counters);
    assert!(
        totals.xact_commit > 0,
        "expected some committed transactions"
    );

    let rows = client
        .query(ACTIVITY_SQL, &[])
        .await
        .expect("pg_stat_activity query failed");
    assert!(!rows.is_empty(), "pg_stat_activity returned no rows");
    let sessions: Vec<_> = rows.iter().map(map_session).collect();
    let counts = count_sessions(&sessions);
    assert!(
        counts.total() > 0,
        "expected at least one session, since the test's own connection is one"
    );

    let row = client
        .query_one(SETTINGS_SQL, &[])
        .await
        .expect("settings query failed");
    assert!(map_settings(&row).max_connections > 0);

    let row = client
        .query_one(DATABASE_SIZE_SQL, &[])
        .await
        .expect("database size query failed");
    let size: i64 = row.get("size");
    assert!(size > 0, "database size should be positive");
}

#[tokio::test]
async fn all_statements_run_on_postgres_14() {
    assert_all_statements_run("14").await;
}

#[tokio::test]
async fn all_statements_run_on_postgres_18() {
    assert_all_statements_run("18").await;
}

#[tokio::test]
async fn a_role_without_pg_monitor_is_classified_as_limited() {
    let (client, container) = connect("18").await;
    client
        .batch_execute("CREATE ROLE watcher LOGIN PASSWORD 'watcher'")
        .await
        .expect("failed to create the limited role");

    let limited = client
        .query_one(
            "SELECT pg_has_role('watcher', 'pg_monitor', 'member') AS is_monitor,
                    (SELECT rolsuper FROM pg_roles WHERE rolname = 'watcher') AS is_superuser",
            &[],
        )
        .await
        .expect("privilege query failed");
    let is_monitor: bool = limited.get("is_monitor");
    let is_superuser: bool = limited.get("is_superuser");
    assert_eq!(
        PrivilegeLevel::classify(is_superuser, is_monitor),
        PrivilegeLevel::Limited
    );

    // The assertion above only exercises `classify` against hand-rolled
    // booleans. Prove it for real: connect as `watcher` and run the
    // production PROBE_SQL through that restricted connection.
    let watcher_client = connect_as(&container, "watcher", "watcher").await;
    let probe = watcher_client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("probe failed when authenticated as watcher");
    let info = map_server_info(&probe);
    assert_eq!(
        info.privilege,
        PrivilegeLevel::Limited,
        "watcher should be classified as limited when probing over its own connection"
    );
}

#[tokio::test]
async fn a_server_without_the_extension_probes_as_not_installed() {
    // The gate must not depend on issuing the query and interpreting the
    // failure: a stock container has no pg_stat_statements at all.
    let (client, _container) = connect("18").await;

    let probe = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("probe failed");

    assert_eq!(
        map_server_info(&probe).statements,
        StatementsAvailability::NotInstalled
    );
}

async fn assert_statements_sql_runs(tag: &str) {
    let (client, _container) = connect_with_statements(tag).await;

    let probe = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("probe failed");
    assert!(
        map_server_info(&probe).statements.is_available(),
        "the extension should probe as available once created"
    );

    // Give pg_stat_statements something of our own to record.
    client
        .batch_execute("SELECT 1; SELECT 1; SELECT 1")
        .await
        .expect("failed to run a sample workload");

    let rows = client
        .query(STATEMENTS_SQL, &[&200i64])
        .await
        .expect("pg_stat_statements query failed");
    assert!(!rows.is_empty(), "pg_stat_statements returned no rows");

    let statements: Vec<_> = rows.iter().map(map_statement).collect();
    assert!(
        statements.iter().all(|s| s.cumulative.calls > 0),
        "every recorded statement should have been called at least once"
    );
    assert!(
        statements.iter().all(|s| s.delta.is_none()),
        "a single sample has nothing to derive a delta from"
    );
}

#[tokio::test]
async fn statements_sql_runs_on_postgres_14() {
    assert_statements_sql_runs("14").await;
}

#[tokio::test]
async fn statements_sql_runs_on_postgres_18() {
    assert_statements_sql_runs("18").await;
}

#[tokio::test]
async fn a_delta_is_derived_across_two_statement_samples() {
    let (client, _container) = connect_with_statements("18").await;

    let first: Vec<_> = client
        .query(STATEMENTS_SQL, &[&200i64])
        .await
        .expect("first statements query failed")
        .iter()
        .map(map_statement)
        .collect();
    let previous = counters_by_key(&first);

    client
        .batch_execute("SELECT count(*) FROM pg_class")
        .await
        .expect("failed to run a workload between samples");

    let mut second: Vec<_> = client
        .query(STATEMENTS_SQL, &[&200i64])
        .await
        .expect("second statements query failed")
        .iter()
        .map(map_statement)
        .collect();
    apply_deltas(&mut second, &previous, Duration::from_secs(1));

    assert!(
        second.iter().any(|s| s.delta.is_some()),
        "at least one statement seen in both samples should carry a delta"
    );
}

async fn assert_relations_sql_runs(tag: &str) {
    let (client, _container) = connect(tag).await;

    let version_num: i32 = client
        .query_one("SELECT current_setting('server_version_num')::int", &[])
        .await
        .expect("failed to read the server version")
        .get(0);
    let tables_query = tables_sql(version_num);

    // pg_stat_user_tables excludes system catalogues, so a stock container
    // has nothing to report until a user table exists.
    client
        .batch_execute(
            "CREATE TABLE orders (id bigserial PRIMARY KEY, note text);
             CREATE INDEX orders_note_idx ON orders (note);
             INSERT INTO orders (note) SELECT 'n' || g FROM generate_series(1, 500) g;
             DELETE FROM orders WHERE id % 5 = 0;
             ANALYZE orders;",
        )
        .await
        .expect("failed to create the sample schema");

    // PostgreSQL throttles pgstat_report_stat() to at most once per second
    // (PGSTAT_MIN_INTERVAL), so pg_stat_user_tables can lag the DML above by
    // up to that long regardless of server version. Poll rather than assert
    // immediately, which would race that throttle.
    let orders = wait_for(|| async {
        let rows = client
            .query(&tables_query, &[&200i64])
            .await
            .expect("pg_stat_user_tables query failed");
        rows.iter()
            .map(map_table_stats)
            .find(|t| t.table_name == "orders" && t.dead_tuple_ratio().is_some())
    })
    .await
    .expect("the orders table should be reported with a dead-tuple ratio once stats flush");
    assert!(orders.total_bytes > 0, "the table should have a size");
    assert!(
        orders.can_maintain,
        "postgres owns the table it created and must be able to maintain it"
    );

    let rows = client
        .query(INDEXES_SQL, &[&200i64])
        .await
        .expect("pg_stat_user_indexes query failed");
    let indexes: Vec<_> = rows.iter().map(map_index_stats).collect();
    assert!(
        indexes
            .iter()
            .any(|i| i.index_name == "orders_pkey" && i.is_primary),
        "the primary key should be reported and flagged"
    );
    assert!(
        indexes
            .iter()
            .any(|i| i.index_name == "orders_note_idx" && i.is_unused()),
        "the never-queried secondary index should be reported as unused"
    );
    assert!(
        !indexes
            .iter()
            .any(|i| i.index_name == "orders_pkey" && i.is_unused()),
        "an unscanned primary key must never be reported as unused"
    );
}

#[tokio::test]
async fn relations_sql_runs_on_postgres_14() {
    assert_relations_sql_runs("14").await;
}

#[tokio::test]
async fn relations_sql_runs_on_postgres_18() {
    assert_relations_sql_runs("18").await;
}

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
                &47i32,         // total_connections
                &100i32,        // max_connections
                &3i32,          // active_queries
                &40i32,         // idle_connections
                &4i32,          // idle_in_transaction
                &Some(0.95f64), // cache_hit_ratio
                &Some(2048i64), // total_database_size_bytes
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

    // The query INSERT matches the column types on this version, and reads back.
    client
        .execute(
            INSERT_QUERY_SQL,
            &[
                &server_id,
                &"1234567890", // query_id (pg-console's column is TEXT)
                &"SELECT 1",   // query_text
                &10i64,        // total_calls
                &55.5f64,      // total_time_ms
                &10i64,        // total_rows
                &5.55f64,      // mean_time_ms
                &100i64,       // shared_blks_hit
                &2i64,         // shared_blks_read
            ],
        )
        .await
        .expect("query INSERT failed");

    let row = client
        .query_one(
            "SELECT query_id, query_text, total_calls, mean_time_ms \
               FROM pgconsole.query_metrics_history WHERE instance_id = $1",
            &[&server_id],
        )
        .await
        .unwrap();
    let query_id: String = row.get("query_id");
    assert_eq!(query_id, "1234567890");
    let query_text: Option<String> = row.get("query_text");
    assert_eq!(query_text.as_deref(), Some("SELECT 1"));
    let total_calls: i64 = row.get("total_calls");
    assert_eq!(total_calls, 10);
    let mean_time_ms: f64 = row.get("mean_time_ms");
    assert_eq!(mean_time_ms, 5.55);

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

/// The four capability columns must run on every supported major. The two
/// guarded expressions are the point: `to_regprocedure` on a server without
/// pg_stat_statements, and the `pg_roles` subselect on a server without
/// `pg_maintain` (14 through 16). Either written naively raises, and a raising
/// probe fails the connection outright.
async fn assert_capability_probe_runs(tag: &str) {
    let (client, container) = connect(tag).await;

    let row = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("the probe must run on a stock server with no extension");
    let info = map_server_info(&row);
    assert!(
        info.capabilities.signal_backend,
        "postgres is a superuser and must be able to signal"
    );
    assert!(info.capabilities.reload_conf);
    assert!(
        !info.capabilities.reset_statements,
        "the extension is absent, so the reset function does not exist"
    );

    client
        .batch_execute("CREATE ROLE plain LOGIN PASSWORD 'plain'")
        .await
        .expect("failed to create the unprivileged role");
    let plain = connect_as(&container, "plain", "plain").await;
    let row = plain
        .query_one(PROBE_SQL, &[])
        .await
        .expect("the probe must run for an unprivileged role too");
    let info = map_server_info(&row);
    assert!(
        !info.capabilities.signal_backend,
        "a plain role may not signal other backends"
    );
    assert!(
        !info.capabilities.maintain,
        "a plain role holds no server-wide maintenance privilege"
    );
}

/// The absent-extension case above cannot catch the opposite mistake: a probe
/// that finds nothing on a server where the reset function is present and
/// callable. From pg_stat_statements 1.11 the function carries four defaulted
/// arguments and has no zero-argument overload, so a signature-based lookup
/// silently reports "not permitted" on a perfectly good server and the menu
/// item is disabled for ever.
async fn assert_reset_capability_is_seen(tag: &str) {
    let (client, _container) = connect_with_statements(tag).await;

    let row = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("the probe must run with the extension installed");
    let info = map_server_info(&row);
    assert!(
        info.statements.is_available(),
        "the fixture installs the extension"
    );
    assert!(
        info.capabilities.reset_statements,
        "a superuser on a server carrying pg_stat_statements may reset it"
    );

    // Prove the capability was not merely reported: the statement it gates
    // must actually run.
    let plan = plan_for(&Action::ResetStatements);
    client
        .execute(plan.sql.as_str(), &[])
        .await
        .expect("pg_stat_statements_reset must run when the probe says it may");
}

#[tokio::test]
async fn reset_capability_is_seen_on_postgres_14() {
    assert_reset_capability_is_seen("14").await;
}

#[tokio::test]
async fn reset_capability_is_seen_on_postgres_18() {
    assert_reset_capability_is_seen("18").await;
}

#[tokio::test]
async fn capability_probe_runs_on_postgres_14() {
    assert_capability_probe_runs("14").await;
}

#[tokio::test]
async fn capability_probe_runs_on_postgres_18() {
    assert_capability_probe_runs("18").await;
}

/// Every action statement must actually run on both supported extremes. The
/// two that could plausibly break are the maintenance ones: `batch_execute`
/// carries them on the simple protocol because VACUUM cannot run inside the
/// implicit transaction the extended protocol opens.
async fn assert_action_statements_run(tag: &str) {
    let (client, _container) = connect(tag).await;
    client
        .batch_execute("CREATE TABLE orders (id bigserial PRIMARY KEY, note text)")
        .await
        .expect("failed to create the sample table");

    for kind in [
        MaintenanceKind::Analyze,
        MaintenanceKind::Vacuum,
        MaintenanceKind::VacuumAnalyze,
    ] {
        let plan = plan_for(&Action::Maintain {
            kind,
            schema: "public".to_string(),
            table: "orders".to_string(),
        });
        client
            .batch_execute(&plan.setup)
            .await
            .expect("the maintenance session settings must apply");
        client
            .batch_execute(&plan.sql)
            .await
            .unwrap_or_else(|e| panic!("{:?} failed: {e}", plan.sql));
    }

    let reload = plan_for(&Action::ReloadConfig);
    client
        .batch_execute(&reload.setup)
        .await
        .expect("the quick session settings must apply");
    client
        .execute(reload.sql.as_str(), &[])
        .await
        .expect("pg_reload_conf must run as superuser");

    // A backend that has already gone: the signal returns false rather than
    // raising, which is the NoSuchBackend case the UI reports honestly.
    let cancel = plan_for(&Action::CancelBackend { pid: 1 });
    let row = client
        .query_one(cancel.sql.as_str(), &[&cancel.pid.expect("a pid")])
        .await
        .expect("pg_cancel_backend must run");
    let signalled: bool = row.get(0);
    assert!(!signalled, "PID 1 is not a backend of this server");
}

#[tokio::test]
async fn action_statements_run_on_postgres_14() {
    assert_action_statements_run("14").await;
}

#[tokio::test]
async fn action_statements_run_on_postgres_18() {
    assert_action_statements_run("18").await;
}

/// The blocked-lock query must run on both versions, and must report nothing
/// on a server with no contention — the healthy case the page renders as
/// "No blocked sessions".
async fn assert_blocked_sql_runs(tag: &str) {
    let (client, _container) = connect(tag).await;

    let rows = client
        .query(BLOCKED_SQL, &[])
        .await
        .expect("the blocked-lock query must run");

    assert!(rows.is_empty(), "an idle server has no lock contention");
}

#[tokio::test]
async fn blocked_sql_runs_on_postgres_14() {
    assert_blocked_sql_runs("14").await;
}

#[tokio::test]
async fn blocked_sql_runs_on_postgres_18() {
    assert_blocked_sql_runs("18").await;
}

/// Real contention, not a fixture: one transaction holds a row lock, a second
/// waits on it. Proves the union of waiters and blockers produces a root
/// carrying the holder's state, which is the whole point of the query.
async fn assert_blocked_sql_finds_a_real_conflict(tag: &str) {
    let (client, container) = connect(tag).await;
    client
        .batch_execute(
            "CREATE TABLE conflict (id int PRIMARY KEY, note text);
             INSERT INTO conflict VALUES (1, 'a')",
        )
        .await
        .expect("failed to create the conflict table");

    let holder = connect_as(&container, "postgres", "postgres").await;
    holder
        .batch_execute("BEGIN; UPDATE conflict SET note = 'held' WHERE id = 1")
        .await
        .expect("the holder must take the row lock");

    let waiter = connect_as(&container, "postgres", "postgres").await;
    let waiting = tokio::spawn(async move {
        waiter
            .execute("UPDATE conflict SET note = 'waiting' WHERE id = 1", &[])
            .await
    });

    // The waiter needs a moment to reach the lock manager and register.
    let forest = wait_for(|| async {
        let rows = client.query(BLOCKED_SQL, &[]).await.ok()?;
        let participants: Vec<_> = rows.iter().map(map_participant).collect();
        let forest = build_forest(&participants);
        (!forest.is_empty()).then_some(forest)
    })
    .await
    .expect("the conflict must appear in the blocked-lock query");

    assert_eq!(forest.len(), 1, "one chain, not several");
    assert_eq!(forest[0].children.len(), 1, "with one waiter beneath it");
    assert_eq!(
        forest[0].participant.state.as_deref(),
        Some("idle in transaction"),
        "the root is the transaction holding the lock"
    );
    assert!(
        forest[0].children[0].participant.waiting,
        "the child is the backend actually waiting"
    );

    holder
        .batch_execute("ROLLBACK")
        .await
        .expect("failed to release the lock");
    let _ = waiting.await;
}

#[tokio::test]
async fn blocked_sql_finds_a_real_conflict_on_postgres_14() {
    assert_blocked_sql_finds_a_real_conflict("14").await;
}

#[tokio::test]
async fn blocked_sql_finds_a_real_conflict_on_postgres_18() {
    assert_blocked_sql_finds_a_real_conflict("18").await;
}

/// Settles the privilege question by observation: a role without pg_monitor
/// must still be able to run the query, whatever it does or does not mask.
async fn assert_blocked_sql_runs_for_a_plain_role(tag: &str) {
    let (client, container) = connect(tag).await;
    client
        .batch_execute("CREATE ROLE plain LOGIN PASSWORD 'plain'")
        .await
        .expect("failed to create the plain role");

    let plain = connect_as(&container, "plain", "plain").await;
    let rows = plain
        .query(BLOCKED_SQL, &[])
        .await
        .expect("a role without pg_monitor must still run the query");

    assert!(rows.is_empty(), "an idle server has no lock contention");
}

#[tokio::test]
async fn blocked_sql_runs_for_a_plain_role_on_postgres_14() {
    assert_blocked_sql_runs_for_a_plain_role("14").await;
}

#[tokio::test]
async fn blocked_sql_runs_for_a_plain_role_on_postgres_18() {
    assert_blocked_sql_runs_for_a_plain_role("18").await;
}

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

use mission_centre_pg::collector::queries::{
    count_sessions, map_database_counters, map_session, map_settings, ACTIVITY_SQL,
    DATABASE_SIZE_SQL, DATABASE_STATS_SQL, SETTINGS_SQL,
};
use mission_centre_pg::collector::snapshot::DatabaseCounters;
use mission_centre_pg::connection::probe::{map_server_info, PrivilegeLevel, PROBE_SQL};
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

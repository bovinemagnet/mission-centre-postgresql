/* history/pgconsole.rs
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

use tokio_postgres::Row;

use super::sample::SystemHistorySample;

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

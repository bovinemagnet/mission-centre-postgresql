/* queries.rs
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

use super::snapshot::{DatabaseCounters, ServerSettings, Session, SessionCounts};

/// Server-wide cumulative counters, one row per database.
/// `datname IS NOT NULL` excludes the shared-object pseudo-row.
pub const DATABASE_STATS_SQL: &str = "\
SELECT xact_commit, xact_rollback, blks_read, blks_hit,
       tup_returned, tup_fetched, tup_inserted, tup_updated, tup_deleted,
       deadlocks, temp_bytes
  FROM pg_stat_database
 WHERE datname IS NOT NULL";

/// Current sessions. `query` is NULL for other users' backends when the
/// connected role lacks pg_monitor; that is expected, not an error.
/// The duration is computed server-side so the client clock is irrelevant.
pub const ACTIVITY_SQL: &str = "\
SELECT pid,
       usename::text            AS user_name,
       application_name,
       client_addr::text        AS client_addr,
       datname                  AS database,
       state,
       wait_event_type,
       wait_event,
       backend_type,
       EXTRACT(EPOCH FROM (now() - query_start))::float8 AS query_duration_secs,
       query
  FROM pg_stat_activity
 WHERE pid <> pg_backend_pid()";

pub const SETTINGS_SQL: &str = "SELECT current_setting('max_connections')::int AS max_connections";

pub const DATABASE_SIZE_SQL: &str = "SELECT pg_database_size(current_database())::bigint AS size";

pub fn map_database_counters(row: &Row) -> DatabaseCounters {
    DatabaseCounters {
        xact_commit: row.get("xact_commit"),
        xact_rollback: row.get("xact_rollback"),
        blks_read: row.get("blks_read"),
        blks_hit: row.get("blks_hit"),
        tup_returned: row.get("tup_returned"),
        tup_fetched: row.get("tup_fetched"),
        tup_inserted: row.get("tup_inserted"),
        tup_updated: row.get("tup_updated"),
        tup_deleted: row.get("tup_deleted"),
        deadlocks: row.get("deadlocks"),
        temp_bytes: row.get("temp_bytes"),
    }
}

pub fn map_session(row: &Row) -> Session {
    Session {
        pid: row.get("pid"),
        user_name: row.get("user_name"),
        application_name: row.get("application_name"),
        client_addr: row.get("client_addr"),
        database: row.get("database"),
        state: row.get("state"),
        wait_event_type: row.get("wait_event_type"),
        wait_event: row.get("wait_event"),
        backend_type: row.get("backend_type"),
        query_duration_secs: row.get("query_duration_secs"),
        query: row.get("query"),
    }
}

pub fn count_sessions(sessions: &[Session]) -> SessionCounts {
    let mut counts = SessionCounts {
        active: 0,
        idle: 0,
        idle_in_transaction: 0,
        other: 0,
    };
    for session in sessions {
        match session.state.as_deref() {
            Some("active") => counts.active += 1,
            Some("idle") => counts.idle += 1,
            Some(s) if s.starts_with("idle in transaction") => counts.idle_in_transaction += 1,
            _ => counts.other += 1,
        }
    }
    counts
}

pub fn map_settings(row: &Row) -> ServerSettings {
    ServerSettings {
        max_connections: row.get("max_connections"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(state: Option<&str>) -> Session {
        Session {
            pid: 1,
            user_name: None,
            application_name: None,
            client_addr: None,
            database: None,
            state: state.map(str::to_string),
            wait_event_type: None,
            wait_event: None,
            backend_type: None,
            query_duration_secs: None,
            query: None,
        }
    }

    #[test]
    fn counts_sessions_by_state() {
        let sessions = vec![
            session(Some("active")),
            session(Some("active")),
            session(Some("idle")),
            session(Some("idle in transaction")),
            session(Some("fastpath function call")),
            session(None),
        ];
        let counts = count_sessions(&sessions);
        assert_eq!(counts.active, 2);
        assert_eq!(counts.idle, 1);
        assert_eq!(counts.idle_in_transaction, 1);
        assert_eq!(counts.other, 2);
        assert_eq!(counts.total(), 6);
    }

    #[test]
    fn idle_in_transaction_aborted_counts_as_idle_in_transaction() {
        let counts = count_sessions(&[session(Some("idle in transaction (aborted)"))]);
        assert_eq!(counts.idle_in_transaction, 1);
        assert_eq!(counts.idle, 0);
    }
}

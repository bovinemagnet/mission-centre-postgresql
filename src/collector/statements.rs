/* statements.rs
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

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use tokio_postgres::Row;

/// Top statements by cumulative execution time.
///
/// Three details worth keeping in view:
///   * `wal_bytes` is `numeric`; casting to float8 avoids pulling in a
///     decimal crate for one approximate size column.
///   * the `NOT LIKE` filter excludes this very statement from the report it
///     produces. It also excludes genuine user queries that mention the view
///     by name — accepted, because the alternative is the monitor
///     permanently ranking itself.
///   * `pg_roles` and `pg_database` are LEFT JOINed: a role or database
///     dropped after the statement was recorded leaves the OID dangling.
pub const STATEMENTS_SQL: &str = "\
SELECT s.queryid,
       s.userid::int8      AS user_oid,
       s.dbid::int8        AS db_oid,
       r.rolname::text     AS user_name,
       d.datname::text     AS database,
       left(s.query, 2000) AS query,
       s.calls,
       s.total_exec_time,
       s.rows,
       s.shared_blks_hit,
       s.shared_blks_read,
       s.shared_blks_dirtied,
       s.shared_blks_written,
       s.temp_blks_read,
       s.temp_blks_written,
       s.wal_bytes::float8 AS wal_bytes
  FROM pg_stat_statements s
  LEFT JOIN pg_roles    r ON r.oid = s.userid
  LEFT JOIN pg_database d ON d.oid = s.dbid
 WHERE s.query NOT LIKE '%pg_stat_statements%'
 ORDER BY s.total_exec_time DESC
 LIMIT $1";

/// How a statement is identified across samples. `queryid` is NULL for some
/// utility statements, so those fall back to a hash of the query text —
/// without a stable key no row can be matched to its previous reading and no
/// delta can be derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatementId {
    QueryId(i64),
    TextHash(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatementKey {
    pub user_oid: i64,
    pub db_oid: i64,
    pub id: StatementId,
}

pub fn statement_key(
    user_oid: i64,
    db_oid: i64,
    query_id: Option<i64>,
    query: &str,
) -> StatementKey {
    let id = match query_id {
        Some(query_id) => StatementId::QueryId(query_id),
        None => {
            let mut hasher = DefaultHasher::new();
            query.hash(&mut hasher);
            StatementId::TextHash(hasher.finish())
        }
    };
    StatementKey {
        user_oid,
        db_oid,
        id,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
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

impl StatementCounters {
    /// True if any counter is lower than in `previous`, which means the
    /// statistics were reset or this entry was evicted and its slot reused.
    pub fn went_backwards_from(&self, previous: &Self) -> bool {
        self.calls < previous.calls
            || self.total_exec_time_ms < previous.total_exec_time_ms
            || self.rows < previous.rows
            || self.shared_blks_hit < previous.shared_blks_hit
            || self.shared_blks_read < previous.shared_blks_read
            || self.shared_blks_dirtied < previous.shared_blks_dirtied
            || self.shared_blks_written < previous.shared_blks_written
            || self.temp_blks_read < previous.temp_blks_read
            || self.temp_blks_written < previous.temp_blks_written
            || self.wal_bytes < previous.wal_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatementDelta {
    pub calls_per_sec: f64,
    /// Milliseconds of execution time accrued per second. 1000 means this
    /// statement kept one core busy for the whole interval.
    pub exec_time_ms_per_sec: f64,
    /// Mean over the interval, not over the statement's lifetime. `None`
    /// when the statement was not called during the interval.
    pub mean_exec_time_ms: Option<f64>,
    pub rows_per_sec: f64,
    /// `None` when no shared blocks were touched in the interval.
    pub cache_hit_ratio: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub key: StatementKey,
    pub query: String,
    /// `None` when the recorded role or database has since been dropped.
    pub user_name: Option<String>,
    pub database: Option<String>,
    pub cumulative: StatementCounters,
    pub delta: Option<StatementDelta>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatementsSample {
    pub statements: Vec<Statement>,
}

/// Derive per-interval figures from two consecutive readings of one
/// statement. `None` when no rate can honestly be reported.
pub fn derive_delta(
    prev: &StatementCounters,
    cur: &StatementCounters,
    elapsed: Duration,
) -> Option<StatementDelta> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    if cur.went_backwards_from(prev) {
        return None;
    }

    let calls_delta = cur.calls - prev.calls;
    let time_delta = cur.total_exec_time_ms - prev.total_exec_time_ms;
    let hit_delta = cur.shared_blks_hit - prev.shared_blks_hit;
    let read_delta = cur.shared_blks_read - prev.shared_blks_read;
    let block_delta = hit_delta + read_delta;

    Some(StatementDelta {
        calls_per_sec: calls_delta as f64 / secs,
        exec_time_ms_per_sec: time_delta / secs,
        mean_exec_time_ms: if calls_delta > 0 {
            Some(time_delta / calls_delta as f64)
        } else {
            None
        },
        rows_per_sec: (cur.rows - prev.rows) as f64 / secs,
        cache_hit_ratio: if block_delta > 0 {
            Some(hit_delta as f64 / block_delta as f64)
        } else {
            None
        },
    })
}

/// Fills in `delta` for every statement that was present in the previous
/// slow sample. Statements new since then keep `None`.
pub fn apply_deltas(
    statements: &mut [Statement],
    previous: &HashMap<StatementKey, StatementCounters>,
    elapsed: Duration,
) {
    for statement in statements.iter_mut() {
        statement.delta = previous
            .get(&statement.key)
            .and_then(|prev| derive_delta(prev, &statement.cumulative, elapsed));
    }
}

pub fn counters_by_key(statements: &[Statement]) -> HashMap<StatementKey, StatementCounters> {
    statements
        .iter()
        .map(|statement| (statement.key, statement.cumulative))
        .collect()
}

pub fn map_statement(row: &Row) -> Statement {
    let user_oid: i64 = row.get("user_oid");
    let db_oid: i64 = row.get("db_oid");
    let query_id: Option<i64> = row.get("queryid");
    // A role without pg_monitor sees the literal text
    // "<insufficient privilege>" rather than NULL, but map defensively: the
    // parser must never panic on server output.
    let query: String = row.get::<_, Option<String>>("query").unwrap_or_default();

    Statement {
        key: statement_key(user_oid, db_oid, query_id, &query),
        query,
        user_name: row.get("user_name"),
        database: row.get("database"),
        cumulative: StatementCounters {
            calls: row.get("calls"),
            total_exec_time_ms: row.get("total_exec_time"),
            rows: row.get("rows"),
            shared_blks_hit: row.get("shared_blks_hit"),
            shared_blks_read: row.get("shared_blks_read"),
            shared_blks_dirtied: row.get("shared_blks_dirtied"),
            shared_blks_written: row.get("shared_blks_written"),
            temp_blks_read: row.get("temp_blks_read"),
            temp_blks_written: row.get("temp_blks_written"),
            wal_bytes: row.get("wal_bytes"),
        },
        delta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(calls: i64, time_ms: f64, rows: i64, hit: i64, read: i64) -> StatementCounters {
        StatementCounters {
            calls,
            total_exec_time_ms: time_ms,
            rows,
            shared_blks_hit: hit,
            shared_blks_read: read,
            ..StatementCounters::default()
        }
    }

    #[test]
    fn derives_per_second_rates_from_the_delta() {
        let prev = counters(100, 1_000.0, 500, 900, 100);
        let cur = counters(300, 3_000.0, 1_500, 2_700, 300);
        let delta = derive_delta(&prev, &cur, Duration::from_secs(2)).unwrap();

        assert_eq!(delta.calls_per_sec, 100.0);
        assert_eq!(delta.exec_time_ms_per_sec, 1_000.0);
        assert_eq!(delta.rows_per_sec, 500.0);
    }

    #[test]
    fn the_interval_mean_is_the_intervals_time_over_its_calls() {
        // Lifetime mean would be 3000/300 = 10ms. The interval mean is
        // 2000/200 = 10ms here only by coincidence, so use figures that differ.
        let prev = counters(100, 1_000.0, 0, 0, 0);
        let cur = counters(200, 5_000.0, 0, 0, 0);
        let delta = derive_delta(&prev, &cur, Duration::from_secs(1)).unwrap();

        assert_eq!(delta.mean_exec_time_ms, Some(40.0));
    }

    #[test]
    fn a_statement_not_called_during_the_interval_has_no_interval_mean() {
        let prev = counters(100, 1_000.0, 0, 0, 0);
        let cur = counters(100, 1_000.0, 0, 0, 0);
        let delta = derive_delta(&prev, &cur, Duration::from_secs(1)).unwrap();

        assert_eq!(delta.mean_exec_time_ms, None);
        assert_eq!(delta.calls_per_sec, 0.0);
    }

    #[test]
    fn a_cache_ratio_needs_blocks_to_have_been_touched() {
        let prev = counters(1, 1.0, 0, 100, 10);
        let cur = counters(2, 2.0, 0, 100, 10);
        assert_eq!(
            derive_delta(&prev, &cur, Duration::from_secs(1))
                .unwrap()
                .cache_hit_ratio,
            None
        );

        let cur = counters(2, 2.0, 0, 190, 20);
        assert_eq!(
            derive_delta(&prev, &cur, Duration::from_secs(1))
                .unwrap()
                .cache_hit_ratio,
            Some(0.9)
        );
    }

    #[test]
    fn counters_going_backwards_yield_no_delta() {
        // pg_stat_statements_reset(), or the entry was evicted at
        // pg_stat_statements.max and its slot reused by another statement.
        let prev = counters(300, 3_000.0, 0, 0, 0);
        let cur = counters(10, 100.0, 0, 0, 0);
        assert_eq!(derive_delta(&prev, &cur, Duration::from_secs(2)), None);
    }

    #[test]
    fn zero_elapsed_time_yields_no_delta() {
        let prev = counters(100, 1_000.0, 0, 0, 0);
        let cur = counters(200, 2_000.0, 0, 0, 0);
        assert_eq!(derive_delta(&prev, &cur, Duration::ZERO), None);
    }

    #[test]
    fn a_null_query_id_falls_back_to_a_hash_of_the_text() {
        let key = statement_key(10, 20, None, "VACUUM orders");
        assert!(matches!(key.id, StatementId::TextHash(_)));

        // The same text must produce the same key, or no row ever matches
        // its previous sample and no delta is ever derived.
        assert_eq!(key, statement_key(10, 20, None, "VACUUM orders"));
    }

    #[test]
    fn different_query_texts_do_not_collide_onto_one_key() {
        assert_ne!(
            statement_key(10, 20, None, "VACUUM orders"),
            statement_key(10, 20, None, "VACUUM customers")
        );
    }

    #[test]
    fn a_present_query_id_is_used_in_preference_to_the_text() {
        assert_eq!(
            statement_key(10, 20, Some(42), "SELECT 1"),
            StatementKey {
                user_oid: 10,
                db_oid: 20,
                id: StatementId::QueryId(42),
            }
        );
    }

    #[test]
    fn the_same_query_id_under_a_different_role_is_a_different_statement() {
        assert_ne!(
            statement_key(10, 20, Some(42), "SELECT 1"),
            statement_key(11, 20, Some(42), "SELECT 1")
        );
    }

    fn statement(key: StatementKey, cumulative: StatementCounters) -> Statement {
        Statement {
            key,
            query: "SELECT 1".to_string(),
            user_name: None,
            database: None,
            cumulative,
            delta: None,
        }
    }

    #[test]
    fn apply_deltas_fills_in_rows_seen_in_the_previous_sample() {
        let key = statement_key(10, 20, Some(1), "SELECT 1");
        let mut statements = vec![statement(key, counters(200, 2_000.0, 0, 0, 0))];

        let mut previous = HashMap::new();
        previous.insert(key, counters(100, 1_000.0, 0, 0, 0));

        apply_deltas(&mut statements, &previous, Duration::from_secs(1));

        assert_eq!(statements[0].delta.unwrap().calls_per_sec, 100.0);
    }

    #[test]
    fn apply_deltas_leaves_a_new_statement_without_one() {
        let key = statement_key(10, 20, Some(2), "SELECT 2");
        let mut statements = vec![statement(key, counters(5, 50.0, 0, 0, 0))];

        apply_deltas(&mut statements, &HashMap::new(), Duration::from_secs(1));

        assert_eq!(statements[0].delta, None);
    }

    #[test]
    fn counters_by_key_indexes_every_statement() {
        let first = statement_key(10, 20, Some(1), "SELECT 1");
        let second = statement_key(10, 20, Some(2), "SELECT 2");
        let statements = vec![
            statement(first, counters(1, 1.0, 0, 0, 0)),
            statement(second, counters(2, 2.0, 0, 0, 0)),
        ];

        let indexed = counters_by_key(&statements);
        assert_eq!(indexed.len(), 2);
        assert_eq!(indexed[&second].calls, 2);
    }
}

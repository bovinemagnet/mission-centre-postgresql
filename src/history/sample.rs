/* history/sample.rs
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

use crate::collector::snapshot::Snapshot;
use crate::collector::statements::{Statement, StatementId};

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
            locks: None,
            lock_inventory: None,
            replication: None,
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

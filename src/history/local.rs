/* history/local.rs
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

use rusqlite::Connection;

use super::sample::{QueryHistorySample, SystemHistorySample};

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
        store
            .write_system("srv", 1_000, &system(11, Some(0.9)))
            .unwrap();

        let loaded = store.load_recent_system("srv", 10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], system(11, Some(0.9)));
    }

    #[test]
    fn a_null_cache_ratio_survives_the_round_trip_as_none() {
        let store = LocalStore::open_in_memory().unwrap();
        store.write_system("srv", 1_000, &system(11, None)).unwrap();
        assert_eq!(
            store.load_recent_system("srv", 10).unwrap()[0].cache_hit_ratio,
            None
        );
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
        assert_eq!(
            store.load_recent_system("a", 10).unwrap()[0].total_connections,
            1
        );
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
        // Round-trip is proven via direct row reads, since Phase 3 renders no
        // query-history view to load them back through. Ordering by
        // total_calls descending makes the two rows deterministic.
        let mut statement = store
            .connection()
            .prepare(
                "SELECT query_id, query_text, total_calls FROM query_history
                  WHERE server_id = 'srv'
                  ORDER BY total_calls DESC",
            )
            .unwrap();
        let rows: Vec<(Option<i64>, String, i64)> = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], (Some(42), "SELECT 1".to_string(), 10));
        // The query_id column must read back as SQL NULL, not a sentinel
        // such as zero.
        assert_eq!(rows[1], (None, "VACUUM t".to_string(), 1));
    }
}

/* history_io.rs
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

use std::time::SystemTime;

use tokio_postgres::Client;

use crate::collector::worker::{map_query_error, CollectorConfig, CollectorError};
use crate::connection::params::HistoryMode;
use crate::history::local::LocalStore;
use crate::history::pgconsole::{
    map_system_row, PgConsoleAvailability, INSERT_QUERY_SQL, INSERT_SYSTEM_SQL, LOAD_SYSTEM_SQL,
    PGCONSOLE_PROBE_SQL,
};
use crate::history::{HistoryBackend, HistoryPreload, QueryHistorySample, SystemHistorySample};

/// Resolves the effective backend for this connection and loads the recent
/// history to preload. A pgconsole schema that is missing or unwritable falls
/// back to Local with a logged note; the connection is never failed for it.
pub(super) async fn open_history(
    client: &Client,
    config: &CollectorConfig,
    preload_limit: usize,
) -> (HistoryBackend, HistoryPreload) {
    match config.history_mode {
        HistoryMode::Off => (HistoryBackend::Off, HistoryPreload::default()),
        HistoryMode::PgConsole => match probe_pgconsole(client).await {
            PgConsoleAvailability::Writable => {
                let system = load_pgconsole_history(client, preload_limit).await;
                (HistoryBackend::PgConsole, HistoryPreload { system })
            }
            other => {
                gtk_free_log(&format!(
                    "pgconsole history unavailable ({other:?}); using local history"
                ));
                open_local(config, preload_limit)
            }
        },
        HistoryMode::Local => open_local(config, preload_limit),
    }
}

async fn probe_pgconsole(client: &Client) -> PgConsoleAvailability {
    match client.query_one(PGCONSOLE_PROBE_SQL, &[]).await {
        Ok(row) => PgConsoleAvailability::classify(row.get("tables_exist"), row.get("can_insert")),
        // A probe that itself errors is treated as no usable schema.
        Err(_) => PgConsoleAvailability::SchemaMissing,
    }
}

async fn load_pgconsole_history(client: &Client, limit: usize) -> Vec<SystemHistorySample> {
    match client.query(LOAD_SYSTEM_SQL, &[&(limit as i64)]).await {
        Ok(rows) => {
            let mut samples: Vec<SystemHistorySample> = rows.iter().map(map_system_row).collect();
            samples.reverse(); // newest-first from SQL → oldest-first
            samples
        }
        Err(_) => Vec::new(),
    }
}

fn open_local(config: &CollectorConfig, preload_limit: usize) -> (HistoryBackend, HistoryPreload) {
    match LocalStore::open(&config.local_db_path) {
        Ok(store) => {
            let system = store
                .load_recent_system(&config.server_id, preload_limit)
                .unwrap_or_default();
            (HistoryBackend::Local(store), HistoryPreload { system })
        }
        Err(e) => {
            gtk_free_log(&format!(
                "local history unavailable ({e}); history disabled"
            ));
            (HistoryBackend::Off, HistoryPreload::default())
        }
    }
}

/// A log line from the collector thread. `g_warning!` is GTK-thread-only, so
/// the collector uses eprintln through the glib logger's stderr, never a GTK
/// call. Kept in one place so the GTK-free rule is easy to check.
pub(super) fn gtk_free_log(message: &str) {
    eprintln!("mission-centre-pg: {message}");
}

/// Unix-epoch second before which local history rows are pruned. Uses wall
/// clock, which is correct for a retention window; the sample loop's Instant
/// timing is monotonic and unrelated.
pub(super) fn retention_cutoff(retention_days: i64) -> i64 {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    now - retention_days * 86_400
}

/// Writes one system row and the latest query rows to whichever backend is
/// active. Local writes are synchronous SQLite calls run inline — they are
/// small and infrequent (default 60s) and the loop is serial, so a brief
/// block is acceptable and simpler than a blocking-pool hop. pgconsole writes
/// go over the existing client.
pub(super) async fn write_history(
    client: &Client,
    history: &HistoryBackend,
    config: &CollectorConfig,
    system: &SystemHistorySample,
    queries: &[QueryHistorySample],
) -> Result<(), CollectorError> {
    let sampled_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match history {
        HistoryBackend::Off => Ok(()),
        HistoryBackend::Local(store) => store
            .write_system(&config.server_id, sampled_at, system)
            .and_then(|_| store.write_queries(&config.server_id, sampled_at, queries))
            .map_err(|e| CollectorError::Query(e.to_string())),
        HistoryBackend::PgConsole => {
            client
                .execute(
                    INSERT_SYSTEM_SQL,
                    &[
                        &config.server_id,
                        &system.total_connections,
                        &system.max_connections,
                        &system.active_queries,
                        &system.idle_connections,
                        &system.idle_in_transaction,
                        &system.cache_hit_ratio,
                        &system.total_database_size_bytes,
                    ],
                )
                .await
                .map_err(map_query_error)?;
            for q in queries {
                // query_id is NOT NULL in pg-console's schema; skip the
                // text-hashed utility statements that have no queryid.
                let Some(id) = q.query_id else { continue };
                client
                    .execute(
                        INSERT_QUERY_SQL,
                        &[
                            &config.server_id,
                            &id.to_string(),
                            &q.query_text,
                            &q.total_calls,
                            &q.total_time_ms,
                            &q.total_rows,
                            &q.mean_time_ms,
                            &q.shared_blks_hit,
                            &q.shared_blks_read,
                        ],
                    )
                    .await
                    .map_err(map_query_error)?;
            }
            Ok(())
        }
    }
}

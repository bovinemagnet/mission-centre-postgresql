/* snapshot.rs
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

use std::time::Instant;

use super::locks::{LockInventorySample, LocksSample};
use super::relations::RelationsSample;
use super::statements::StatementsSample;
use super::worker::CollectorError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DatabaseCounters {
    pub xact_commit: i64,
    pub xact_rollback: i64,
    pub blks_read: i64,
    pub blks_hit: i64,
    pub tup_returned: i64,
    pub tup_fetched: i64,
    pub tup_inserted: i64,
    pub tup_updated: i64,
    pub tup_deleted: i64,
    pub deadlocks: i64,
    pub temp_bytes: i64,
}

impl DatabaseCounters {
    /// Server-wide totals. `pg_stat_database` returns one row per database;
    /// the Overview page shows the sum, because "how loaded is this server"
    /// is the question it answers.
    pub fn sum(rows: &[DatabaseCounters]) -> DatabaseCounters {
        rows.iter().fold(DatabaseCounters::default(), |mut acc, r| {
            acc.xact_commit += r.xact_commit;
            acc.xact_rollback += r.xact_rollback;
            acc.blks_read += r.blks_read;
            acc.blks_hit += r.blks_hit;
            acc.tup_returned += r.tup_returned;
            acc.tup_fetched += r.tup_fetched;
            acc.tup_inserted += r.tup_inserted;
            acc.tup_updated += r.tup_updated;
            acc.tup_deleted += r.tup_deleted;
            acc.deadlocks += r.deadlocks;
            acc.temp_bytes += r.temp_bytes;
            acc
        })
    }

    /// True if any counter is lower than in `previous`, which means the
    /// statistics were reset or the server restarted.
    pub fn went_backwards_from(&self, previous: &DatabaseCounters) -> bool {
        self.xact_commit < previous.xact_commit
            || self.xact_rollback < previous.xact_rollback
            || self.blks_read < previous.blks_read
            || self.blks_hit < previous.blks_hit
            || self.tup_returned < previous.tup_returned
            || self.tup_fetched < previous.tup_fetched
            || self.tup_inserted < previous.tup_inserted
            || self.tup_updated < previous.tup_updated
            || self.tup_deleted < previous.tup_deleted
            || self.deadlocks < previous.deadlocks
            || self.temp_bytes < previous.temp_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DatabaseRates {
    pub transactions_per_sec: f64,
    /// `None` when no blocks were accessed in the interval — there is no
    /// ratio to report, and reporting zero would be a lie.
    pub cache_hit_ratio: Option<f64>,
    pub tuples_returned_per_sec: f64,
    pub tuples_fetched_per_sec: f64,
    pub tuples_inserted_per_sec: f64,
    pub tuples_updated_per_sec: f64,
    pub tuples_deleted_per_sec: f64,
    pub deadlocks_per_sec: f64,
    pub temp_bytes_per_sec: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCounts {
    pub active: usize,
    pub idle: usize,
    pub idle_in_transaction: usize,
    pub other: usize,
}

impl SessionCounts {
    pub fn total(&self) -> usize {
        self.active + self.idle + self.idle_in_transaction + self.other
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub pid: i32,
    pub user_name: Option<String>,
    pub application_name: Option<String>,
    pub client_addr: Option<String>,
    pub database: Option<String>,
    pub state: Option<String>,
    pub wait_event_type: Option<String>,
    pub wait_event: Option<String>,
    pub backend_type: Option<String>,
    /// Seconds since `query_start`, computed server-side so the client clock
    /// is irrelevant. `None` when no query is running.
    pub query_duration_secs: Option<f64>,
    /// `None` when the connected role lacks `pg_monitor` and the backend
    /// belongs to another user.
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSettings {
    pub max_connections: i32,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub taken_at: Instant,
    pub totals: DatabaseCounters,
    pub rates: Option<DatabaseRates>,
    pub connected_database_size_bytes: Option<i64>,
    pub session_counts: SessionCounts,
    pub sessions: Vec<Session>,
    pub settings: ServerSettings,
    /// `None` on a fast tick — the page keeps its previous contents.
    /// `Err` carries the reason the page renders in place of its table.
    pub statements: Option<Result<StatementsSample, CollectorError>>,
    pub relations: Option<Result<RelationsSample, CollectorError>>,
    /// Fast tier, so `Some` on every tick. `Err` carries the reason the page
    /// renders in place of its tree.
    pub locks: Option<Result<LocksSample, CollectorError>>,
    /// `None` means the inventory view is not on screen and was not sampled —
    /// the resting state, not a failure.
    pub lock_inventory: Option<Result<LockInventorySample, CollectorError>>,
}

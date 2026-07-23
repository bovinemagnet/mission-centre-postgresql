/* rates.rs
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

use std::time::Duration;

use super::snapshot::{DatabaseCounters, DatabaseRates};

/// Derive per-interval rates from two consecutive counter readings.
///
/// Returns `None` when no rate can honestly be reported: zero elapsed time,
/// or a counter that went backwards because the statistics were reset.
pub fn derive_rates(
    prev: &DatabaseCounters,
    cur: &DatabaseCounters,
    elapsed: Duration,
) -> Option<DatabaseRates> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    if cur.went_backwards_from(prev) {
        return None;
    }

    let per_sec = |cur_v: i64, prev_v: i64| (cur_v - prev_v) as f64 / secs;

    let hit_delta = cur.blks_hit - prev.blks_hit;
    let read_delta = cur.blks_read - prev.blks_read;
    let block_delta = hit_delta + read_delta;
    let cache_hit_ratio = if block_delta > 0 {
        Some(hit_delta as f64 / block_delta as f64)
    } else {
        None
    };

    Some(DatabaseRates {
        transactions_per_sec: per_sec(
            cur.xact_commit + cur.xact_rollback,
            prev.xact_commit + prev.xact_rollback,
        ),
        cache_hit_ratio,
        tuples_returned_per_sec: per_sec(cur.tup_returned, prev.tup_returned),
        tuples_fetched_per_sec: per_sec(cur.tup_fetched, prev.tup_fetched),
        tuples_inserted_per_sec: per_sec(cur.tup_inserted, prev.tup_inserted),
        tuples_updated_per_sec: per_sec(cur.tup_updated, prev.tup_updated),
        tuples_deleted_per_sec: per_sec(cur.tup_deleted, prev.tup_deleted),
        deadlocks_per_sec: per_sec(cur.deadlocks, prev.deadlocks),
        temp_bytes_per_sec: per_sec(cur.temp_bytes, prev.temp_bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::super::snapshot::DatabaseCounters;
    use super::derive_rates;
    use std::time::Duration;

    fn counters(commit: i64, rollback: i64, hit: i64, read: i64) -> DatabaseCounters {
        DatabaseCounters {
            xact_commit: commit,
            xact_rollback: rollback,
            blks_hit: hit,
            blks_read: read,
            ..DatabaseCounters::default()
        }
    }

    #[test]
    fn derives_transactions_per_second_from_the_delta() {
        let prev = counters(1_000, 100, 0, 0);
        let cur = counters(3_000, 200, 0, 0);
        let rates = derive_rates(&prev, &cur, Duration::from_secs(2)).unwrap();
        // (3000-1000) + (200-100) = 2100 over 2 seconds
        assert_eq!(rates.transactions_per_sec, 1_050.0);
    }

    #[test]
    fn cache_hit_ratio_uses_the_interval_not_the_cumulative_totals() {
        // A long-running server with a 99.99% lifetime ratio that is currently
        // missing cache on every single read. The naive cumulative calculation
        // would report ~0.9999; the correct interval calculation reports 0.0.
        let prev = counters(0, 0, 999_900, 100);
        let cur = counters(0, 0, 999_900, 1_100);
        let rates = derive_rates(&prev, &cur, Duration::from_secs(1)).unwrap();
        assert_eq!(rates.cache_hit_ratio, Some(0.0));
    }

    #[test]
    fn cache_hit_ratio_is_none_when_no_blocks_were_accessed() {
        let prev = counters(10, 0, 500, 20);
        let cur = counters(20, 0, 500, 20);
        let rates = derive_rates(&prev, &cur, Duration::from_secs(1)).unwrap();
        assert_eq!(rates.cache_hit_ratio, None);
    }

    #[test]
    fn returns_none_when_a_counter_goes_backwards() {
        // pg_stat_reset() or a server restart.
        let prev = counters(5_000, 100, 900, 100);
        let cur = counters(12, 0, 4, 1);
        assert_eq!(derive_rates(&prev, &cur, Duration::from_secs(2)), None);
    }

    #[test]
    fn returns_none_when_no_time_has_elapsed() {
        let prev = counters(1_000, 0, 0, 0);
        let cur = counters(2_000, 0, 0, 0);
        assert_eq!(derive_rates(&prev, &cur, Duration::ZERO), None);
    }

    #[test]
    fn sums_counters_across_databases() {
        let a = counters(10, 1, 100, 5);
        let b = counters(20, 2, 200, 10);
        let total = DatabaseCounters::sum(&[a, b]);
        assert_eq!(total.xact_commit, 30);
        assert_eq!(total.xact_rollback, 3);
        assert_eq!(total.blks_hit, 300);
        assert_eq!(total.blks_read, 15);
    }
}

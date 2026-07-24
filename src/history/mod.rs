/* history/mod.rs
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

pub mod local;
pub mod pgconsole;
pub mod sample;

pub use sample::{query_samples_from, system_sample_from, QueryHistorySample, SystemHistorySample};

use std::time::{Duration, Instant};

use local::LocalStore;

/// The resolved history backend for one connection. `Local` owns its SQLite
/// connection; `PgConsole` writes through the sample loop's tokio-postgres
/// client, so it carries no state here.
pub enum HistoryBackend {
    Off,
    Local(LocalStore),
    PgConsole,
}

impl HistoryBackend {
    pub fn is_off(&self) -> bool {
        matches!(self, HistoryBackend::Off)
    }
}

/// History loaded on connect, fed into the Overview graph buffers before live
/// samples begin. Oldest-first.
#[derive(Debug, Clone, Default)]
pub struct HistoryPreload {
    pub system: Vec<SystemHistorySample>,
}

/// True when a history row should be written this tick: immediately for the
/// first write of a connection, then once `interval` has elapsed. The same
/// shape as the collector's `is_slow_tick`, and a coarser clock than the 2s
/// sample loop — writing history every sample would flood the store.
pub fn is_history_tick(last_write: Option<Instant>, now: Instant, interval: Duration) -> bool {
    match last_write {
        None => true,
        Some(previous) => now.duration_since(previous) >= interval,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_history_write_happens_immediately() {
        // With no prior write, a data point should be recorded at once so the
        // store is not empty for a whole interval after connecting.
        let now = Instant::now();
        assert!(is_history_tick(None, now, Duration::from_secs(60)));
    }

    #[test]
    fn a_write_waits_for_its_interval() {
        let now = Instant::now();
        let recent = now - Duration::from_secs(30);
        assert!(!is_history_tick(Some(recent), now, Duration::from_secs(60)));
    }

    #[test]
    fn a_write_happens_once_the_interval_has_elapsed() {
        let now = Instant::now();
        let stale = now - Duration::from_secs(61);
        assert!(is_history_tick(Some(stale), now, Duration::from_secs(60)));
    }
}

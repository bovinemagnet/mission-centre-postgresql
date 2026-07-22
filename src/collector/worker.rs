/* worker.rs
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

use std::time::{Duration, Instant};

use tokio_postgres::{error::SqlState, Client};

use crate::collector::queries::{
    count_sessions, map_database_counters, map_session, map_settings, ACTIVITY_SQL,
    DATABASE_SIZE_SQL, DATABASE_STATS_SQL, SETTINGS_SQL,
};
use crate::collector::rates::derive_rates;
use crate::collector::snapshot::{DatabaseCounters, ServerSettings, Snapshot};
use crate::connection::params::ConnectionParams;
use crate::connection::probe::{map_server_info, ServerInfo, PROBE_SQL};

/// Guards against a wedged server hanging the sampler for ever.
const STATEMENT_TIMEOUT: &str = "SET statement_timeout = '5s'";

/// Consecutive failed samples before the collector declares the connection lost.
const FAILURES_BEFORE_DISCONNECT: u32 = 3;

#[derive(Debug, Clone, thiserror::Error)]
pub enum CollectorError {
    #[error("Could not connect: {0}")]
    Connect(String),
    #[error("Query failed: {0}")]
    Query(String),
    #[error("The server did not respond within five seconds")]
    Timeout,
    #[error("The connection to the server was lost")]
    LostConnection,
}

#[derive(Debug, Clone)]
pub enum CollectorEvent {
    Connecting,
    Connected(ServerInfo),
    Sample(Box<Snapshot>),
    Error(CollectorError),
    Disconnected,
}

pub struct CollectorHandle {
    pub events: async_channel::Receiver<CollectorEvent>,
    stop: async_channel::Sender<()>,
}

impl CollectorHandle {
    pub fn stop(&self) {
        let _ = self.stop.try_send(());
    }
}

impl Drop for CollectorHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// True when the controller has asked us to stop, or has dropped the handle.
/// A closed channel means the consumer is gone, so there is nobody left to
/// sample for.
fn stop_requested(stop: &async_channel::Receiver<()>) -> bool {
    !matches!(stop.try_recv(), Err(async_channel::TryRecvError::Empty))
}

/// Why `sample_loop` returned control to `run`.
enum Exit {
    /// The controller asked us to stop, or dropped the handle.
    Stopped,
    /// Gave up after `FAILURES_BEFORE_DISCONNECT` consecutive failed samples.
    /// `had_success` records whether at least one sample succeeded earlier in
    /// this connection's lifetime, which decides whether the backoff counter
    /// resets.
    Failed { had_success: bool },
}

/// 1s, 2s, 4s, 8s, 16s, then 30s for ever.
///
/// The shift amount is bounded to 16, so `1u64 << n` cannot overflow.
pub fn backoff_delay(consecutive_failures: u32) -> Duration {
    let seconds = 1u64 << consecutive_failures.min(16);
    Duration::from_secs(seconds.min(30))
}

pub fn spawn(params: ConnectionParams, password: String, interval: Duration) -> CollectorHandle {
    let (event_tx, event_rx) = async_channel::bounded(32);
    let (stop_tx, stop_rx) = async_channel::bounded(1);

    std::thread::Builder::new()
        .name("mcpg-collector".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build the collector runtime");
            runtime.block_on(run(params, password, interval, event_tx, stop_rx));
        })
        .expect("failed to spawn the collector thread");

    CollectorHandle {
        events: event_rx,
        stop: stop_tx,
    }
}

async fn run(
    params: ConnectionParams,
    password: String,
    interval: Duration,
    events: async_channel::Sender<CollectorEvent>,
    stop: async_channel::Receiver<()>,
) {
    let mut consecutive_failures = 0u32;

    loop {
        if stop_requested(&stop) {
            return;
        }

        let _ = events.send(CollectorEvent::Connecting).await;

        match connect(&params, &password).await {
            Ok((client, info)) => {
                let _ = events.send(CollectorEvent::Connected(info)).await;
                match sample_loop(&client, interval, &events, &stop).await {
                    Exit::Stopped => return,
                    Exit::Failed { had_success } => {
                        consecutive_failures = if had_success {
                            0
                        } else {
                            consecutive_failures.saturating_add(1)
                        };
                    }
                }
                let _ = events.send(CollectorEvent::Disconnected).await;
            }
            Err(e) => {
                let _ = events.send(CollectorEvent::Error(e)).await;
                consecutive_failures = consecutive_failures.saturating_add(1);
            }
        }

        if stop_requested(&stop) {
            return;
        }

        let delay = backoff_delay(consecutive_failures.saturating_sub(1));
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = stop.recv() => return,
        }
    }
}

async fn connect(
    params: &ConnectionParams,
    password: &str,
) -> Result<(Client, ServerInfo), CollectorError> {
    let config = params.to_config(password);

    let (client, connection) = config
        .connect(tokio_postgres::NoTls)
        .await
        .map_err(|e| CollectorError::Connect(e.to_string()))?;

    tokio::spawn(async move {
        let _ = connection.await;
    });

    client
        .batch_execute(STATEMENT_TIMEOUT)
        .await
        .map_err(map_query_error)?;

    let row = client
        .query_one(PROBE_SQL, &[])
        .await
        .map_err(map_query_error)?;

    Ok((client, map_server_info(&row)))
}

/// Samples serially: the next sample starts only once the previous one has
/// finished or timed out, so a slow server spreads samples out rather than
/// piling overlapping queries onto one connection.
async fn sample_loop(
    client: &Client,
    interval: Duration,
    events: &async_channel::Sender<CollectorEvent>,
    stop: &async_channel::Receiver<()>,
) -> Exit {
    let mut previous: Option<(DatabaseCounters, Instant)> = None;
    let mut consecutive_failures = 0u32;
    let mut had_success = false;

    loop {
        if stop_requested(stop) {
            return Exit::Stopped;
        }

        match sample(client, previous).await {
            Ok(snapshot) => {
                consecutive_failures = 0;
                had_success = true;
                previous = Some((snapshot.totals, snapshot.taken_at));
                // A stalled consumer must never wedge shutdown, and a late
                // sample has no monitoring value anyway, so drop it rather
                // than block on a full channel.
                let _ = events.try_send(CollectorEvent::Sample(Box::new(snapshot)));
            }
            Err(e) => {
                consecutive_failures += 1;
                let _ = events.send(CollectorEvent::Error(e)).await;
                if consecutive_failures >= FAILURES_BEFORE_DISCONNECT {
                    return Exit::Failed { had_success };
                }
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = stop.recv() => return Exit::Stopped,
        }
    }
}

async fn sample(
    client: &Client,
    previous: Option<(DatabaseCounters, Instant)>,
) -> Result<Snapshot, CollectorError> {
    let taken_at = Instant::now();

    let stat_rows = client
        .query(DATABASE_STATS_SQL, &[])
        .await
        .map_err(map_query_error)?;
    let per_database: Vec<DatabaseCounters> = stat_rows.iter().map(map_database_counters).collect();
    let totals = DatabaseCounters::sum(&per_database);

    let activity_rows = client
        .query(ACTIVITY_SQL, &[])
        .await
        .map_err(map_query_error)?;
    let sessions: Vec<_> = activity_rows.iter().map(map_session).collect();
    let session_counts = count_sessions(&sessions);

    let settings_row = client
        .query_one(SETTINGS_SQL, &[])
        .await
        .map_err(map_query_error)?;
    let settings: ServerSettings = map_settings(&settings_row);

    let size_row = client
        .query_one(DATABASE_SIZE_SQL, &[])
        .await
        .map_err(map_query_error)?;
    let connected_database_size_bytes: Option<i64> = size_row.get("size");

    let rates = previous.and_then(|(prev_counters, prev_at)| {
        derive_rates(&prev_counters, &totals, taken_at.duration_since(prev_at))
    });

    Ok(Snapshot {
        taken_at,
        totals,
        rates,
        connected_database_size_bytes,
        session_counts,
        sessions,
        settings,
    })
}

fn map_query_error(e: tokio_postgres::Error) -> CollectorError {
    if e.code() == Some(&SqlState::QUERY_CANCELED) {
        CollectorError::Timeout
    } else if e.is_closed() {
        CollectorError::LostConnection
    } else {
        CollectorError::Query(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_then_caps_at_thirty_seconds() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(2));
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(3), Duration::from_secs(8));
        assert_eq!(backoff_delay(4), Duration::from_secs(16));
        assert_eq!(backoff_delay(5), Duration::from_secs(30));
        assert_eq!(backoff_delay(50), Duration::from_secs(30));
    }

    #[test]
    fn errors_render_without_exposing_connection_details() {
        let error = CollectorError::Connect("password authentication failed".to_string());
        let rendered = error.to_string();
        assert!(rendered.contains("password authentication failed"));
        assert!(!rendered.contains("postgresql://"));
    }

    #[test]
    fn stop_requested_is_false_for_an_empty_channel_and_true_for_a_closed_one() {
        let (tx, rx) = async_channel::bounded::<()>(1);
        assert!(!stop_requested(&rx));
        drop(tx);
        assert!(stop_requested(&rx));
    }

    #[test]
    fn stop_requested_is_true_when_a_stop_message_is_pending() {
        let (tx, rx) = async_channel::bounded::<()>(1);
        tx.try_send(()).unwrap();
        assert!(stop_requested(&rx));
    }

    #[test]
    fn dropping_the_handle_without_calling_stop_closes_the_stop_channel() {
        let (stop_tx, stop_rx) = async_channel::bounded::<()>(1);
        let (_event_tx, event_rx) = async_channel::bounded::<CollectorEvent>(1);
        let handle = CollectorHandle {
            events: event_rx,
            stop: stop_tx,
        };

        drop(handle);

        assert!(stop_rx.is_closed());
    }
}

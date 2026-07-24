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

use std::collections::HashMap;
use std::sync::Once;
use std::time::{Duration, Instant};

use rustls::RootCertStore;
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{error::SqlState, Client, NoTls, Socket};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::collector::queries::{
    count_sessions, map_database_counters, map_session, map_settings, ACTIVITY_SQL,
    DATABASE_SIZE_SQL, DATABASE_STATS_SQL, SETTINGS_SQL,
};
use crate::collector::rates::derive_rates;
use crate::collector::relations::{
    map_index_stats, map_table_stats, RelationsSample, INDEXES_SQL, TABLES_SQL,
};
use crate::collector::snapshot::{DatabaseCounters, ServerSettings, Snapshot};
use crate::collector::statements::{
    apply_deltas, counters_by_key, map_statement, StatementCounters, StatementKey,
    StatementsSample, STATEMENTS_SQL,
};
use crate::connection::params::{ConnectionParams, SslMode};
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

/// Emit a lifecycle event. Blocks only until the collector is asked to stop,
/// so a stalled consumer can never wedge the sampler somewhere `stop()`
/// cannot reach it. Returns false when we should shut down.
async fn emit(
    events: &async_channel::Sender<CollectorEvent>,
    stop: &async_channel::Receiver<()>,
    event: CollectorEvent,
) -> bool {
    tokio::select! {
        result = events.send(event) => result.is_ok(),
        _ = stop.recv() => false,
    }
}

/// Emit a sample, dropping it if the consumer is behind. Monitoring data has
/// no value once late, and `previous` still advances so rates stay accurate
/// across a dropped sample.
fn emit_sample(events: &async_channel::Sender<CollectorEvent>, snapshot: Box<Snapshot>) {
    let _ = events.try_send(CollectorEvent::Sample(snapshot));
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

/// How the collector is configured for one connection. A struct rather than
/// four positional arguments, since three of the four are durations or
/// limits that would be easy to transpose.
#[derive(Debug, Clone, Copy)]
pub struct CollectorConfig {
    pub interval: Duration,
    pub slow_interval: Duration,
    pub statements_limit: i64,
    pub relations_limit: i64,
}

/// True when this tick should also run the slow tier: always for the first
/// sample of a connection, so the heavy pages populate immediately, then
/// once `slow_interval` has elapsed.
pub fn is_slow_tick(last_slow: Option<Instant>, now: Instant, slow_interval: Duration) -> bool {
    match last_slow {
        None => true,
        Some(previous) => now.duration_since(previous) >= slow_interval,
    }
}

/// A slow-tier failure degrades one page rather than the connection. Only a
/// timeout or a lost connection is allowed to fail the whole sample; a query
/// error — insufficient privilege, an extension dropped mid-session, a
/// relation dropped between the catalogue read and the size call — is
/// captured into the snapshot for its page to render.
fn classify_slow<T>(
    result: Result<T, CollectorError>,
) -> Result<Result<T, CollectorError>, CollectorError> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(error) => match error {
            CollectorError::Timeout | CollectorError::LostConnection => Err(error),
            _ => Ok(Err(error)),
        },
    }
}

pub fn spawn(
    params: ConnectionParams,
    password: String,
    config: CollectorConfig,
) -> CollectorHandle {
    let (event_tx, event_rx) = async_channel::bounded(32);
    let (stop_tx, stop_rx) = async_channel::bounded(1);

    std::thread::Builder::new()
        .name("mcpg-collector".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build the collector runtime");
            runtime.block_on(run(params, password, config, event_tx, stop_rx));
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
    config: CollectorConfig,
    events: async_channel::Sender<CollectorEvent>,
    stop: async_channel::Receiver<()>,
) {
    let mut consecutive_failures = 0u32;

    loop {
        if stop_requested(&stop) {
            return;
        }

        if !emit(&events, &stop, CollectorEvent::Connecting).await {
            return;
        }

        // The connect-and-probe step is otherwise unbounded from `stop`'s
        // point of view: a server that completes the TCP handshake and then
        // never answers would leave the thread parked here indefinitely.
        let connect_result = tokio::select! {
            result = connect(&params, &password) => result,
            _ = stop.recv() => return,
        };

        match connect_result {
            Ok((client, info)) => {
                let statements_available = info.statements.is_available();
                if !emit(&events, &stop, CollectorEvent::Connected(info)).await {
                    return;
                }
                match sample_loop(&client, config, statements_available, &events, &stop).await {
                    Exit::Stopped => {
                        // We are already shutting down, so route this
                        // through the non-blocking path rather than emit(),
                        // which would see the pending stop and drop it.
                        let _ = events.try_send(CollectorEvent::Disconnected);
                        return;
                    }
                    Exit::Failed { had_success } => {
                        consecutive_failures = if had_success {
                            0
                        } else {
                            consecutive_failures.saturating_add(1)
                        };
                    }
                }
                if !emit(&events, &stop, CollectorEvent::Disconnected).await {
                    return;
                }
            }
            Err(e) => {
                if !emit(&events, &stop, CollectorEvent::Error(e)).await {
                    return;
                }
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

    // Only the connector differs between the TLS and non-TLS paths; the probe
    // and connection-driver setup are shared in `establish`. `to_config` has
    // already stamped the tokio-postgres `SslMode` onto `config`, so for
    // `Require` tokio-postgres itself rejects a server that refuses TLS, and
    // for `Prefer` it falls back to plaintext when the server has no TLS.
    match params.ssl_mode {
        SslMode::Disable => establish(&config, NoTls).await,
        SslMode::Prefer | SslMode::Require => {
            let connector = rustls_connector()?;
            establish(&config, connector).await
        }
    }
}

/// Shared connect-and-probe path, generic over the TLS connector so the two
/// arms of `connect` share the probe and the connection-driver spawn rather
/// than duplicating the sampling setup.
async fn establish<T>(
    config: &tokio_postgres::Config,
    tls: T,
) -> Result<(Client, ServerInfo), CollectorError>
where
    T: MakeTlsConnect<Socket>,
    T::Stream: Send + 'static,
{
    let (client, connection) = config
        .connect(tls)
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

/// Builds a rustls TLS connector trusting the platform's native root
/// certificates. Used for both `Prefer` and `Require`; tokio-postgres decides
/// from the `SslMode` on the `Config` whether TLS is mandatory or opportunistic.
fn rustls_connector() -> Result<MakeRustlsConnect, CollectorError> {
    install_crypto_provider();

    let mut roots = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    roots.add_parsable_certificates(native.certs);

    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    Ok(MakeRustlsConnect::new(client_config))
}

/// Installs a process-default rustls crypto provider exactly once. rustls 0.23
/// requires a process-default provider before `ClientConfig::builder()` will
/// work when more than one provider is compiled in; `aws_lc_rs` is rustls'
/// default and is always available in this build. The `Once` keeps this off
/// the global-static path, so it runs only when a TLS connection is first set
/// up rather than at unpredictable start-up time.
fn install_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        // A failure here means another provider is already installed, which is
        // fine: we only need *some* process-default provider to exist.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Samples serially: the next sample starts only once the previous one has
/// finished or timed out, so a slow server spreads samples out rather than
/// piling overlapping queries onto one connection.
async fn sample_loop(
    client: &Client,
    config: CollectorConfig,
    statements_available: bool,
    events: &async_channel::Sender<CollectorEvent>,
    stop: &async_channel::Receiver<()>,
) -> Exit {
    let mut previous: Option<(DatabaseCounters, Instant)> = None;
    let mut previous_statements: Option<(HashMap<StatementKey, StatementCounters>, Instant)> = None;
    let mut last_slow: Option<Instant> = None;
    let mut consecutive_failures = 0u32;
    let mut had_success = false;

    loop {
        if stop_requested(stop) {
            return Exit::Stopped;
        }

        let slow =
            is_slow_tick(last_slow, Instant::now(), config.slow_interval).then_some(SlowTier {
                statements_available,
                statements_limit: config.statements_limit,
                relations_limit: config.relations_limit,
                previous_statements: previous_statements
                    .as_ref()
                    .map(|(counters, at)| (counters, *at)),
            });

        match sample(client, previous, slow).await {
            Ok(snapshot) => {
                consecutive_failures = 0;
                had_success = true;
                previous = Some((snapshot.totals, snapshot.taken_at));
                if let Some(Ok(sample)) = snapshot.statements.as_ref() {
                    previous_statements =
                        Some((counters_by_key(&sample.statements), snapshot.taken_at));
                }
                // Mark the slow tier as run whether or not its queries
                // succeeded: retrying a failing view every two seconds would
                // turn one broken page into a load problem.
                if snapshot.statements.is_some() || snapshot.relations.is_some() {
                    last_slow = Some(snapshot.taken_at);
                }
                emit_sample(events, Box::new(snapshot));
            }
            Err(e) => {
                consecutive_failures += 1;
                if !emit(events, stop, CollectorEvent::Error(e)).await {
                    return Exit::Stopped;
                }
                if consecutive_failures >= FAILURES_BEFORE_DISCONNECT {
                    return Exit::Failed { had_success };
                }
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(config.interval) => {}
            _ = stop.recv() => return Exit::Stopped,
        }
    }
}

/// What the slow tier needs for one run. Present only on a slow tick.
struct SlowTier<'a> {
    statements_available: bool,
    statements_limit: i64,
    relations_limit: i64,
    previous_statements: Option<(&'a HashMap<StatementKey, StatementCounters>, Instant)>,
}

async fn sample(
    client: &Client,
    previous: Option<(DatabaseCounters, Instant)>,
    slow: Option<SlowTier<'_>>,
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

    let (statements, relations) = match slow {
        None => (None, None),
        Some(slow) => {
            let statements = if slow.statements_available {
                Some(classify_slow(
                    sample_statements(client, &slow, taken_at).await,
                )?)
            } else {
                // The page learns from ServerInfo that the extension is
                // missing; there is nothing to report here.
                None
            };
            let relations = Some(classify_slow(
                sample_relations(client, slow.relations_limit).await,
            )?);
            (statements, relations)
        }
    };

    Ok(Snapshot {
        taken_at,
        totals,
        rates,
        connected_database_size_bytes,
        session_counts,
        sessions,
        settings,
        statements,
        relations,
    })
}

async fn sample_statements(
    client: &Client,
    slow: &SlowTier<'_>,
    taken_at: Instant,
) -> Result<StatementsSample, CollectorError> {
    let rows = client
        .query(STATEMENTS_SQL, &[&slow.statements_limit])
        .await
        .map_err(map_query_error)?;
    let mut statements: Vec<_> = rows.iter().map(map_statement).collect();

    if let Some((previous, previous_at)) = slow.previous_statements {
        apply_deltas(
            &mut statements,
            previous,
            taken_at.duration_since(previous_at),
        );
    }

    Ok(StatementsSample { statements })
}

async fn sample_relations(client: &Client, limit: i64) -> Result<RelationsSample, CollectorError> {
    let rows = client
        .query(TABLES_SQL, &[&limit])
        .await
        .map_err(map_query_error)?;
    let tables = rows.iter().map(map_table_stats).collect();

    let rows = client
        .query(INDEXES_SQL, &[&limit])
        .await
        .map_err(map_query_error)?;
    let indexes = rows.iter().map(map_index_stats).collect();

    Ok(RelationsSample { tables, indexes })
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
    fn dropping_the_handle_sends_a_stop_signal_before_closing_the_channel() {
        // Asserting only `is_closed()` here would pass even with the `Drop`
        // impl deleted, since dropping the struct also drops the `stop`
        // sender field on its own. Assert the buffered stop message that
        // only the `Drop` impl's call to `self.stop()` can have produced.
        let (stop_tx, stop_rx) = async_channel::bounded::<()>(1);
        let (_event_tx, event_rx) = async_channel::bounded::<CollectorEvent>(1);
        let handle = CollectorHandle {
            events: event_rx,
            stop: stop_tx,
        };

        drop(handle);

        assert!(matches!(stop_rx.try_recv(), Ok(())));
        assert!(stop_rx.is_closed());
    }

    #[test]
    fn the_first_sample_of_a_connection_always_runs_the_slow_tier() {
        // Otherwise both heavy pages sit blank for the whole slow interval
        // after connecting.
        let now = Instant::now();
        assert!(is_slow_tick(None, now, Duration::from_secs(10)));
    }

    #[test]
    fn the_slow_tier_waits_for_its_interval() {
        let now = Instant::now();
        let recent = now - Duration::from_secs(3);
        assert!(!is_slow_tick(Some(recent), now, Duration::from_secs(10)));
    }

    #[test]
    fn the_slow_tier_runs_once_the_interval_has_elapsed() {
        let now = Instant::now();
        let stale = now - Duration::from_secs(11);
        assert!(is_slow_tick(Some(stale), now, Duration::from_secs(10)));
    }

    #[test]
    fn a_slow_tier_query_error_degrades_one_page_not_the_connection() {
        // A permission error on pg_stat_statements must not count towards
        // the three-strike disconnect.
        let classified = classify_slow(Err::<(), _>(CollectorError::Query(
            "permission denied for view pg_stat_statements".to_string(),
        )));
        assert!(matches!(classified, Ok(Err(CollectorError::Query(_)))));
    }

    #[test]
    fn a_slow_tier_timeout_still_fails_the_sample() {
        assert!(matches!(
            classify_slow(Err::<(), _>(CollectorError::Timeout)),
            Err(CollectorError::Timeout)
        ));
    }

    #[test]
    fn a_slow_tier_connection_loss_still_fails_the_sample() {
        assert!(matches!(
            classify_slow(Err::<(), _>(CollectorError::LostConnection)),
            Err(CollectorError::LostConnection)
        ));
    }
}

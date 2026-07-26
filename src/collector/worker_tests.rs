/* worker_tests.rs
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

use super::*;

#[test]
fn a_pgconsole_write_failure_disables_history_without_failing_the_sample() {
    // A mid-session INSERT failure (schema dropped, privilege revoked) is
    // a property of the store, not the connection: history goes Off and
    // the sample still succeeds. Classified exactly like a slow-tier
    // Query error.
    let outcome = classify_history_error(CollectorError::Query("permission denied".into()));
    assert!(matches!(outcome, HistoryOutcome::Disable));
}

#[test]
fn a_pgconsole_write_timeout_skips_the_write_without_failing_the_sample() {
    assert!(matches!(
        classify_history_error(CollectorError::Timeout),
        HistoryOutcome::SkipWrite
    ));
}

#[test]
fn a_pgconsole_write_connection_loss_skips_the_write_without_failing_the_sample() {
    assert!(matches!(
        classify_history_error(CollectorError::LostConnection),
        HistoryOutcome::SkipWrite
    ));
}

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
    let (command_tx, _command_rx) = async_channel::bounded::<Action>(1);
    let handle = CollectorHandle {
        events: event_rx,
        stop: stop_tx,
        commands: command_tx,
    };

    drop(handle);

    assert!(matches!(stop_rx.try_recv(), Ok(())));
    assert!(stop_rx.is_closed());
}

#[test]
fn the_first_sample_of_a_connection_runs_the_fast_tier_only() {
    // Sampling is serial, so waiting for the slow tier here would leave
    // the Overview blank until the three heavy queries returned.
    let now = Instant::now();
    assert!(!is_slow_tick(None, now, Duration::from_secs(10), true));
}

#[test]
fn the_slow_tier_fires_on_the_tick_after_the_first_sample() {
    // The heavy pages still populate promptly — about one fast interval
    // after connecting — rather than waiting a full slow interval.
    let now = Instant::now();
    assert!(is_slow_tick(None, now, Duration::from_secs(10), false));
}

#[test]
fn the_slow_tier_waits_for_its_interval() {
    let now = Instant::now();
    let recent = now - Duration::from_secs(3);
    assert!(!is_slow_tick(
        Some(recent),
        now,
        Duration::from_secs(10),
        false
    ));
}

#[test]
fn the_slow_tier_runs_once_the_interval_has_elapsed() {
    let now = Instant::now();
    let stale = now - Duration::from_secs(11);
    assert!(is_slow_tick(
        Some(stale),
        now,
        Duration::from_secs(10),
        false
    ));
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

#[test]
fn an_attempted_slow_tick_advances_last_slow_so_the_next_fast_tick_is_refused() {
    // Advancing `last_slow` at the moment the tick is attempted — rather
    // than after the sample returns — is exactly the fix this guards: a
    // slow-tier timeout produces no snapshot, so if the assignment lived
    // in the `Ok` arm instead, `last_slow` would stay unset and this
    // would attempt again on the very next fast tick, two seconds later.
    let mut last_slow = None;
    let attempted_at = Instant::now();

    assert!(take_slow_tick(
        &mut last_slow,
        attempted_at,
        Duration::from_secs(10),
        false
    ));
    assert_eq!(last_slow, Some(attempted_at));

    let one_fast_tick_later = attempted_at + Duration::from_secs(2);
    assert!(!take_slow_tick(
        &mut last_slow,
        one_fast_tick_later,
        Duration::from_secs(10),
        false
    ));
    assert_eq!(last_slow, Some(attempted_at));
}

#[test]
fn the_first_sample_neither_attempts_nor_advances_last_slow() {
    let mut last_slow = None;
    let now = Instant::now();

    assert!(!take_slow_tick(
        &mut last_slow,
        now,
        Duration::from_secs(10),
        true
    ));
    assert_eq!(last_slow, None);
}

#[test]
fn a_tick_inside_the_interval_neither_attempts_nor_advances_last_slow() {
    let now = Instant::now();
    let recent = now - Duration::from_secs(3);
    let mut last_slow = Some(recent);

    assert!(!take_slow_tick(
        &mut last_slow,
        now,
        Duration::from_secs(10),
        false
    ));
    assert_eq!(last_slow, Some(recent));
}

#[test]
fn a_full_command_channel_refuses_rather_than_queues() {
    // Destructive actions must never pile up behind a wedged collector: a
    // terminate the user gave up on and re-clicked five times should not
    // arrive five times a minute later.
    let (tx, rx) = async_channel::bounded::<Action>(2);
    assert!(offer_command(&tx, Action::ReloadConfig));
    assert!(offer_command(&tx, Action::ReloadConfig));
    assert!(!offer_command(&tx, Action::ReloadConfig));
    drop(rx);
    assert!(!offer_command(&tx, Action::ReloadConfig));
}

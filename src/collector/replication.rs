/* collector/replication.rs
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

use tokio_postgres::{Client, Row};

/// A standby connected to this primary, from `pg_stat_replication`.
#[derive(Debug, Clone, PartialEq)]
pub struct Standby {
    pub pid: i32,
    pub application_name: Option<String>,
    pub client_addr: Option<String>,
    pub state: Option<String>,
    pub sync_state: Option<String>,
    pub write_lag_secs: Option<f64>,
    pub flush_lag_secs: Option<f64>,
    pub replay_lag_secs: Option<f64>,
    /// Bytes between `sent_lsn` and `replay_lsn`: how much write-ahead log the
    /// standby still has to apply. Seconds answer "how stale is this replica";
    /// bytes answer "how much work is catching up". Neither substitutes for
    /// the other.
    pub replay_lag_bytes: Option<i64>,
}

/// A replication slot. `inactive_since_secs` is `None` before PostgreSQL 17,
/// which is a different thing from "active right now" and renders
/// differently.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub slot_name: String,
    pub slot_type: Option<String>,
    pub plugin: Option<String>,
    pub database: Option<String>,
    pub active: bool,
    pub wal_status: Option<String>,
    pub safe_wal_size: Option<i64>,
    pub inactive_since_secs: Option<f64>,
    pub conflicting: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub subname: String,
    pub pid: Option<i32>,
    pub worker_type: Option<String>,
    pub latest_end_lag_secs: Option<f64>,
    pub apply_error_count: Option<i64>,
    pub sync_error_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Publication {
    pub pubname: String,
    pub all_tables: bool,
}

/// Upstream state when this server is itself a standby.
#[derive(Debug, Clone, PartialEq)]
pub struct WalReceiver {
    pub status: Option<String>,
    pub sender_host: Option<String>,
    pub received_lsn: Option<String>,
    pub replayed_lsn: Option<String>,
    pub replay_delay_secs: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct ReplicationSample {
    pub in_recovery: bool,
    pub standbys: Vec<Standby>,
    pub receiver: Option<WalReceiver>,
    pub slots: Vec<Slot>,
    pub subscriptions: Vec<Subscription>,
    pub publications: Vec<Publication>,
}

/// Identical columns on 14 through 18, so no branch is needed. The byte
/// distance is computed server-side: pg_lsn arithmetic has no client-side
/// equivalent worth writing.
pub const STANDBYS_SQL: &str = "\
SELECT pid                                                 AS pid,
       application_name::text                              AS application_name,
       client_addr::text                                   AS client_addr,
       state::text                                         AS state,
       sync_state::text                                    AS sync_state,
       EXTRACT(EPOCH FROM write_lag)::float8               AS write_lag_secs,
       EXTRACT(EPOCH FROM flush_lag)::float8               AS flush_lag_secs,
       EXTRACT(EPOCH FROM replay_lag)::float8              AS replay_lag_secs,
       (sent_lsn - replay_lsn)::bigint                     AS replay_lag_bytes
FROM pg_stat_replication";

/// Only ever returns a row on a standby.
pub const RECEIVER_SQL: &str = "\
SELECT status::text                                        AS status,
       sender_host::text                                   AS sender_host,
       latest_end_lsn::text                                AS received_lsn,
       pg_last_wal_replay_lsn()::text                      AS replayed_lsn,
       EXTRACT(EPOCH FROM (now() - pg_last_xact_replay_timestamp()))::float8
                                                           AS replay_delay_secs
FROM pg_stat_wal_receiver";

/// Publications belong to the connected database only — `pg_publication` is
/// not a shared catalogue, unlike `pg_subscription` — which the page states
/// rather than implying the server has none.
pub const PUBLICATIONS_SQL: &str = "\
SELECT pubname::text AS pubname, puballtables AS all_tables
FROM pg_publication
ORDER BY pubname";

pub const IN_RECOVERY_SQL: &str = "SELECT pg_is_in_recovery() AS in_recovery";

/// Slot columns arrive across three releases, so the query is built rather
/// than branched wholesale: 16 adds `conflicting`, 17 adds `inactive_since`.
/// Asking PostgreSQL 14 for either is an error, not a NULL, so the substituted
/// literals keep the row shape identical on every version and the mapper needs
/// no branch of its own.
pub fn slots_sql(version_num: i32) -> String {
    let conflicting = if version_num >= 160000 {
        "s.conflicting"
    } else {
        "NULL::boolean"
    };
    let inactive_since = if version_num >= 170000 {
        "EXTRACT(EPOCH FROM (now() - s.inactive_since))::float8"
    } else {
        "NULL::float8"
    };

    format!(
        "SELECT s.slot_name          AS slot_name,
       s.slot_type::text            AS slot_type,
       s.plugin::text               AS plugin,
       s.database::text             AS database,
       s.active                     AS active,
       s.wal_status::text           AS wal_status,
       s.safe_wal_size              AS safe_wal_size,
       {inactive_since}             AS inactive_since_secs,
       {conflicting}                AS conflicting
FROM pg_replication_slots s"
    )
}

/// `pg_stat_subscription_stats` is 15 and later; `worker_type` is 17 and
/// later. As with the slots, absent columns become substituted literals so
/// every version returns the same shape.
pub fn subscriptions_sql(version_num: i32) -> String {
    let worker_type = if version_num >= 170000 {
        "s.worker_type::text"
    } else {
        "NULL::text"
    };
    let (errors, join) = if version_num >= 150000 {
        (
            "st.apply_error_count            AS apply_error_count,\n       st.sync_error_count             AS sync_error_count",
            "LEFT JOIN pg_stat_subscription_stats st ON st.subid = s.subid",
        )
    } else {
        (
            "NULL::bigint                    AS apply_error_count,\n       NULL::bigint                    AS sync_error_count",
            "",
        )
    };

    format!(
        "SELECT s.subname::text      AS subname,
       s.pid                        AS pid,
       {worker_type}                AS worker_type,
       EXTRACT(EPOCH FROM (now() - s.latest_end_time))::float8
                                    AS latest_end_lag_secs,
       {errors}
FROM pg_stat_subscription s
{join}"
    )
}

/// Inactive slots first: a slot with no consumer retains write-ahead log until
/// the disk fills, which is the one thing on this page that can take a server
/// down by itself. Ties break by name so the order is stable between samples.
pub fn sort_slots(slots: &mut [Slot]) {
    slots.sort_by(|a, b| {
        a.active
            .cmp(&b.active)
            .then_with(|| a.slot_name.cmp(&b.slot_name))
    });
}

pub fn map_standby(row: &Row) -> Standby {
    Standby {
        pid: row.get("pid"),
        application_name: row.get("application_name"),
        client_addr: row.get("client_addr"),
        state: row.get("state"),
        sync_state: row.get("sync_state"),
        write_lag_secs: row.get("write_lag_secs"),
        flush_lag_secs: row.get("flush_lag_secs"),
        replay_lag_secs: row.get("replay_lag_secs"),
        replay_lag_bytes: row.get("replay_lag_bytes"),
    }
}

pub fn map_slot(row: &Row) -> Slot {
    Slot {
        slot_name: row.get("slot_name"),
        slot_type: row.get("slot_type"),
        plugin: row.get("plugin"),
        database: row.get("database"),
        active: row.get("active"),
        wal_status: row.get("wal_status"),
        safe_wal_size: row.get("safe_wal_size"),
        inactive_since_secs: row.get("inactive_since_secs"),
        conflicting: row.get("conflicting"),
    }
}

pub fn map_subscription(row: &Row) -> Subscription {
    Subscription {
        subname: row.get("subname"),
        pid: row.get("pid"),
        worker_type: row.get("worker_type"),
        latest_end_lag_secs: row.get("latest_end_lag_secs"),
        apply_error_count: row.get("apply_error_count"),
        sync_error_count: row.get("sync_error_count"),
    }
}

pub fn map_publication(row: &Row) -> Publication {
    Publication {
        pubname: row.get("pubname"),
        all_tables: row.get("all_tables"),
    }
}

pub fn map_receiver(row: &Row) -> WalReceiver {
    WalReceiver {
        status: row.get("status"),
        sender_host: row.get("sender_host"),
        received_lsn: row.get("received_lsn"),
        replayed_lsn: row.get("replayed_lsn"),
        replay_delay_secs: row.get("replay_delay_secs"),
    }
}

/// One slow-tier pass over every replication source. Nothing here fails just
/// because a section is empty, which is the normal case on most servers.
pub async fn sample_replication(
    client: &Client,
    version_num: i32,
) -> Result<ReplicationSample, tokio_postgres::Error> {
    let in_recovery: bool = client
        .query_one(IN_RECOVERY_SQL, &[])
        .await?
        .get("in_recovery");

    // A standby has no standbys of its own to report, and a primary has no
    // receiver. Skipping the query that cannot apply keeps a slow tick short.
    let standbys = if in_recovery {
        Vec::new()
    } else {
        client
            .query(STANDBYS_SQL, &[])
            .await?
            .iter()
            .map(map_standby)
            .collect()
    };

    let receiver = if in_recovery {
        client
            .query_opt(RECEIVER_SQL, &[])
            .await?
            .as_ref()
            .map(map_receiver)
    } else {
        None
    };

    let mut slots: Vec<Slot> = client
        .query(slots_sql(version_num).as_str(), &[])
        .await?
        .iter()
        .map(map_slot)
        .collect();
    sort_slots(&mut slots);

    let subscriptions = client
        .query(subscriptions_sql(version_num).as_str(), &[])
        .await?
        .iter()
        .map(map_subscription)
        .collect();

    let publications = client
        .query(PUBLICATIONS_SQL, &[])
        .await?
        .iter()
        .map(map_publication)
        .collect();

    Ok(ReplicationSample {
        in_recovery,
        standbys,
        receiver,
        slots,
        subscriptions,
        publications,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(name: &str, active: bool) -> Slot {
        Slot {
            slot_name: name.to_string(),
            slot_type: Some("physical".to_string()),
            plugin: None,
            database: None,
            active,
            wal_status: Some("reserved".to_string()),
            safe_wal_size: None,
            inactive_since_secs: None,
            conflicting: None,
        }
    }

    #[test]
    fn postgres_14_asks_for_no_column_it_does_not_have() {
        let sql = slots_sql(140000);
        assert!(!sql.contains("s.inactive_since"));
        assert!(!sql.contains("s.conflicting"));
        assert!(sql.contains("wal_status"));
    }

    #[test]
    fn postgres_16_asks_for_conflicting_but_not_inactive_since() {
        let sql = slots_sql(160000);
        assert!(sql.contains("s.conflicting"));
        assert!(!sql.contains("s.inactive_since"));
    }

    #[test]
    fn postgres_17_asks_for_inactive_since() {
        let sql = slots_sql(170000);
        assert!(sql.contains("s.inactive_since"));
        assert!(sql.contains("s.conflicting"));
    }

    #[test]
    fn subscription_statistics_are_only_requested_from_15_onwards() {
        assert!(!subscriptions_sql(140000).contains("pg_stat_subscription_stats"));
        assert!(subscriptions_sql(150000).contains("pg_stat_subscription_stats"));
    }

    #[test]
    fn worker_type_is_only_requested_from_17_onwards() {
        assert!(!subscriptions_sql(160000).contains("s.worker_type"));
        assert!(subscriptions_sql(170000).contains("s.worker_type"));
    }

    #[test]
    fn inactive_slots_sort_above_active_ones() {
        let mut slots = vec![slot("live", true), slot("abandoned", false)];
        sort_slots(&mut slots);
        assert_eq!(slots[0].slot_name, "abandoned");
    }

    #[test]
    fn slots_of_equal_activity_sort_by_name() {
        let mut slots = vec![slot("b", true), slot("a", true)];
        sort_slots(&mut slots);
        assert_eq!(slots[0].slot_name, "a");
    }
}

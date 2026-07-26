/* relations.rs
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

use tokio_postgres::Row;

/// Whether the connected role may maintain a given table is a property of the
/// row, not of the connection: a table owner may VACUUM their own tables
/// holding no server-wide privilege at all.
///
/// This is the first query in the project that genuinely branches on server
/// version, which is what the parent spec §5 deferred `sql_for(version)` for.
/// `has_table_privilege(..., 'MAINTAIN')` raises *unrecognized privilege type*
/// before PostgreSQL 17, so 14 through 16 fall back to an ownership check —
/// which also covers superusers, who are members of every role.
const MAINTAIN_PRIVILEGE_VERSION: i32 = 170000;

/// Table statistics for the connected database. `pg_stat_user_tables` is
/// per-database; there is no server-wide equivalent.
///
/// `idx_scan` is NULL for a table with no indexes, and COALESCE to zero is
/// correct: a table with no indexes has had no index scans. `GREATEST` over
/// the two vacuum timestamps is NULL only when neither route has ever
/// vacuumed the table, which is itself the interesting answer.
pub fn tables_sql(version_num: i32) -> String {
    let can_maintain = if version_num >= MAINTAIN_PRIVILEGE_VERSION {
        "has_table_privilege(current_user, c.oid, 'MAINTAIN')"
    } else {
        "pg_has_role(current_user, c.relowner, 'MEMBER')"
    };

    format!(
        "\
SELECT t.schemaname::text AS schema_name,
       t.relname::text    AS table_name,
       t.seq_scan,
       t.seq_tup_read,
       COALESCE(t.idx_scan, 0)      AS idx_scan,
       COALESCE(t.idx_tup_fetch, 0) AS idx_tup_fetch,
       t.n_tup_ins,
       t.n_tup_upd,
       t.n_tup_del,
       t.n_live_tup,
       t.n_dead_tup,
       EXTRACT(EPOCH FROM (now() - GREATEST(t.last_vacuum, t.last_autovacuum)))::float8
         AS secs_since_vacuum,
       pg_total_relation_size(t.relid)::int8 AS total_bytes,
       {can_maintain} AS can_maintain
  FROM pg_stat_user_tables t
  JOIN pg_class c ON c.oid = t.relid
 ORDER BY total_bytes DESC
 LIMIT $1"
    )
}

/// Index statistics joined to `pg_index` for the constraint flags. Those
/// flags are what stop every primary key being reported as an unused index.
pub const INDEXES_SQL: &str = "\
SELECT i.schemaname::text   AS schema_name,
       i.relname::text      AS table_name,
       i.indexrelname::text AS index_name,
       COALESCE(i.idx_scan, 0)      AS idx_scan,
       COALESCE(i.idx_tup_read, 0)  AS idx_tup_read,
       COALESCE(i.idx_tup_fetch, 0) AS idx_tup_fetch,
       pg_relation_size(i.indexrelid)::int8 AS bytes,
       x.indisprimary AS is_primary,
       x.indisunique  AS is_unique,
       x.indisvalid   AS is_valid
  FROM pg_stat_user_indexes i
  JOIN pg_index x ON x.indexrelid = i.indexrelid
 ORDER BY bytes DESC
 LIMIT $1";

#[derive(Debug, Clone, PartialEq)]
pub struct TableStats {
    pub schema_name: String,
    pub table_name: String,
    pub seq_scan: i64,
    pub seq_tup_read: i64,
    pub idx_scan: i64,
    pub idx_tup_fetch: i64,
    pub n_tup_ins: i64,
    pub n_tup_upd: i64,
    pub n_tup_del: i64,
    pub n_live_tup: i64,
    pub n_dead_tup: i64,
    /// `None` when neither a manual nor an automatic vacuum has ever run.
    pub secs_since_vacuum: Option<f64>,
    pub total_bytes: i64,
    /// True when the connected role may maintain this specific table —
    /// through ownership, a granted MAINTAIN, or superuser.
    pub can_maintain: bool,
}

impl TableStats {
    /// Whether maintenance may run on this table, combining the row's own
    /// answer with the connection's server-wide `pg_maintain` membership.
    pub fn may_maintain(&self, server_wide: bool) -> bool {
        self.can_maintain || server_wide
    }

    /// Dead tuples as a fraction of all tuples. `None` for a table with no
    /// tuples at all — this is the component of bloat a statistic can see,
    /// which is why the column is named for dead tuples and not for bloat.
    pub fn dead_tuple_ratio(&self) -> Option<f64> {
        let total = self.n_live_tup + self.n_dead_tup;
        if total > 0 {
            Some(self.n_dead_tup as f64 / total as f64)
        } else {
            None
        }
    }

    /// Sequential scans as a fraction of all scans. `None` when the table has
    /// never been scanned by either route.
    pub fn sequential_scan_ratio(&self) -> Option<f64> {
        let total = self.seq_scan + self.idx_scan;
        if total > 0 {
            Some(self.seq_scan as f64 / total as f64)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexStats {
    pub schema_name: String,
    pub table_name: String,
    pub index_name: String,
    pub idx_scan: i64,
    pub idx_tup_read: i64,
    pub idx_tup_fetch: i64,
    pub bytes: i64,
    pub is_primary: bool,
    pub is_unique: bool,
    pub is_valid: bool,
}

impl IndexStats {
    /// Zero scans and backing no constraint. The flag checks are the point:
    /// an unscanned primary key is not a removal candidate, and including
    /// those rows drowns the real answers.
    pub fn is_unused(&self) -> bool {
        self.idx_scan == 0 && !self.is_primary && !self.is_unique
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelationsSample {
    pub tables: Vec<TableStats>,
    pub indexes: Vec<IndexStats>,
}

pub fn map_table_stats(row: &Row) -> TableStats {
    TableStats {
        schema_name: row.get("schema_name"),
        table_name: row.get("table_name"),
        seq_scan: row.get("seq_scan"),
        seq_tup_read: row.get("seq_tup_read"),
        idx_scan: row.get("idx_scan"),
        idx_tup_fetch: row.get("idx_tup_fetch"),
        n_tup_ins: row.get("n_tup_ins"),
        n_tup_upd: row.get("n_tup_upd"),
        n_tup_del: row.get("n_tup_del"),
        n_live_tup: row.get("n_live_tup"),
        n_dead_tup: row.get("n_dead_tup"),
        secs_since_vacuum: row.get("secs_since_vacuum"),
        total_bytes: row.get("total_bytes"),
        can_maintain: row.get("can_maintain"),
    }
}

pub fn map_index_stats(row: &Row) -> IndexStats {
    IndexStats {
        schema_name: row.get("schema_name"),
        table_name: row.get("table_name"),
        index_name: row.get("index_name"),
        idx_scan: row.get("idx_scan"),
        idx_tup_read: row.get("idx_tup_read"),
        idx_tup_fetch: row.get("idx_tup_fetch"),
        bytes: row.get("bytes"),
        is_primary: row.get("is_primary"),
        is_unique: row.get("is_unique"),
        is_valid: row.get("is_valid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(live: i64, dead: i64, seq: i64, idx: i64) -> TableStats {
        TableStats {
            schema_name: "public".to_string(),
            table_name: "orders".to_string(),
            seq_scan: seq,
            seq_tup_read: 0,
            idx_scan: idx,
            idx_tup_fetch: 0,
            n_tup_ins: 0,
            n_tup_upd: 0,
            n_tup_del: 0,
            n_live_tup: live,
            n_dead_tup: dead,
            secs_since_vacuum: None,
            total_bytes: 0,
            can_maintain: false,
        }
    }

    #[test]
    fn dead_tuple_ratio_is_dead_over_the_total() {
        assert_eq!(table(750, 250, 0, 0).dead_tuple_ratio(), Some(0.25));
        assert_eq!(table(0, 100, 0, 0).dead_tuple_ratio(), Some(1.0));
    }

    #[test]
    fn an_empty_table_has_no_dead_tuple_ratio() {
        // Reporting 0% for a table with no tuples would claim a measurement
        // that was never taken, the same lie as a zero cache hit ratio.
        assert_eq!(table(0, 0, 0, 0).dead_tuple_ratio(), None);
    }

    #[test]
    fn sequential_scan_ratio_is_seq_over_all_scans() {
        assert_eq!(table(0, 0, 30, 70).sequential_scan_ratio(), Some(0.3));
        assert_eq!(table(0, 0, 10, 0).sequential_scan_ratio(), Some(1.0));
    }

    #[test]
    fn a_never_scanned_table_has_no_scan_ratio() {
        assert_eq!(table(0, 0, 0, 0).sequential_scan_ratio(), None);
    }

    fn index(scans: i64, primary: bool, unique: bool) -> IndexStats {
        IndexStats {
            schema_name: "public".to_string(),
            table_name: "orders".to_string(),
            index_name: "orders_pkey".to_string(),
            idx_scan: scans,
            idx_tup_read: 0,
            idx_tup_fetch: 0,
            bytes: 0,
            is_primary: primary,
            is_unique: unique,
            is_valid: true,
        }
    }

    #[test]
    fn an_index_with_no_scans_and_no_constraint_is_unused() {
        assert!(index(0, false, false).is_unused());
    }

    #[test]
    fn a_scanned_index_is_not_unused() {
        assert!(!index(1, false, false).is_unused());
    }

    #[test]
    fn an_unscanned_primary_key_is_not_reported_as_unused() {
        // Including these makes the report useless: a primary key is not a
        // removal candidate however few scans it has served.
        assert!(!index(0, true, true).is_unused());
    }

    #[test]
    fn an_unscanned_unique_index_is_not_reported_as_unused() {
        assert!(!index(0, false, true).is_unused());
    }

    #[test]
    fn postgres_17_and_later_ask_for_the_maintain_privilege() {
        for version in [170000, 180004] {
            let sql = tables_sql(version);
            assert!(
                sql.contains("has_table_privilege(current_user, c.oid, 'MAINTAIN')"),
                "{version} should use the MAINTAIN privilege"
            );
        }
    }

    #[test]
    fn postgres_16_and_earlier_fall_back_to_ownership() {
        // has_table_privilege raises "unrecognized privilege type" for
        // MAINTAIN before 17, and a raising slow-tier query costs the page.
        for version in [140011, 160002] {
            let sql = tables_sql(version);
            assert!(
                sql.contains("pg_has_role(current_user, c.relowner, 'MEMBER')"),
                "{version} should fall back to an ownership check"
            );
            assert!(
                !sql.contains("'MAINTAIN'"),
                "{version} must never mention a privilege it cannot parse"
            );
        }
    }

    #[test]
    fn every_version_still_selects_the_same_columns() {
        for version in [140011, 180004] {
            let sql = tables_sql(version);
            assert!(sql.contains("AS can_maintain"));
            assert!(sql.contains("pg_total_relation_size"));
            assert!(sql.ends_with("LIMIT $1"));
        }
    }

    #[test]
    fn a_server_wide_privilege_covers_a_table_the_role_does_not_own() {
        let table = table_stats_with(false);
        assert!(table.may_maintain(true));
        assert!(!table.may_maintain(false));
    }

    #[test]
    fn an_owned_table_is_maintainable_without_any_server_privilege() {
        // The common case for an application role: it owns its own tables and
        // holds nothing else. Greying the button out here is the failure this
        // whole column exists to avoid.
        let table = table_stats_with(true);
        assert!(table.may_maintain(false));
    }

    fn table_stats_with(can_maintain: bool) -> TableStats {
        TableStats {
            schema_name: "public".to_string(),
            table_name: "orders".to_string(),
            seq_scan: 0,
            seq_tup_read: 0,
            idx_scan: 0,
            idx_tup_fetch: 0,
            n_tup_ins: 0,
            n_tup_upd: 0,
            n_tup_del: 0,
            n_live_tup: 0,
            n_dead_tup: 0,
            secs_since_vacuum: None,
            total_bytes: 0,
            can_maintain,
        }
    }
}

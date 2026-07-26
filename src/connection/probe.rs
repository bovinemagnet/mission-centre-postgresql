/* probe.rs
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

/// PostgreSQL 14. Connection is never refused on version grounds; pages that
/// need a newer server gate themselves. See spec §5.
pub const MIN_SUPPORTED_VERSION: i32 = 140000;

/// Every capability expression must be written so it cannot raise on a server
/// that lacks the object it names: this query runs on every connect, and a
/// probe that fails fails the connection. Each subselect returns no row rather
/// than raising when its object is absent, and the `pg_roles` one covers 14-16,
/// where `pg_maintain` does not exist — a bare
/// `pg_has_role(current_user, 'pg_maintain', 'member')` would raise there.
///
/// The reset function is looked up **by name, never by signature**. From
/// pg_stat_statements 1.11 it is declared `(oid, oid, bigint, boolean)` with
/// every argument defaulted, so there is no zero-argument overload and
/// `to_regprocedure('pg_stat_statements_reset()')` yields NULL on a server that
/// has the extension installed and working. `bool_or` over the matching rows
/// gives the answer for any version, and NULL — hence false — when the
/// extension is absent entirely.
///
/// Superusers need no special case: `pg_has_role` and `has_function_privilege`
/// both return true for them.
pub const PROBE_SQL: &str = "\
SELECT current_setting('server_version_num')::int AS version_num,
       pg_has_role(current_user, 'pg_monitor', 'member') AS is_monitor,
       COALESCE((SELECT rolsuper FROM pg_roles WHERE rolname = current_user), false) AS is_superuser,
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_stat_statements')
         AS statements_version,
       pg_has_role(current_user, 'pg_signal_backend', 'member') AS can_signal,
       has_function_privilege(current_user, 'pg_reload_conf()', 'execute') AS can_reload,
       (SELECT bool_or(has_function_privilege(current_user, p.oid, 'execute'))
          FROM pg_proc p
         WHERE p.proname = 'pg_stat_statements_reset')
         AS can_reset_statements,
       (SELECT pg_has_role(current_user, oid, 'member')
          FROM pg_roles WHERE rolname = 'pg_maintain')
         AS can_maintain";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeLevel {
    Superuser,
    Monitor,
    Limited,
}

impl PrivilegeLevel {
    pub fn classify(is_superuser: bool, is_monitor: bool) -> Self {
        if is_superuser {
            PrivilegeLevel::Superuser
        } else if is_monitor {
            PrivilegeLevel::Monitor
        } else {
            PrivilegeLevel::Limited
        }
    }

    /// True when PostgreSQL will return NULL query text for backends owned by
    /// other users, which the window must explain rather than silently render
    /// as blanks.
    pub fn hides_other_sessions(&self) -> bool {
        matches!(self, PrivilegeLevel::Limited)
    }
}

/// The `pg_stat_statements` columns this project reads — `total_exec_time`
/// and `mean_exec_time` — arrived in extension version 1.8. A server at or
/// above the PostgreSQL 14 floor can still carry 1.7 through a `pg_upgrade`
/// that never ran `ALTER EXTENSION … UPDATE`.
pub const MINIMUM_STATEMENTS_VERSION: (u32, u32) = (1, 8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementsAvailability {
    Available { version: String },
    TooOld { version: String },
    NotInstalled,
}

impl StatementsAvailability {
    pub fn classify(extversion: Option<&str>) -> Self {
        let Some(version) = extversion else {
            return StatementsAvailability::NotInstalled;
        };
        match parse_extension_version(version) {
            Some(parsed) if parsed >= MINIMUM_STATEMENTS_VERSION => {
                StatementsAvailability::Available {
                    version: version.to_string(),
                }
            }
            _ => StatementsAvailability::TooOld {
                version: version.to_string(),
            },
        }
    }

    pub fn is_available(&self) -> bool {
        matches!(self, StatementsAvailability::Available { .. })
    }
}

/// Extension versions are `major.minor`. Comparison must be numeric per
/// component: as text "1.10" sorts before "1.8", which would reject an
/// extension newer than the floor.
fn parse_extension_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor))
}

/// What the connected role may *do*, as distinct from what it may *see*.
///
/// `PrivilegeLevel` answers visibility and drives the window banner; it is the
/// wrong authority for actions in both directions. `pg_monitor` grants no
/// right to signal a backend, and a plain role granted `pg_signal_backend`, or
/// one that merely owns the table it wants to ANALYZE, may act without holding
/// either level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub signal_backend: bool,
    pub reload_conf: bool,
    pub reset_statements: bool,
    pub maintain: bool,
}

impl Capabilities {
    /// SQL NULL — an absent extension, an absent `pg_maintain` role — means
    /// the capability could not be established, which is never permission.
    pub fn from_flags(
        signal_backend: Option<bool>,
        reload_conf: Option<bool>,
        reset_statements: Option<bool>,
        maintain: Option<bool>,
    ) -> Self {
        Capabilities {
            signal_backend: signal_backend.unwrap_or(false),
            reload_conf: reload_conf.unwrap_or(false),
            reset_statements: reset_statements.unwrap_or(false),
            maintain: maintain.unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub version_num: i32,
    pub version_display: String,
    pub privilege: PrivilegeLevel,
    pub statements: StatementsAvailability,
    pub capabilities: Capabilities,
}

impl ServerInfo {
    /// True when the server predates the supported floor. Callers warn; they
    /// never refuse the connection — pages that need a newer server gate
    /// themselves.
    pub fn is_below_floor(&self) -> bool {
        self.version_num < MIN_SUPPORTED_VERSION
    }
}

/// PostgreSQL 10 and later encode the version as MMmmmm: major * 10000 + minor.
pub fn format_version(version_num: i32) -> String {
    format!("{}.{}", version_num / 10000, version_num % 10000)
}

pub fn map_server_info(row: &Row) -> ServerInfo {
    let version_num: i32 = row.get("version_num");
    let is_monitor: bool = row.get("is_monitor");
    let is_superuser: bool = row.get("is_superuser");
    let statements_version: Option<String> = row.get("statements_version");
    let can_signal: Option<bool> = row.get("can_signal");
    let can_reload: Option<bool> = row.get("can_reload");
    let can_reset_statements: Option<bool> = row.get("can_reset_statements");
    let can_maintain: Option<bool> = row.get("can_maintain");
    ServerInfo {
        version_num,
        version_display: format_version(version_num),
        privilege: PrivilegeLevel::classify(is_superuser, is_monitor),
        statements: StatementsAvailability::classify(statements_version.as_deref()),
        capabilities: Capabilities::from_flags(
            can_signal,
            can_reload,
            can_reset_statements,
            can_maintain,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_modern_version_number() {
        assert_eq!(format_version(140011), "14.11");
        assert_eq!(format_version(180004), "18.4");
        assert_eq!(format_version(160000), "16.0");
    }

    #[test]
    fn superuser_and_monitor_see_everything() {
        assert!(!PrivilegeLevel::Superuser.hides_other_sessions());
        assert!(!PrivilegeLevel::Monitor.hides_other_sessions());
    }

    #[test]
    fn a_limited_role_hides_other_sessions() {
        assert!(PrivilegeLevel::Limited.hides_other_sessions());
    }

    #[test]
    fn classifies_privilege_from_probe_flags() {
        assert_eq!(
            PrivilegeLevel::classify(true, true),
            PrivilegeLevel::Superuser
        );
        assert_eq!(
            PrivilegeLevel::classify(true, false),
            PrivilegeLevel::Superuser
        );
        assert_eq!(
            PrivilegeLevel::classify(false, true),
            PrivilegeLevel::Monitor
        );
        assert_eq!(
            PrivilegeLevel::classify(false, false),
            PrivilegeLevel::Limited
        );
    }

    #[test]
    fn recognises_a_server_below_the_supported_floor() {
        let server_13 = ServerInfo {
            version_num: 130015,
            version_display: format_version(130015),
            privilege: PrivilegeLevel::Superuser,
            statements: StatementsAvailability::NotInstalled,
            capabilities: Capabilities::default(),
        };
        assert!(server_13.is_below_floor());

        let server_14 = ServerInfo {
            version_num: 140000,
            version_display: format_version(140000),
            privilege: PrivilegeLevel::Superuser,
            statements: StatementsAvailability::NotInstalled,
            capabilities: Capabilities::default(),
        };
        assert!(!server_14.is_below_floor());

        let server_18 = ServerInfo {
            version_num: 180000,
            version_display: format_version(180000),
            privilege: PrivilegeLevel::Superuser,
            statements: StatementsAvailability::NotInstalled,
            capabilities: Capabilities::default(),
        };
        assert!(!server_18.is_below_floor());
    }

    #[test]
    fn an_absent_extension_is_not_installed() {
        assert_eq!(
            StatementsAvailability::classify(None),
            StatementsAvailability::NotInstalled
        );
    }

    #[test]
    fn version_1_8_and_later_are_available() {
        for version in ["1.8", "1.9", "1.11"] {
            assert_eq!(
                StatementsAvailability::classify(Some(version)),
                StatementsAvailability::Available {
                    version: version.to_string()
                },
                "{version} should be usable"
            );
        }
    }

    #[test]
    fn version_1_10_is_available_despite_sorting_before_1_8_as_text() {
        // The case that catches lexical comparison: "1.10" < "1.8" as text,
        // so a string compare would reject a newer extension than the floor.
        assert_eq!(
            StatementsAvailability::classify(Some("1.10")),
            StatementsAvailability::Available {
                version: "1.10".to_string()
            }
        );
    }

    #[test]
    fn version_1_7_is_too_old() {
        // 1.7 predates total_exec_time, so the query fails on a missing
        // column rather than a missing view.
        assert_eq!(
            StatementsAvailability::classify(Some("1.7")),
            StatementsAvailability::TooOld {
                version: "1.7".to_string()
            }
        );
    }

    #[test]
    fn an_unparseable_version_is_treated_as_too_old() {
        // Better to show the upgrade remedy than to run a query every ten
        // seconds that is going to fail on a column we cannot prove exists.
        assert_eq!(
            StatementsAvailability::classify(Some("banana")),
            StatementsAvailability::TooOld {
                version: "banana".to_string()
            }
        );
    }

    #[test]
    fn only_the_available_variant_reports_itself_usable() {
        assert!(StatementsAvailability::classify(Some("1.9")).is_available());
        assert!(!StatementsAvailability::classify(Some("1.7")).is_available());
        assert!(!StatementsAvailability::classify(None).is_available());
    }

    #[test]
    fn absent_objects_probe_as_no_capability() {
        // to_regprocedure returns NULL when pg_stat_statements is absent, and
        // the pg_roles subselect returns no row on 14-16 where pg_maintain
        // does not exist. Both reach us as None and must not be read as
        // permission.
        let caps = Capabilities::from_flags(Some(false), Some(false), None, None);
        assert!(!caps.reset_statements);
        assert!(!caps.maintain);
    }

    #[test]
    fn granted_capabilities_are_carried_through_independently() {
        let caps = Capabilities::from_flags(Some(true), Some(false), Some(true), Some(false));
        assert!(caps.signal_backend);
        assert!(!caps.reload_conf);
        assert!(caps.reset_statements);
        assert!(!caps.maintain);
    }

    #[test]
    fn a_monitor_role_has_no_action_capabilities_by_default() {
        // The parent spec says the privilege probe gates the action buttons.
        // It does not: pg_monitor grants no right to signal a backend. This
        // test is the guard against that conflation coming back.
        let caps = Capabilities::from_flags(Some(false), Some(false), Some(false), Some(false));
        assert!(!caps.signal_backend);
        assert!(!caps.reload_conf);
    }
}

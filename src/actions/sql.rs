/* actions/sql.rs
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

use crate::actions::{Action, MaintenanceKind};

/// Wraps a name in double quotes, doubling any it already contains.
///
/// `VACUUM` cannot be parameterised, so its identifiers are interpolated. The
/// names come from the catalogue rather than from the user, but
/// `CREATE TABLE "x""; DROP TABLE bar --"` is legal PostgreSQL, so the quoting
/// is required rather than decorative. Quoting unconditionally also preserves
/// case, which matters the moment a name is not all lower case.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn qualified_name(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

/// Everything the runner needs to execute one action: the session settings to
/// apply first, the statement, whether a PID binds as `$1`, and whether the
/// simple protocol is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub setup: String,
    pub sql: String,
    pub pid: Option<i32>,
    /// True when the statement must go through `batch_execute`. `VACUUM`
    /// cannot run inside a transaction block, and the extended protocol wraps
    /// its statement in an implicit one.
    pub batch: bool,
}

/// The sampler's own guard against a wedged server; short actions inherit it.
const QUICK_SETUP: &str = "SET statement_timeout = '5s'";

/// Maintenance runs without a statement timeout — a VACUUM may legitimately
/// take an hour, and a timeout firing part-way discards the work already done
/// — but keeps a lock timeout, so a VACUUM blocked behind conflicting DDL
/// reports rather than hanging invisibly.
const MAINTENANCE_SETUP: &str = "SET statement_timeout = 0; SET lock_timeout = '30s'";

pub fn plan_for(action: &Action) -> Plan {
    match action {
        Action::CancelBackend { pid } => Plan {
            setup: QUICK_SETUP.to_string(),
            sql: "SELECT pg_cancel_backend($1)".to_string(),
            pid: Some(*pid),
            batch: false,
        },
        Action::TerminateBackend { pid } => Plan {
            setup: QUICK_SETUP.to_string(),
            sql: "SELECT pg_terminate_backend($1)".to_string(),
            pid: Some(*pid),
            batch: false,
        },
        Action::ResetStatements => Plan {
            setup: QUICK_SETUP.to_string(),
            sql: "SELECT pg_stat_statements_reset()".to_string(),
            pid: None,
            batch: false,
        },
        Action::ReloadConfig => Plan {
            setup: QUICK_SETUP.to_string(),
            sql: "SELECT pg_reload_conf()".to_string(),
            pid: None,
            batch: false,
        },
        Action::Maintain {
            kind,
            schema,
            table,
        } => {
            let relation = qualified_name(schema, table);
            let sql = match kind {
                MaintenanceKind::Analyze => format!("ANALYZE {relation}"),
                MaintenanceKind::Vacuum => format!("VACUUM {relation}"),
                MaintenanceKind::VacuumAnalyze => format!("VACUUM (ANALYZE) {relation}"),
            };
            Plan {
                setup: MAINTENANCE_SETUP.to_string(),
                sql,
                pid: None,
                batch: true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{Action, MaintenanceKind};

    #[test]
    fn a_plain_identifier_is_still_quoted() {
        // Always quoting is what makes a reserved word or a capitalised name
        // safe; there is no case where leaving it bare is worth the branch.
        assert_eq!(quote_ident("orders"), "\"orders\"");
    }

    #[test]
    fn an_embedded_double_quote_is_doubled() {
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn a_name_carrying_sql_is_neutralised() {
        // Legal PostgreSQL: CREATE TABLE "x"; DROP TABLE bar --".
        assert_eq!(
            quote_ident("x\"; DROP TABLE bar --"),
            "\"x\"\"; DROP TABLE bar --\""
        );
    }

    #[test]
    fn case_is_preserved_because_quoting_makes_it_significant() {
        assert_eq!(quote_ident("MyTable"), "\"MyTable\"");
    }

    #[test]
    fn a_qualified_name_quotes_both_parts() {
        assert_eq!(qualified_name("public", "orders"), "\"public\".\"orders\"");
    }

    #[test]
    fn analyze_runs_on_the_simple_protocol_with_no_statement_timeout() {
        let plan = plan_for(&Action::Maintain {
            kind: MaintenanceKind::Analyze,
            schema: "public".to_string(),
            table: "orders".to_string(),
        });
        assert_eq!(plan.sql, "ANALYZE \"public\".\"orders\"");
        assert!(plan.batch, "maintenance must not use the extended protocol");
        assert_eq!(plan.pid, None);
        assert_eq!(
            plan.setup,
            "SET statement_timeout = 0; SET lock_timeout = '30s'"
        );
    }

    #[test]
    fn vacuum_and_vacuum_analyze_have_distinct_spellings() {
        let vacuum = plan_for(&Action::Maintain {
            kind: MaintenanceKind::Vacuum,
            schema: "public".to_string(),
            table: "orders".to_string(),
        });
        assert_eq!(vacuum.sql, "VACUUM \"public\".\"orders\"");

        let both = plan_for(&Action::Maintain {
            kind: MaintenanceKind::VacuumAnalyze,
            schema: "public".to_string(),
            table: "orders".to_string(),
        });
        assert_eq!(both.sql, "VACUUM (ANALYZE) \"public\".\"orders\"");
    }

    #[test]
    fn the_signal_actions_bind_the_pid_rather_than_interpolating_it() {
        let cancel = plan_for(&Action::CancelBackend { pid: 4821 });
        assert_eq!(cancel.sql, "SELECT pg_cancel_backend($1)");
        assert_eq!(cancel.pid, Some(4821));
        assert!(!cancel.batch);
        assert_eq!(cancel.setup, "SET statement_timeout = '5s'");

        let terminate = plan_for(&Action::TerminateBackend { pid: 4821 });
        assert_eq!(terminate.sql, "SELECT pg_terminate_backend($1)");
        assert_eq!(terminate.pid, Some(4821));
    }

    #[test]
    fn the_server_wide_actions_take_no_parameter() {
        let reset = plan_for(&Action::ResetStatements);
        assert_eq!(reset.sql, "SELECT pg_stat_statements_reset()");
        assert_eq!(reset.pid, None);
        assert!(!reset.batch);

        let reload = plan_for(&Action::ReloadConfig);
        assert_eq!(reload.sql, "SELECT pg_reload_conf()");
        assert_eq!(reload.pid, None);
        assert!(!reload.batch);
    }

    #[test]
    fn only_maintenance_lifts_the_statement_timeout() {
        for action in [
            Action::CancelBackend { pid: 1 },
            Action::TerminateBackend { pid: 1 },
            Action::ResetStatements,
            Action::ReloadConfig,
        ] {
            assert_eq!(
                plan_for(&action).setup,
                "SET statement_timeout = '5s'",
                "{action:?} must keep the sampler's timeout"
            );
        }
    }
}

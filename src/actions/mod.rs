/* actions/mod.rs
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

pub mod sql;

/// Which maintenance command to run. Kept separate from `Action` so the three
/// variants share one target and one capability check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceKind {
    Analyze,
    Vacuum,
    VacuumAnalyze,
}

/// A single operation the user asked for. Never constructed by the collector
/// or by a timer — only by a control the user activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CancelBackend {
        pid: i32,
    },
    TerminateBackend {
        pid: i32,
    },
    Maintain {
        kind: MaintenanceKind,
        schema: String,
        table: String,
    },
    ResetStatements,
    ReloadConfig,
}

impl Action {
    /// True for anything that interrupts another user's work or destroys data
    /// that cannot be recovered. `pg_reload_conf` is idempotent and loses
    /// nothing; VACUUM and ANALYZE are what autovacuum does unprompted.
    pub fn requires_confirmation(&self) -> bool {
        matches!(
            self,
            Action::CancelBackend { .. }
                | Action::TerminateBackend { .. }
                | Action::ResetStatements
        )
    }

    /// What the action was aimed at, for the result message. `None` for the
    /// server-wide actions, whose message names no target.
    pub fn target(&self) -> Option<String> {
        match self {
            Action::CancelBackend { pid } | Action::TerminateBackend { pid } => {
                Some(pid.to_string())
            }
            Action::Maintain { schema, table, .. } => Some(format!("{schema}.{table}")),
            Action::ResetStatements | Action::ReloadConfig => None,
        }
    }

    /// True when the action may take minutes rather than milliseconds, so the
    /// window knows to post a persistent in-flight notice rather than assume
    /// the result toast will follow immediately.
    pub fn is_long_running(&self) -> bool {
        matches!(self, Action::Maintain { .. })
    }
}

/// How an action ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Succeeded,
    /// The signal functions return false when the PID has already gone.
    NoSuchBackend,
    Failed(String),
}

/// Classifies the boolean `pg_cancel_backend` and `pg_terminate_backend`
/// return.
pub fn signal_outcome(returned: bool) -> ActionOutcome {
    if returned {
        ActionOutcome::Succeeded
    } else {
        ActionOutcome::NoSuchBackend
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_affecting_other_users_or_destroying_data_are_confirmed() {
        assert!(Action::CancelBackend { pid: 1 }.requires_confirmation());
        assert!(Action::TerminateBackend { pid: 1 }.requires_confirmation());
        assert!(Action::ResetStatements.requires_confirmation());
    }

    #[test]
    fn idempotent_and_routine_actions_are_not_confirmed() {
        assert!(!Action::ReloadConfig.requires_confirmation());
        for kind in [
            MaintenanceKind::Analyze,
            MaintenanceKind::Vacuum,
            MaintenanceKind::VacuumAnalyze,
        ] {
            assert!(!Action::Maintain {
                kind,
                schema: "public".to_string(),
                table: "orders".to_string(),
            }
            .requires_confirmation());
        }
    }

    #[test]
    fn a_signal_that_found_its_backend_succeeded() {
        assert_eq!(signal_outcome(true), ActionOutcome::Succeeded);
    }

    #[test]
    fn a_signal_that_found_nothing_is_neither_success_nor_failure() {
        // pg_cancel_backend returns false when the PID has already gone.
        // Reporting that as success would claim work that never happened;
        // reporting it as an error would blame the user for a race.
        assert_eq!(signal_outcome(false), ActionOutcome::NoSuchBackend);
    }

    #[test]
    fn maintenance_reports_the_relation_it_targeted() {
        let action = Action::Maintain {
            kind: MaintenanceKind::Vacuum,
            schema: "public".to_string(),
            table: "orders".to_string(),
        };
        assert_eq!(action.target(), Some("public.orders".to_string()));
        assert_eq!(
            Action::CancelBackend { pid: 4821 }.target(),
            Some("4821".to_string())
        );
        assert_eq!(Action::ReloadConfig.target(), None);
    }

    #[test]
    fn only_maintenance_is_long_running() {
        assert!(Action::Maintain {
            kind: MaintenanceKind::Vacuum,
            schema: "public".to_string(),
            table: "orders".to_string(),
        }
        .is_long_running());
        assert!(!Action::CancelBackend { pid: 1 }.is_long_running());
        assert!(!Action::ResetStatements.is_long_running());
    }
}

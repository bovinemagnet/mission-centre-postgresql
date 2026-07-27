/* window_actions.rs
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

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::gio;

use mission_centre_pg::actions::{Action, ActionOutcome, MaintenanceKind};
use mission_centre_pg::collector::snapshot::Session;
use mission_centre_pg::collector::worker::CollectorEvent;
use mission_centre_pg::connection::probe::Capabilities;
use mission_centre_pg::i18n::{i18n, i18n_f};

use crate::window::MissionCentrePgWindow;

const ACTION_CANCEL: &str = "cancel-backend";
const ACTION_TERMINATE: &str = "terminate-backend";
const ACTION_ANALYZE: &str = "analyze-table";
const ACTION_VACUUM: &str = "vacuum-table";
const ACTION_VACUUM_ANALYZE: &str = "vacuum-analyze-table";
const ACTION_RESET_STATEMENTS: &str = "reset-statements";
const ACTION_RELOAD_CONF: &str = "reload-configuration";

/// Everything needed to tell one backend apart from another that looks like
/// it. Fields the server withheld are simply omitted rather than rendered as
/// blanks, which would read as "this backend has no user".
pub fn confirmation_body(session: &Session) -> String {
    let mut lines = vec![i18n_f("PID {}", &[&session.pid.to_string()])];

    if let Some(user) = session.user_name.as_deref() {
        lines.push(i18n_f("User {}", &[user]));
    }
    if let Some(database) = session.database.as_deref() {
        lines.push(i18n_f("Database {}", &[database]));
    }
    if let Some(application) = session.application_name.as_deref() {
        lines.push(i18n_f("Application {}", &[application]));
    }
    if let Some(state) = session.state.as_deref() {
        lines.push(i18n_f("State {}", &[state]));
    }
    if let Some(secs) = session.query_duration_secs {
        lines.push(i18n_f("Running for {} seconds", &[&format!("{secs:.0}")]));
    }
    if let Some(query) = session.query.as_deref() {
        lines.push(String::new());
        lines.push(query.to_string());
    }

    lines.join("\n")
}

/// The toast text for a finished action.
pub fn outcome_message(action: &Action, outcome: &ActionOutcome) -> String {
    match outcome {
        ActionOutcome::Succeeded => match action {
            Action::CancelBackend { pid } => {
                i18n_f("Cancelled the query on backend {}.", &[&pid.to_string()])
            }
            Action::TerminateBackend { pid } => {
                i18n_f("Terminated backend {}.", &[&pid.to_string()])
            }
            Action::Maintain { kind, .. } => {
                let relation = action.target().unwrap_or_default();
                match kind {
                    MaintenanceKind::Analyze => i18n_f("Analysed {}.", &[&relation]),
                    MaintenanceKind::Vacuum => i18n_f("Vacuumed {}.", &[&relation]),
                    MaintenanceKind::VacuumAnalyze => {
                        i18n_f("Vacuumed and analysed {}.", &[&relation])
                    }
                }
            }
            Action::ResetStatements => i18n("Query statistics reset."),
            Action::ReloadConfig => i18n("Configuration reloaded."),
        },
        // Neither a success nor an error: the backend exited between the
        // sample that listed it and the signal.
        ActionOutcome::NoSuchBackend => i18n_f(
            "Backend {} was no longer running.",
            &[&action.target().unwrap_or_default()],
        ),
        ActionOutcome::Failed(message) => i18n_f("Action failed: {}", &[message]),
    }
}

/// The in-flight notice for a long-running action.
fn in_flight_message(action: &Action) -> String {
    let command = match action {
        Action::Maintain {
            kind: MaintenanceKind::Analyze,
            ..
        } => "ANALYZE",
        Action::Maintain {
            kind: MaintenanceKind::Vacuum,
            ..
        } => "VACUUM",
        _ => "VACUUM (ANALYZE)",
    };
    i18n_f(
        "Running {} on {}…",
        &[command, &action.target().unwrap_or_default()],
    )
}

impl MissionCentrePgWindow {
    /// Registers the seven actions and connects the two selection sources that
    /// change their enablement. Called once, from `constructed`.
    pub fn install_actions(&self) {
        for name in [
            ACTION_CANCEL,
            ACTION_TERMINATE,
            ACTION_ANALYZE,
            ACTION_VACUUM,
            ACTION_VACUUM_ANALYZE,
            ACTION_RESET_STATEMENTS,
            ACTION_RELOAD_CONF,
        ] {
            let action = gio::SimpleAction::new(name, None);
            action.set_enabled(false);
            let window = self.clone();
            let name = name.to_string();
            action.connect_activate(move |_, _| window.activate_action_named(&name));
            self.add_action(&action);
        }

        let window = self.clone();
        self.imp()
            .sessions_page
            .connect_selection_changed(move || window.update_action_enablement());

        let window = self.clone();
        self.imp()
            .relations_page
            .connect_tables_selection_changed(move || window.update_action_enablement());
    }

    /// Builds the `Action` for a named GAction and either confirms it or
    /// submits it. A control whose target has gone between the click and here
    /// simply does nothing — the enablement pass that follows will disable it.
    fn activate_action_named(&self, name: &str) {
        if let Some(action) = self.action_for(name) {
            if action.requires_confirmation() {
                self.confirm_then(action);
            } else {
                self.submit_action(action);
            }
        }
    }

    /// Separate from `activate_action_named` so the selected-row lookups can
    /// use `?`: the activation path itself returns nothing.
    fn action_for(&self, name: &str) -> Option<Action> {
        match name {
            ACTION_CANCEL => Some(Action::CancelBackend {
                pid: self.signal_target()?.pid,
            }),
            ACTION_TERMINATE => Some(Action::TerminateBackend {
                pid: self.signal_target()?.pid,
            }),
            ACTION_ANALYZE => self.maintenance_action(MaintenanceKind::Analyze),
            ACTION_VACUUM => self.maintenance_action(MaintenanceKind::Vacuum),
            ACTION_VACUUM_ANALYZE => self.maintenance_action(MaintenanceKind::VacuumAnalyze),
            ACTION_RESET_STATEMENTS => Some(Action::ResetStatements),
            ACTION_RELOAD_CONF => Some(Action::ReloadConfig),
            _ => None,
        }
    }

    /// The backend a cancel or terminate would act on. Both the Sessions and
    /// the Locks page can select one, so the target follows whichever page the
    /// user is looking at — otherwise terminating a blocker found on the Locks
    /// page would silently signal whatever the Sessions page had selected.
    fn signal_target(&self) -> Option<Session> {
        let imp = self.imp();
        match imp.view_stack.visible_child_name().as_deref() {
            Some("locks") => imp.locks_page.selected_session(),
            _ => imp.sessions_page.selected_session(),
        }
    }

    fn maintenance_action(&self, kind: MaintenanceKind) -> Option<Action> {
        let table = self.imp().relations_page.selected_table()?;
        Some(Action::Maintain {
            kind,
            schema: table.schema_name,
            table: table.table_name,
        })
    }

    /// Presents a dialog naming the exact target, and submits only on the
    /// affirmative response.
    fn confirm_then(&self, action: Action) {
        let session_body = || {
            self.signal_target()
                .map(|session| confirmation_body(&session))
                .unwrap_or_default()
        };

        let (heading, body, verb, destructive) = match &action {
            Action::CancelBackend { .. } => (
                i18n("Cancel this query?"),
                session_body(),
                i18n("Cancel query"),
                false,
            ),
            Action::TerminateBackend { .. } => (
                i18n("Terminate this backend?"),
                session_body(),
                i18n("Terminate"),
                true,
            ),
            _ => (
                i18n("Reset query statistics?"),
                i18n(
                    "Every statistic pg_stat_statements has accumulated since the last reset is discarded. This cannot be undone.",
                ),
                i18n("Reset"),
                true,
            ),
        };

        let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
        dialog.add_response("dismiss", &i18n("Dismiss"));
        dialog.add_response("confirm", &verb);
        if destructive {
            dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
        }
        dialog.set_default_response(Some("dismiss"));
        dialog.set_close_response("dismiss");

        let window = self.clone();
        dialog.connect_response(None, move |dialog, response| {
            dialog.close();
            if response == "confirm" {
                window.submit_action(action.clone());
            }
        });
        dialog.present(Some(self));
    }

    fn submit_action(&self, action: Action) {
        let accepted = self
            .imp()
            .collector
            .borrow()
            .as_ref()
            .map(|handle| handle.submit(action))
            .unwrap_or(false);

        if !accepted {
            self.add_toast_text(&i18n(
                "The action could not be sent — the connection is busy or has gone.",
            ));
        }
    }

    /// Recomputes all seven actions. Called on connect, disconnect, and every
    /// selection change — including the ones a refresh causes when the
    /// selected row disappears.
    pub fn update_action_enablement(&self) {
        let imp = self.imp();
        let connected = imp.connected.get();
        let capabilities = imp.capabilities.borrow().unwrap_or_default();

        let has_session = imp.sessions_page.selected_session().is_some();
        let can_signal = connected && capabilities.signal_backend && has_session;
        self.set_action_enabled(ACTION_CANCEL, can_signal);
        self.set_action_enabled(ACTION_TERMINATE, can_signal);

        let can_maintain = connected
            && imp
                .relations_page
                .selected_table()
                .map(|table| table.may_maintain(capabilities.maintain))
                .unwrap_or(false);
        self.set_action_enabled(ACTION_ANALYZE, can_maintain);
        self.set_action_enabled(ACTION_VACUUM, can_maintain);
        self.set_action_enabled(ACTION_VACUUM_ANALYZE, can_maintain);

        self.set_action_enabled(
            ACTION_RESET_STATEMENTS,
            connected && capabilities.reset_statements,
        );
        self.set_action_enabled(ACTION_RELOAD_CONF, connected && capabilities.reload_conf);

        imp.sessions_page.set_capabilities(&capabilities);
        imp.relations_page.set_capabilities(&capabilities);
        imp.locks_page.set_capabilities(&capabilities);
    }

    fn set_action_enabled(&self, name: &str, enabled: bool) {
        if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
            action.set_enabled(enabled);
        }
    }

    /// Records the connection's capabilities and re-runs enablement.
    pub fn set_capabilities(&self, capabilities: Option<Capabilities>) {
        let imp = self.imp();
        imp.connected.set(capabilities.is_some());
        imp.capabilities.replace(capabilities);
        self.update_action_enablement();
    }

    /// The two action events. A long-running action posts a persistent notice
    /// when it starts, which the result dismisses.
    pub fn handle_action_event(&self, event: CollectorEvent) {
        match event {
            CollectorEvent::ActionStarted(action) if action.is_long_running() => {
                let toast = adw::Toast::new(&in_flight_message(&action));
                toast.set_timeout(0);
                self.imp().toast_overlay.add_toast(toast.clone());
                self.imp().in_flight_toast.replace(Some(toast));
            }
            CollectorEvent::ActionStarted(_) => {}
            CollectorEvent::ActionFinished { action, outcome } => {
                if let Some(toast) = self.imp().in_flight_toast.take() {
                    toast.dismiss();
                }
                self.add_toast_text(&outcome_message(&action, &outcome));
            }
            _ => {}
        }
    }

    fn add_toast_text(&self, text: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mission_centre_pg::actions::{Action, MaintenanceKind};
    use mission_centre_pg::collector::snapshot::Session;

    fn session() -> Session {
        Session {
            pid: 4821,
            user_name: Some("alice".to_string()),
            application_name: Some("orders-api".to_string()),
            client_addr: None,
            database: Some("prod".to_string()),
            state: Some("idle in transaction".to_string()),
            wait_event_type: None,
            wait_event: None,
            backend_type: Some("client backend".to_string()),
            query_duration_secs: Some(842.0),
            query: Some("UPDATE orders SET status = 'sent'".to_string()),
        }
    }

    #[test]
    fn a_confirmation_names_every_field_needed_to_identify_the_backend() {
        // The table re-sorts under the pointer every two seconds. A dialog
        // that does not name its target is how the wrong backend gets killed.
        let body = confirmation_body(&session());
        assert!(body.contains("4821"));
        assert!(body.contains("alice"));
        assert!(body.contains("prod"));
        assert!(body.contains("idle in transaction"));
        assert!(body.contains("UPDATE orders SET status = 'sent'"));
    }

    #[test]
    fn a_confirmation_survives_a_session_with_nothing_but_a_pid() {
        let bare = Session {
            user_name: None,
            application_name: None,
            database: None,
            state: None,
            query: None,
            query_duration_secs: None,
            ..session()
        };
        let body = confirmation_body(&bare);
        assert!(body.contains("4821"));
    }

    #[test]
    fn a_missing_backend_is_reported_as_neither_success_nor_failure() {
        let message = outcome_message(
            &Action::CancelBackend { pid: 4821 },
            &ActionOutcome::NoSuchBackend,
        );
        assert!(message.contains("4821"));
        assert!(message.contains("no longer running"));
    }

    #[test]
    fn a_maintenance_success_names_the_relation() {
        let message = outcome_message(
            &Action::Maintain {
                kind: MaintenanceKind::Vacuum,
                schema: "public".to_string(),
                table: "orders".to_string(),
            },
            &ActionOutcome::Succeeded,
        );
        assert!(message.contains("public.orders"));
    }

    #[test]
    fn a_failure_carries_the_servers_own_words() {
        let message = outcome_message(
            &Action::ReloadConfig,
            &ActionOutcome::Failed("permission denied for function pg_reload_conf".to_string()),
        );
        assert!(message.contains("permission denied for function pg_reload_conf"));
    }
}

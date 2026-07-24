/* window.rs
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

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::prelude::{Cast, IsA};
use gtk::{gio, glib};

use mission_centre_pg::collector::worker::{
    spawn, CollectorConfig, CollectorEvent, CollectorHandle,
};
use mission_centre_pg::connection::params::ConnectionParams;
use mission_centre_pg::connection::{credentials, registry};
use mission_centre_pg::dialogs::McpgAddServerDialog;
use mission_centre_pg::pages::{
    McpgOverviewPage, McpgQueriesPage, McpgRelationsPage, McpgSessionsPage,
};
use mission_centre_pg::widgets::sidebar_row::{ConnectionState, McpgSidebarRow};

use mission_centre_pg::i18n::{i18n, i18n_f};

use crate::application::APP_ID;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/window.ui")]
    pub struct MissionCentrePgWindow {
        #[template_child]
        pub server_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub add_server_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub privilege_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub error_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub overview_page: TemplateChild<McpgOverviewPage>,
        #[template_child]
        pub sessions_page: TemplateChild<McpgSessionsPage>,
        #[template_child]
        pub queries_page: TemplateChild<McpgQueriesPage>,
        #[template_child]
        pub relations_page: TemplateChild<McpgRelationsPage>,

        pub settings: RefCell<Option<gio::Settings>>,
        pub servers: RefCell<Vec<ConnectionParams>>,
        pub collector: RefCell<Option<CollectorHandle>>,
        /// Bumped on every `select_server` call and captured by the spawned
        /// event loop, so events from a superseded collector can be told
        /// apart from events belonging to the currently selected one.
        pub generation: Cell<u64>,
        /// Set while `reload_servers` is restoring the selection it
        /// remembered, so the `row-selected` handler does not treat that
        /// restoration as a user pick and reconnect an already-healthy
        /// collector.
        pub restoring_selection: Cell<bool>,
        /// The below-floor warning for the connected server, if any. Unlike a
        /// transient error it describes a permanent property of the server, so
        /// it is re-asserted after each successful `Sample` rather than being
        /// cleared. Reset to `None` on a server switch or a fresh connection.
        pub below_floor_warning: RefCell<Option<String>>,
        /// The database of the currently selected server, for messages that
        /// name it. Extension presence is a per-database property.
        pub connected_database: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MissionCentrePgWindow {
        const NAME: &'static str = "MissionCentrePgWindow";
        type Type = super::MissionCentrePgWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            McpgOverviewPage::ensure_type();
            McpgSessionsPage::ensure_type();
            McpgQueriesPage::ensure_type();
            McpgRelationsPage::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MissionCentrePgWindow {
        fn constructed(&self) {
            self.parent_constructed();

            let settings = gio::Settings::new(APP_ID);
            self.overview_page
                .set_graph_points(settings.int("graph-points").max(1) as u32);
            self.sessions_page
                .set_hide_idle(settings.boolean("hide-idle-sessions"));
            self.settings.replace(Some(settings));

            let window = self.obj().clone();
            self.add_server_button
                .connect_clicked(move |_| window.present_add_server_dialog());

            let window = self.obj().clone();
            self.server_list.connect_row_selected(move |_, row| {
                if window.imp().restoring_selection.get() {
                    return;
                }
                if let Some(row) = row {
                    window.select_server(row.index());
                }
            });

            self.obj().reload_servers();
        }
    }

    impl WidgetImpl for MissionCentrePgWindow {}
    impl WindowImpl for MissionCentrePgWindow {}
    impl ApplicationWindowImpl for MissionCentrePgWindow {}
    impl AdwApplicationWindowImpl for MissionCentrePgWindow {}
}

glib::wrapper! {
    pub struct MissionCentrePgWindow(ObjectSubclass<imp::MissionCentrePgWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

/// Events from a superseded collector must be discarded: the old thread keeps
/// draining for a moment after `stop()`, and its final `Disconnected` would
/// otherwise be applied to whichever server is now selected.
fn is_current(event_generation: u64, current_generation: u64) -> bool {
    event_generation == current_generation
}

impl MissionCentrePgWindow {
    pub fn new(app: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    fn settings(&self) -> gio::Settings {
        self.imp()
            .settings
            .borrow()
            .clone()
            .expect("settings are created in constructed()")
    }

    fn reload_servers(&self) {
        let imp = self.imp();
        let servers = registry::load(&self.settings());

        // A still-connected server's collector keeps running untouched
        // through a reload; losing its selection here would strand the user
        // on no row while the pages keep updating underneath them. Remember
        // it by id so the selection can be restored after the rebuild below
        // without disturbing that collector.
        let selected_id = imp
            .server_list
            .selected_row()
            .map(|row| row.index())
            .and_then(|index| imp.servers.borrow().get(index as usize).map(|p| p.id));

        while let Some(child) = imp.server_list.first_child() {
            imp.server_list.remove(&child);
        }

        let mut restore_index = None;
        for (i, server) in servers.iter().enumerate() {
            let row = McpgSidebarRow::new(&server.label);
            row.set_subheading(&format!("{}:{}", server.host, server.port));
            row.set_state(ConnectionState::Disconnected);
            imp.server_list.append(&row);
            if Some(server.id) == selected_id {
                restore_index = Some(i as i32);
            }
        }

        imp.servers.replace(servers);

        // Restore the selection without going through `select_server`: the
        // collector for this server is still running and connected, and
        // re-selecting it must not reconnect a healthy connection just
        // because an unrelated server was added.
        if let Some(index) = restore_index {
            if let Some(row) = imp.server_list.row_at_index(index) {
                imp.restoring_selection.set(true);
                imp.server_list.select_row(Some(&row));
                imp.restoring_selection.set(false);
            }
        }
    }

    fn present_add_server_dialog(&self) {
        let dialog = McpgAddServerDialog::new();
        let window = self.clone();
        dialog.connect_added(move |params| {
            let mut servers = registry::load(&window.settings());
            servers.push(params.clone());
            if let Err(e) = registry::save(&window.settings(), &servers) {
                gtk::glib::g_warning!("mission-centre-pg", "could not save the server list: {e}");
            }
            window.reload_servers();
        });
        dialog.present(Some(self));
    }

    fn select_server(&self, index: i32) {
        let imp = self.imp();

        // Neither banner, nor either page's own privilege banner, belongs to
        // the server about to be selected: leaving them set would let a
        // limited-privilege server's banner survive onto a server that fails
        // to connect entirely.
        imp.privilege_banner.set_revealed(false);
        imp.sessions_page.set_privilege_limited(false);
        imp.queries_page.set_privilege_limited(false);
        // The below-floor warning belongs to the previously connected server;
        // clear it so it cannot survive onto the server about to be selected.
        imp.below_floor_warning.replace(None);

        if let Some(handle) = imp.collector.take() {
            handle.stop();
        }

        let Some(params) = imp.servers.borrow().get(index as usize).cloned() else {
            return;
        };
        imp.connected_database.replace(params.database.clone());

        // Clear the sparkline before the new collector's first sample
        // arrives: without this, re-selecting a row after time spent on a
        // different server would join its old history to the new one across
        // the gap where nothing was sampled, drawing a misleading shape.
        if let Some(row) = self.selected_row() {
            row.reset_series();
        }

        // Clear the queries page too: its rows belong to the server just
        // left, and the slow tier will not refresh them for up to one slow
        // interval, during which the old server's statistics would sit
        // under the new connection with nothing to mark them stale.
        imp.queries_page.clear();
        // Clear the relations page for the same reason: its rows also belong
        // to the server just left, and are also refreshed only on the slow
        // tier.
        imp.relations_page.clear();

        let password = match credentials::fetch_password(&params.id) {
            Ok(password) => password.unwrap_or_default(),
            Err(e) => {
                // `Ok(None)` (no password stored) is a normal case and stays
                // quiet; this arm is reached only for an unreadable secret
                // store, which the user otherwise has no way to learn about.
                gtk::glib::g_warning!(
                    "mission-centre-pg",
                    "could not read the stored password, continuing without one: {e}"
                );
                String::new()
            }
        };

        let settings = self.settings();
        let config = CollectorConfig {
            interval: std::time::Duration::from_millis(
                settings.int("sample-interval-ms").max(500) as u64
            ),
            slow_interval: std::time::Duration::from_millis(
                settings.int("slow-sample-interval-ms").max(2000) as u64,
            ),
            statements_limit: settings.int("statements-limit").max(10) as i64,
            relations_limit: settings.int("relations-limit").max(10) as i64,
        };

        // Every event this collector ever emits is stamped with this
        // generation; the event loop below discards anything that arrives
        // after a later `select_server` call has moved the generation on.
        let generation = imp.generation.get().saturating_add(1);
        imp.generation.set(generation);

        let handle = spawn(params, password, config);
        let events = handle.events.clone();
        imp.collector.replace(Some(handle));

        let window = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                if !is_current(generation, window.imp().generation.get()) {
                    // Superseded: the collector that sent this has already
                    // been told to stop, so stop draining for it too rather
                    // than spin until its channel closes.
                    break;
                }
                window.handle_event(event);
            }
        });
    }

    fn selected_row(&self) -> Option<McpgSidebarRow> {
        // `server_list` holds plain `McpgSidebarRow` children, which
        // `GtkListBox` wraps in its own `GtkListBoxRow`; the row itself is
        // never an `McpgSidebarRow`, so the widget of interest is its child.
        self.imp()
            .server_list
            .selected_row()
            .and_then(|row| row.child())
            .and_then(|child| child.downcast::<McpgSidebarRow>().ok())
    }

    fn handle_event(&self, event: CollectorEvent) {
        let imp = self.imp();

        match event {
            CollectorEvent::Connecting => {
                imp.error_banner.set_revealed(false);
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Connecting);
                }
            }
            CollectorEvent::Connected(info) => {
                imp.error_banner.set_revealed(false);
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Connected);
                    row.set_subheading(&i18n_f("PostgreSQL {}", &[&info.version_display]));
                }

                let limited = info.privilege.hides_other_sessions();
                imp.privilege_banner.set_revealed(limited);
                imp.privilege_banner.set_title(&i18n(
                    "Connected without pg_monitor — query text and statistics for other users' sessions are hidden.",
                ));
                imp.sessions_page.set_privilege_limited(limited);
                imp.queries_page.set_privilege_limited(limited);
                imp.queries_page.set_statements_availability(
                    &info.statements,
                    &imp.connected_database.borrow(),
                );
                imp.relations_page
                    .set_database(&imp.connected_database.borrow());

                if info.is_below_floor() {
                    let message = i18n_f(
                        "PostgreSQL {} is older than the supported floor of 14. Some statistics may be missing.",
                        &[&info.version_display],
                    );
                    imp.error_banner.set_revealed(true);
                    imp.error_banner.set_title(&message);
                    imp.below_floor_warning.replace(Some(message));
                } else {
                    imp.below_floor_warning.replace(None);
                }
            }
            CollectorEvent::Sample(snapshot) => {
                // A successful sample clears a transient error, but must not
                // clear the below-floor warning, which is a permanent property
                // of the connected server; re-assert it instead.
                match imp.below_floor_warning.borrow().as_ref() {
                    Some(message) => {
                        imp.error_banner.set_revealed(true);
                        imp.error_banner.set_title(message);
                    }
                    None => imp.error_banner.set_revealed(false),
                }
                imp.overview_page.update(&snapshot);
                imp.sessions_page.update(&snapshot.sessions);
                // None means this was a fast tick, so the page keeps what it
                // has. Err means the slow tier ran and failed.
                match snapshot.statements.as_ref() {
                    Some(Ok(sample)) => imp.queries_page.update(sample),
                    Some(Err(error)) => imp.queries_page.set_error(&i18n(&error.to_string())),
                    None => {}
                }
                match snapshot.relations.as_ref() {
                    Some(Ok(sample)) => imp.relations_page.update(sample),
                    Some(Err(error)) => imp.relations_page.set_error(&i18n(&error.to_string())),
                    None => {}
                }
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Connected);
                    row.push_value(snapshot.session_counts.total() as f64);
                }
            }
            CollectorEvent::Error(error) => {
                imp.error_banner.set_revealed(true);
                imp.error_banner.set_title(&i18n(&error.to_string()));
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Failed);
                }
            }
            CollectorEvent::Disconnected => {
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Disconnected);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_current_when_the_generation_matches() {
        assert!(is_current(3, 3));
    }

    #[test]
    fn is_stale_when_the_generation_is_older_than_current() {
        assert!(!is_current(1, 2));
    }

    #[test]
    fn is_current_at_generation_zero() {
        // The window's initial generation before any `select_server` call.
        assert!(is_current(0, 0));
    }
}

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

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::prelude::{Cast, IsA};
use gtk::{gio, glib};

use mission_centre_pg::collector::worker::{spawn, CollectorEvent, CollectorHandle};
use mission_centre_pg::connection::params::ConnectionParams;
use mission_centre_pg::connection::{credentials, registry};
use mission_centre_pg::dialogs::McpgAddServerDialog;
use mission_centre_pg::pages::{McpgOverviewPage, McpgSessionsPage};
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

        pub settings: RefCell<Option<gio::Settings>>,
        pub servers: RefCell<Vec<ConnectionParams>>,
        pub collector: RefCell<Option<CollectorHandle>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MissionCentrePgWindow {
        const NAME: &'static str = "MissionCentrePgWindow";
        type Type = super::MissionCentrePgWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            McpgOverviewPage::ensure_type();
            McpgSessionsPage::ensure_type();
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

        while let Some(child) = imp.server_list.first_child() {
            imp.server_list.remove(&child);
        }

        for server in &servers {
            let row = McpgSidebarRow::new(&server.label);
            row.set_subheading(&format!("{}:{}", server.host, server.port));
            row.set_state(ConnectionState::Disconnected);
            imp.server_list.append(&row);
        }

        imp.servers.replace(servers);
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

        if let Some(handle) = imp.collector.take() {
            handle.stop();
        }

        let Some(params) = imp.servers.borrow().get(index as usize).cloned() else {
            return;
        };

        // Clear the sparkline before the new collector's first sample
        // arrives: without this, re-selecting a row after time spent on a
        // different server would join its old history to the new one across
        // the gap where nothing was sampled, drawing a misleading shape.
        if let Some(row) = self.selected_row() {
            row.reset_series();
        }

        let password = credentials::fetch_password(&params.id)
            .ok()
            .flatten()
            .unwrap_or_default();

        let interval = std::time::Duration::from_millis(
            self.settings().int("sample-interval-ms").max(500) as u64,
        );

        let handle = spawn(params, password, interval);
        let events = handle.events.clone();
        imp.collector.replace(Some(handle));

        let window = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
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

                if info.is_below_floor() {
                    imp.error_banner.set_revealed(true);
                    imp.error_banner.set_title(&i18n_f(
                        "PostgreSQL {} is older than the supported floor of 14. Some statistics may be missing.",
                        &[&info.version_display],
                    ));
                }
            }
            CollectorEvent::Sample(snapshot) => {
                imp.error_banner.set_revealed(false);
                imp.overview_page.update(&snapshot);
                imp.sessions_page.update(&snapshot.sessions);
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Connected);
                    row.push_value(snapshot.session_counts.total() as f64);
                }
            }
            CollectorEvent::Error(error) => {
                imp.error_banner.set_revealed(true);
                imp.error_banner.set_title(&error.to_string());
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

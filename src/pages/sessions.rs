/* pages/sessions.rs
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
use gtk::glib;

use crate::collector::snapshot::Session;
use crate::connection::probe::Capabilities;
use crate::i18n::i18n;
use crate::table::{Column, Table};

const COLUMNS: &[Column<Session>] = &[
    Column {
        title: "PID",
        render: |s| s.pid.to_string(),
        sort_key: Some(|s| s.pid as f64),
        expand: false,
    },
    Column {
        title: "User",
        render: |s| s.user_name.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Database",
        render: |s| s.database.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Application",
        render: |s| s.application_name.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Client",
        render: |s| s.client_addr.clone().unwrap_or_else(|| "local".to_string()),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "State",
        render: |s| s.state.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Wait",
        render: |s| match (&s.wait_event_type, &s.wait_event) {
            (Some(kind), Some(event)) => format!("{kind}: {event}"),
            _ => String::new(),
        },
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Duration",
        render: |s| match s.query_duration_secs {
            Some(secs) if secs >= 1.0 => format!("{secs:.0}s"),
            Some(secs) => format!("{:.0}ms", secs * 1000.0),
            None => String::new(),
        },
        // A session with no running query has no duration; sort it as zero so
        // idle backends group at the short end rather than sorting by text.
        sort_key: Some(|s| s.query_duration_secs.unwrap_or(0.0)),
        expand: false,
    },
    Column {
        title: "Query",
        render: |s| {
            s.query
                .clone()
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        },
        sort_key: None,
        expand: true,
    },
];

fn session_key(session: &Session) -> String {
    session.pid.to_string()
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/sessions_page.ui")]
    pub struct McpgSessionsPage {
        #[template_child]
        pub filter_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub hide_idle_toggle: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub privilege_note: TemplateChild<adw::Banner>,
        #[template_child]
        pub column_view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub signal_reason: TemplateChild<gtk::Label>,
        #[template_child]
        pub cancel_backend_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub terminate_backend_button: TemplateChild<gtk::Button>,

        pub table: RefCell<Option<Table<Session>>>,
        pub hide_idle: Cell<bool>,
        pub filter_text: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgSessionsPage {
        const NAME: &'static str = "McpgSessionsPage";
        type Type = super::McpgSessionsPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgSessionsPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.hide_idle.set(true);

            let page = self.obj().clone();
            let table = Table::attach(
                &self.column_view.get(),
                COLUMNS,
                move |session| page.matches(session),
                session_key,
            );
            self.table.replace(Some(table));

            let page = self.obj().clone();
            self.filter_entry.connect_search_changed(move |entry| {
                page.imp()
                    .filter_text
                    .replace(entry.text().to_lowercase().to_string());
                page.refilter();
            });

            let page = self.obj().clone();
            self.hide_idle_toggle.connect_toggled(move |button| {
                page.imp().hide_idle.set(button.is_active());
                page.refilter();
            });
        }
    }

    impl WidgetImpl for McpgSessionsPage {}
    impl BoxImpl for McpgSessionsPage {}
}

glib::wrapper! {
    pub struct McpgSessionsPage(ObjectSubclass<imp::McpgSessionsPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgSessionsPage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn matches(&self, session: &Session) -> bool {
        let imp = self.imp();

        if imp.hide_idle.get() && session.state.as_deref() == Some("idle") {
            return false;
        }

        let needle = imp.filter_text.borrow();
        if needle.is_empty() {
            return true;
        }

        let haystack = [
            session.user_name.as_deref(),
            session.database.as_deref(),
            session.application_name.as_deref(),
            session.query.as_deref(),
        ];
        haystack
            .iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(needle.as_str()))
    }

    fn refilter(&self) {
        if let Some(table) = self.imp().table.borrow().as_ref() {
            table.refilter();
        }
    }

    pub fn set_hide_idle(&self, hide: bool) {
        self.imp().hide_idle_toggle.set_active(hide);
    }

    pub fn set_privilege_limited(&self, limited: bool) {
        self.imp().privilege_note.set_revealed(limited);
    }

    /// The selected backend, or `None` when nothing is selected — including
    /// after a refresh in which the selected backend exited.
    pub fn selected_session(&self) -> Option<Session> {
        self.imp()
            .table
            .borrow()
            .as_ref()
            .and_then(|table| table.selected())
            .map(|row| (*row).clone())
    }

    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) {
        if let Some(table) = self.imp().table.borrow().as_ref() {
            table.connect_selection_changed(f);
        }
    }

    /// Shows why the buttons are unavailable when the role cannot signal.
    ///
    /// A label rather than only a tooltip: GTK does not deliver tooltips to
    /// insensitive widgets, so a tooltip alone would be invisible in exactly
    /// the case it exists for. The tooltip set in the Blueprint still serves
    /// the sensitive case.
    pub fn set_capabilities(&self, capabilities: &Capabilities) {
        let imp = self.imp();
        imp.signal_reason.set_visible(!capabilities.signal_backend);
        imp.signal_reason.set_text(&i18n(
            "Cancelling and terminating backends requires membership of pg_signal_backend.",
        ));
    }

    pub fn update(&self, sessions: &[Session]) {
        if let Some(table) = self.imp().table.borrow().as_ref() {
            table.update(sessions);
        }
    }
}

impl Default for McpgSessionsPage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::compare_rows;
    use std::cmp::Ordering;

    fn session_with_pid(pid: i32) -> Session {
        Session {
            pid,
            user_name: None,
            application_name: None,
            client_addr: None,
            database: None,
            state: None,
            wait_event_type: None,
            wait_event: None,
            backend_type: None,
            query_duration_secs: None,
            query: None,
        }
    }

    #[test]
    fn pid_sorts_numerically_not_lexically() {
        let column = &COLUMNS[0];
        assert_eq!(column.title, "PID");

        let nine = session_with_pid(9);
        let ten = session_with_pid(10);

        assert_eq!(
            compare_rows(&nine, &ten, column.render, column.sort_key),
            Ordering::Less
        );
        // Guard against a regression to lexical sorting: as text, "10" < "9".
        assert_eq!(
            (column.render)(&ten).cmp(&(column.render)(&nine)),
            Ordering::Less
        );
    }

    #[test]
    fn duration_sorts_numerically_by_seconds() {
        let column = COLUMNS
            .iter()
            .find(|c| c.title == "Duration")
            .expect("the Duration column exists");

        let mut short = session_with_pid(1);
        let mut long = session_with_pid(2);
        short.query_duration_secs = Some(2.0);
        long.query_duration_secs = Some(10.0);

        assert_eq!(
            compare_rows(&short, &long, column.render, column.sort_key),
            Ordering::Less
        );
    }

    #[test]
    fn text_columns_sort_lexically() {
        let column = COLUMNS
            .iter()
            .find(|c| c.title == "User")
            .expect("the User column exists");

        let mut alice = session_with_pid(1);
        let mut bob = session_with_pid(2);
        alice.user_name = Some("alice".to_string());
        bob.user_name = Some("bob".to_string());

        assert_eq!(
            compare_rows(&alice, &bob, column.render, column.sort_key),
            Ordering::Less
        );
    }
}

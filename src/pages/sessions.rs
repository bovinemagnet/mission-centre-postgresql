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
use std::cmp::Ordering;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::prelude::{Cast, CastNone};
use gtk::{gio, glib};

use crate::collector::snapshot::Session;

glib::wrapper! {
    pub struct SessionObject(ObjectSubclass<session_object::SessionObject>);
}

mod session_object {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct SessionObject {
        pub session: RefCell<Option<Session>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SessionObject {
        const NAME: &'static str = "McpgSessionObject";
        type Type = super::SessionObject;
    }

    impl ObjectImpl for SessionObject {}
}

impl SessionObject {
    pub fn new(session: Session) -> Self {
        let object: Self = glib::Object::new();
        object.imp().session.replace(Some(session));
        object
    }

    pub fn session(&self) -> Session {
        self.imp()
            .session
            .borrow()
            .clone()
            .expect("SessionObject always holds a session")
    }
}

/// How a session renders as text in a column cell.
type Renderer = fn(&Session) -> String;
/// Extracts a numeric sort key from a session, for columns that must sort
/// numerically rather than lexically (so "10" sorts after "9").
type NumericKey = fn(&Session) -> f64;

/// Column definitions: title, how to render a session as text, and — for
/// columns whose values are numbers rather than words — how to extract a
/// numeric key so header clicks sort them numerically.
const COLUMNS: &[(&str, Renderer, Option<NumericKey>)] = &[
    ("PID", |s| s.pid.to_string(), Some(|s| s.pid as f64)),
    ("User", |s| s.user_name.clone().unwrap_or_default(), None),
    ("Database", |s| s.database.clone().unwrap_or_default(), None),
    (
        "Application",
        |s| s.application_name.clone().unwrap_or_default(),
        None,
    ),
    (
        "Client",
        |s| s.client_addr.clone().unwrap_or_else(|| "local".to_string()),
        None,
    ),
    ("State", |s| s.state.clone().unwrap_or_default(), None),
    (
        "Wait",
        |s| match (&s.wait_event_type, &s.wait_event) {
            (Some(kind), Some(event)) => format!("{kind}: {event}"),
            _ => String::new(),
        },
        None,
    ),
    (
        "Duration",
        |s| match s.query_duration_secs {
            Some(secs) if secs >= 1.0 => format!("{secs:.0}s"),
            Some(secs) => format!("{:.0}ms", secs * 1000.0),
            None => String::new(),
        },
        // A session with no running query has no duration; sort it as zero so
        // idle backends group at the short end rather than sorting by text.
        Some(|s| s.query_duration_secs.unwrap_or(0.0)),
    ),
    (
        "Query",
        |s| {
            s.query
                .clone()
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        },
        None,
    ),
];

/// Orders two sessions for a column. Numeric columns compare by their numeric
/// key; the rest compare lexically on the rendered text. Split out as a pure
/// function so the numeric-versus-lexical behaviour can be unit-tested without
/// a GTK widget in the loop.
fn compare_sessions(
    a: &Session,
    b: &Session,
    render: Renderer,
    numeric_key: Option<NumericKey>,
) -> Ordering {
    match numeric_key {
        Some(key) => key(a).partial_cmp(&key(b)).unwrap_or(Ordering::Equal),
        None => render(a).cmp(&render(b)),
    }
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

        pub store: RefCell<Option<gio::ListStore>>,
        pub filter: RefCell<Option<gtk::CustomFilter>>,
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
            self.obj().build_model();
            self.obj().build_columns();

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

    fn build_model(&self) {
        let imp = self.imp();
        let store = gio::ListStore::new::<SessionObject>();

        let page = self.clone();
        let filter = gtk::CustomFilter::new(move |object| {
            let session = object
                .downcast_ref::<SessionObject>()
                .expect("the model only holds SessionObject")
                .session();
            page.matches(&session)
        });

        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        // Incremental filtering plus rapid items-changed is the combination
        // implicated in the upstream GTK sort/filter crash; keep it off.
        filtered.set_incremental(false);

        let sorted = gtk::SortListModel::new(Some(filtered), self.imp().column_view.sorter());
        sorted.set_incremental(false);

        imp.column_view
            .set_model(Some(&gtk::NoSelection::new(Some(sorted))));
        imp.store.replace(Some(store));
        imp.filter.replace(Some(filter));
    }

    fn build_columns(&self) {
        let imp = self.imp();
        for (title, render, numeric_key) in COLUMNS {
            let render = *render;
            let numeric_key = *numeric_key;
            let factory = gtk::SignalListItemFactory::new();

            factory.connect_setup(|_, item| {
                let label = gtk::Label::new(None);
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                item.downcast_ref::<gtk::ListItem>()
                    .expect("a ListItem")
                    .set_child(Some(&label));
            });

            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("a ListItem");
                let label = item
                    .child()
                    .and_downcast::<gtk::Label>()
                    .expect("the child set in setup");
                let session = item
                    .item()
                    .and_downcast::<SessionObject>()
                    .expect("a SessionObject")
                    .session();
                let text = render(&session);
                label.set_tooltip_text(Some(&text));
                label.set_text(&text);
            });

            // Give the column a sorter so clicking its header sorts the table.
            // The `SortListModel` built in `build_model` already watches
            // `column_view.sorter()`, which tracks whichever column's sorter is
            // active.
            let sorter = gtk::CustomSorter::new(move |a, b| {
                let session_a = a
                    .downcast_ref::<SessionObject>()
                    .expect("the model only holds SessionObject")
                    .session();
                let session_b = b
                    .downcast_ref::<SessionObject>()
                    .expect("the model only holds SessionObject")
                    .session();
                compare_sessions(&session_a, &session_b, render, numeric_key).into()
            });

            let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
            column.set_resizable(true);
            column.set_expand(*title == "Query");
            column.set_sorter(Some(&sorter));
            imp.column_view.append_column(&column);
        }
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
        if let Some(filter) = self.imp().filter.borrow().as_ref() {
            filter.changed(gtk::FilterChange::Different);
        }
    }

    pub fn set_hide_idle(&self, hide: bool) {
        self.imp().hide_idle_toggle.set_active(hide);
    }

    pub fn set_privilege_limited(&self, limited: bool) {
        self.imp().privilege_note.set_revealed(limited);
    }

    pub fn update(&self, sessions: &[Session]) {
        let imp = self.imp();
        let Some(store) = imp.store.borrow().clone() else {
            return;
        };
        // Replacing the contents in one splice keeps items-changed to a single
        // emission per sample rather than one per row.
        let objects: Vec<SessionObject> =
            sessions.iter().cloned().map(SessionObject::new).collect();
        store.splice(0, store.n_items(), &objects);
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
        let (title, render, numeric_key) = COLUMNS[0];
        assert_eq!(title, "PID");

        let nine = session_with_pid(9);
        let ten = session_with_pid(10);

        // Numerically 9 < 10, so the comparator must order nine before ten.
        assert_eq!(
            compare_sessions(&nine, &ten, render, numeric_key),
            Ordering::Less
        );
        // Guard against a regression to lexical sorting: as text, "10" < "9",
        // which is the wrong order this numeric key exists to prevent.
        assert_eq!(render(&ten).cmp(&render(&nine)), Ordering::Less);
    }

    #[test]
    fn duration_sorts_numerically_by_seconds() {
        let mut short = session_with_pid(1);
        let mut long = session_with_pid(2);
        short.query_duration_secs = Some(2.0);
        long.query_duration_secs = Some(10.0);

        let numeric_key: Option<NumericKey> = Some(|s| s.query_duration_secs.unwrap_or(0.0));
        let render: Renderer = |_| String::new();
        assert_eq!(
            compare_sessions(&short, &long, render, numeric_key),
            Ordering::Less
        );
    }

    #[test]
    fn text_columns_sort_lexically() {
        let mut alice = session_with_pid(1);
        let mut bob = session_with_pid(2);
        alice.user_name = Some("alice".to_string());
        bob.user_name = Some("bob".to_string());

        let render: Renderer = |s| s.user_name.clone().unwrap_or_default();
        assert_eq!(compare_sessions(&alice, &bob, render, None), Ordering::Less);
    }
}

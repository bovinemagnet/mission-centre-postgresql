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

/// Column definitions: title, and how to render a session as text.
const COLUMNS: &[(&str, fn(&Session) -> String)] = &[
    ("PID", |s| s.pid.to_string()),
    ("User", |s| s.user_name.clone().unwrap_or_default()),
    ("Database", |s| s.database.clone().unwrap_or_default()),
    ("Application", |s| {
        s.application_name.clone().unwrap_or_default()
    }),
    ("Client", |s| {
        s.client_addr.clone().unwrap_or_else(|| "local".to_string())
    }),
    ("State", |s| s.state.clone().unwrap_or_default()),
    ("Wait", |s| match (&s.wait_event_type, &s.wait_event) {
        (Some(kind), Some(event)) => format!("{kind}: {event}"),
        _ => String::new(),
    }),
    ("Duration", |s| match s.query_duration_secs {
        Some(secs) if secs >= 1.0 => format!("{secs:.0}s"),
        Some(secs) => format!("{:.0}ms", secs * 1000.0),
        None => String::new(),
    }),
    ("Query", |s| {
        s.query
            .clone()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }),
];

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
        for (title, render) in COLUMNS {
            let render = *render;
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

            let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
            column.set_resizable(true);
            column.set_expand(*title == "Query");
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

/* dialogs/add_server.rs
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
use gtk::glib;
use uuid::Uuid;

use crate::connection::credentials;
use crate::connection::params::{ConnectionParams, SslMode};

type AddedCallback = Box<dyn Fn(&ConnectionParams)>;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/add_server_dialog.ui")]
    pub struct McpgAddServerDialog {
        #[template_child]
        pub label_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub host_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub port_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub database_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub user_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub password_row: TemplateChild<adw::PasswordEntryRow>,
        #[template_child]
        pub ssl_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,

        pub on_added: RefCell<Option<AddedCallback>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgAddServerDialog {
        const NAME: &'static str = "McpgAddServerDialog";
        type Type = super::McpgAddServerDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgAddServerDialog {
        fn constructed(&self) {
            self.parent_constructed();

            self.host_row.set_text("localhost");
            self.port_row.set_text("5432");
            self.database_row.set_text("postgres");

            let dialog = self.obj().clone();
            self.cancel_button.connect_clicked(move |_| {
                dialog.close();
            });

            let dialog = self.obj().clone();
            self.add_button.connect_clicked(move |_| dialog.submit());
        }
    }

    impl WidgetImpl for McpgAddServerDialog {}
    impl AdwDialogImpl for McpgAddServerDialog {}
}

glib::wrapper! {
    pub struct McpgAddServerDialog(ObjectSubclass<imp::McpgAddServerDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl McpgAddServerDialog {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn connect_added<F: Fn(&ConnectionParams) + 'static>(&self, callback: F) {
        self.imp().on_added.replace(Some(Box::new(callback)));
    }

    fn submit(&self) {
        let imp = self.imp();

        let host = imp.host_row.text().trim().to_string();
        if host.is_empty() {
            imp.host_row.add_css_class("error");
            return;
        }
        imp.host_row.remove_css_class("error");

        let port: u16 = match imp.port_row.text().trim().parse() {
            Ok(port) => port,
            Err(_) => {
                imp.port_row.add_css_class("error");
                return;
            }
        };
        imp.port_row.remove_css_class("error");

        let label = imp.label_row.text().trim().to_string();
        let label = if label.is_empty() {
            format!("{host}:{port}")
        } else {
            label
        };

        let params = ConnectionParams {
            id: Uuid::new_v4(),
            label,
            host,
            port,
            database: imp.database_row.text().trim().to_string(),
            user: imp.user_row.text().trim().to_string(),
            ssl_mode: match imp.ssl_row.selected() {
                0 => SslMode::Disable,
                2 => SslMode::Require,
                _ => SslMode::Prefer,
            },
        };

        // The password goes straight to the secret store and is never held on
        // ConnectionParams, which is serialised into GSettings.
        let password = imp.password_row.text();
        if let Err(e) = credentials::store_password(&params.id, &password) {
            gtk::glib::g_warning!("mission-centre-pg", "could not store the password: {e}");
        }

        if let Some(callback) = imp.on_added.borrow().as_ref() {
            callback(&params);
        }
        self.close();
    }
}

impl Default for McpgAddServerDialog {
    fn default() -> Self {
        Self::new()
    }
}

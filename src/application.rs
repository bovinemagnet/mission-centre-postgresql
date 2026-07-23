/* application.rs
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
use gtk::{gio, glib};

use crate::window::MissionCentrePgWindow;

pub const APP_ID: &str = "io.github.paulsnow.MissionCentrePg";

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MissionCentrePgApplication;

    #[glib::object_subclass]
    impl ObjectSubclass for MissionCentrePgApplication {
        const NAME: &'static str = "MissionCentrePgApplication";
        type Type = super::MissionCentrePgApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for MissionCentrePgApplication {}

    impl ApplicationImpl for MissionCentrePgApplication {
        fn activate(&self) {
            let app = self.obj();
            let window = app
                .active_window()
                .unwrap_or_else(|| MissionCentrePgWindow::new(&*app).upcast());
            window.present();
        }
    }

    impl GtkApplicationImpl for MissionCentrePgApplication {}
    impl AdwApplicationImpl for MissionCentrePgApplication {}
}

glib::wrapper! {
    pub struct MissionCentrePgApplication(ObjectSubclass<imp::MissionCentrePgApplication>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl MissionCentrePgApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("flags", gio::ApplicationFlags::empty())
            .build()
    }

    pub fn settings(&self) -> gio::Settings {
        gio::Settings::new(APP_ID)
    }
}

impl Default for MissionCentrePgApplication {
    fn default() -> Self {
        Self::new()
    }
}

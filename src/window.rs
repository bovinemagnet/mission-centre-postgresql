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

use adw::subclass::prelude::*;
use gtk::prelude::IsA;
use gtk::{gio, glib};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/window.ui")]
    pub struct MissionCentrePgWindow;

    #[glib::object_subclass]
    impl ObjectSubclass for MissionCentrePgWindow {
        const NAME: &'static str = "MissionCentrePgWindow";
        type Type = super::MissionCentrePgWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MissionCentrePgWindow {}
    impl WidgetImpl for MissionCentrePgWindow {}
    impl WindowImpl for MissionCentrePgWindow {}
    impl ApplicationWindowImpl for MissionCentrePgWindow {}
    impl AdwApplicationWindowImpl for MissionCentrePgWindow {}
}

glib::wrapper! {
    pub struct MissionCentrePgWindow(ObjectSubclass<imp::MissionCentrePgWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::ConstraintTarget, gtk::Accessible,
                    gtk::Buildable, gtk::ShortcutManager, gtk::Root, gtk::Native;
}

impl MissionCentrePgWindow {
    pub fn new(app: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder().property("application", app).build()
    }
}

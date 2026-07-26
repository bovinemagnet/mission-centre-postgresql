/* main.rs
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

mod application;
mod window;
mod window_actions;

use gtk::prelude::*;
use gtk::{gio, glib};

use application::MissionCentrePgApplication;

fn main() -> glib::ExitCode {
    let resource_dir = std::env::var("MCPG_RESOURCE_DIR")
        .unwrap_or_else(|_| "/usr/share/mission-centre-pg".to_string());
    let resource_path = format!("{resource_dir}/mission-centre-pg.gresource");

    let resources = gio::Resource::load(&resource_path)
        .unwrap_or_else(|e| panic!("Failed to load resources from {resource_path}: {e}"));
    gio::resources_register(&resources);

    MissionCentrePgApplication::new().run()
}

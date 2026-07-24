/* widgets/sidebar_row.rs
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

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::connection::params::HistoryMode;
use crate::widgets::graph_widget::GraphWidget;
use crate::widgets::graph_widget_utils::DatasetGroup;

/// Points retained in a sidebar sparkline. Deliberately shorter than the
/// full pages' graphs — the row is 72px wide.
const SPARKLINE_POINTS: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

impl ConnectionState {
    fn icon_name(self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "media-playback-stop-symbolic",
            ConnectionState::Connecting => "content-loading-symbolic",
            ConnectionState::Connected => "media-record-symbolic",
            ConnectionState::Failed => "dialog-warning-symbolic",
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/sidebar_row.ui")]
    pub struct McpgSidebarRow {
        #[template_child]
        pub state_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub heading_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub subheading_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub graph: TemplateChild<GraphWidget>,
        #[template_child]
        pub history_menu_button: TemplateChild<gtk::MenuButton>,

        /// The stateful "history-mode" action backing the menu's tick marks.
        /// Stored so `set_history_mode` can update which item is ticked
        /// without re-fetching the action group off the widget.
        pub history_action: RefCell<Option<gio::SimpleAction>>,
        /// Invoked when the user picks a mode from the menu. The window wires
        /// this to save the choice and restart the collector.
        pub on_history_change: RefCell<Option<Box<dyn Fn(HistoryMode)>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgSidebarRow {
        const NAME: &'static str = "McpgSidebarRow";
        type Type = super::McpgSidebarRow;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            GraphWidget::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgSidebarRow {
        fn constructed(&self) {
            self.parent_constructed();
            self.graph.set_data_points(SPARKLINE_POINTS);
            self.graph.add_dataset(DatasetGroup::new());
            // Accent blue, so the sparkline reads against the dark sidebar
            // rather than drawing in the default invisible black.
            self.graph
                .set_base_color(gdk::RGBA::new(0.30, 0.56, 0.90, 1.0));

            // A GtkListBox row is transient, so the menu is modelled with a
            // stateful action taking a string parameter rather than one action
            // per mode. Selecting an item sets the action state (moving the
            // tick) and emits the chosen mode through `on_history_change`.
            let menu = gio::Menu::new();
            menu.append(
                Some(&crate::i18n::i18n("Off")),
                Some("row.history-mode::off"),
            );
            menu.append(
                Some(&crate::i18n::i18n("Local")),
                Some("row.history-mode::local"),
            );
            menu.append(
                Some(&crate::i18n::i18n("pgconsole")),
                Some("row.history-mode::pgconsole"),
            );
            self.history_menu_button.set_menu_model(Some(&menu));

            let group = gio::SimpleActionGroup::new();
            let action = gio::SimpleAction::new_stateful(
                "history-mode",
                Some(glib::VariantTy::STRING),
                &"off".to_variant(),
            );
            let row = self.obj().clone();
            action.connect_activate(move |action, param| {
                let Some(value) = param.and_then(|p| p.str().map(str::to_owned)) else {
                    return;
                };
                action.set_state(&value.to_variant());
                let mode = match value.as_str() {
                    "local" => HistoryMode::Local,
                    "pgconsole" => HistoryMode::PgConsole,
                    _ => HistoryMode::Off,
                };
                if let Some(cb) = row.imp().on_history_change.borrow().as_ref() {
                    cb(mode);
                }
            });
            self.history_action.replace(Some(action.clone()));
            group.add_action(&action);
            self.obj().insert_action_group("row", Some(&group));
        }
    }

    impl WidgetImpl for McpgSidebarRow {}
    impl BoxImpl for McpgSidebarRow {}
}

glib::wrapper! {
    pub struct McpgSidebarRow(ObjectSubclass<imp::McpgSidebarRow>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgSidebarRow {
    pub fn new(heading: &str) -> Self {
        let row: Self = glib::Object::new();
        row.set_heading(heading);
        row
    }

    pub fn set_heading(&self, text: &str) {
        self.imp().heading_label.set_text(text);
    }

    pub fn set_subheading(&self, text: &str) {
        self.imp().subheading_label.set_text(text);
    }

    pub fn set_state(&self, state: ConnectionState) {
        self.imp().state_icon.set_icon_name(Some(state.icon_name()));
    }

    /// Append one point to the sparkline.
    pub fn push_value(&self, value: f64) {
        self.imp().graph.add_data_point(vec![vec![value as f32]]);
    }

    /// Drop the series so selecting a different server does not inherit the
    /// previous one's shape.
    pub fn reset_series(&self) {
        let graph = self.imp().graph.get();
        graph.clear_datasets();
        graph.add_dataset(DatasetGroup::new());
    }

    /// Tick the menu item matching `mode` without invoking the change
    /// callback. Called when the row is built to reflect the stored mode.
    pub fn set_history_mode(&self, mode: HistoryMode) {
        let value = match mode {
            HistoryMode::Off => "off",
            HistoryMode::Local => "local",
            HistoryMode::PgConsole => "pgconsole",
        };
        if let Some(action) = self.imp().history_action.borrow().as_ref() {
            action.set_state(&value.to_variant());
        }
    }

    /// Register the callback invoked when the user picks a mode from the menu.
    pub fn connect_history_change<F: Fn(HistoryMode) + 'static>(&self, callback: F) {
        self.imp()
            .on_history_change
            .replace(Some(Box::new(callback)));
    }
}

impl Default for McpgSidebarRow {
    fn default() -> Self {
        Self::new("")
    }
}

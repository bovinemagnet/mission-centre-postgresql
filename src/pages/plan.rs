/* pages/plan.rs
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

use crate::collector::statements::StatementKey;
use crate::explain::{flatten, render_text, PlanNode, PlanRow, GENERIC_PLAN_VERSION};
use crate::i18n::{i18n, i18n_f};
use crate::table::{Column, Table};

/// What the page is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanState {
    Empty,
    Pending,
    Failed,
    Shown,
}

/// Which stack page to show.
///
/// The version gate outranks everything: a server that cannot explain at all
/// should say so rather than invite a right-click that will never work, and
/// should keep saying so even after a failed attempt.
pub fn state_for(state: PlanState, version_num: i32) -> &'static str {
    if version_num < GENERIC_PLAN_VERSION {
        return "unsupported";
    }
    match state {
        PlanState::Empty => "empty",
        PlanState::Pending => "pending",
        PlanState::Failed => "failed",
        PlanState::Shown => "plan",
    }
}

/// A node's label, indented by its depth so the tree's shape is visible in a
/// flat column view — the same approach the Locks page takes.
pub fn node_label(node: &PlanNode, depth: usize) -> String {
    let indent = "    ".repeat(depth);
    match node.relation.as_deref() {
        Some(relation) => format!("{indent}{} on {relation}", node.node_type),
        None => format!("{indent}{}", node.node_type),
    }
}

/// The statement, collapsed onto one line so a formatted query does not push
/// the plan off the page.
pub fn one_line(statement: &str) -> String {
    statement.split_whitespace().collect::<Vec<_>>().join(" ")
}

const COLUMNS: &[Column<PlanRow>] = &[
    Column {
        title: "Node",
        render: |row| node_label(&row.node, row.depth),
        sort_key: None,
        expand: true,
    },
    Column {
        title: "Total cost",
        render: |row| format!("{:.2}", row.node.total_cost),
        sort_key: Some(|row| row.node.total_cost),
        expand: false,
    },
    Column {
        title: "Startup cost",
        render: |row| format!("{:.2}", row.node.startup_cost),
        sort_key: Some(|row| row.node.startup_cost),
        expand: false,
    },
    Column {
        title: "Rows",
        render: |row| row.node.rows.to_string(),
        sort_key: Some(|row| row.node.rows as f64),
        expand: false,
    },
    Column {
        title: "Width",
        render: |row| row.node.width.to_string(),
        sort_key: Some(|row| row.node.width as f64),
        expand: false,
    },
];

/// Depth and node type alone are not unique — a plan can hold two identical
/// scans at the same depth — so the position in the flattened list completes
/// the key.
fn plan_key(row: &PlanRow) -> String {
    format!("{}:{}", row.depth, row.node.node_type)
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/plan_page.ui")]
    pub struct McpgPlanPage {
        #[template_child]
        pub statement_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub taken_at_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub state_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub failed_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub view_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub tree_view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub text_view: TemplateChild<gtk::TextView>,

        pub table: RefCell<Option<Table<PlanRow>>>,
        pub version_num: Cell<i32>,
        /// The statement a plan is expected for. A result carrying any other
        /// key belongs to a request the user has moved on from.
        pub pending_key: RefCell<Option<StatementKey>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgPlanPage {
        const NAME: &'static str = "McpgPlanPage";
        type Type = super::McpgPlanPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgPlanPage {
        fn constructed(&self) {
            self.parent_constructed();

            let table = Table::attach(&self.tree_view.get(), COLUMNS, |_| true, plan_key);
            self.table.replace(Some(table));
        }
    }

    impl WidgetImpl for McpgPlanPage {}
    impl BoxImpl for McpgPlanPage {}
}

glib::wrapper! {
    pub struct McpgPlanPage(ObjectSubclass<imp::McpgPlanPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgPlanPage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_version(&self, version_num: i32) {
        self.imp().version_num.set(version_num);
        self.apply_state(PlanState::Empty);
    }

    /// True when this server can explain a captured statement at all.
    pub fn is_supported(&self) -> bool {
        self.imp().version_num.get() >= GENERIC_PLAN_VERSION
    }

    pub fn pending_key(&self) -> Option<StatementKey> {
        self.imp().pending_key.borrow().clone()
    }

    /// Called when a request is sent, so the page says something is happening
    /// rather than showing the previous plan as though it were the new one.
    pub fn show_pending(&self, key: StatementKey, statement: &str) {
        let imp = self.imp();
        imp.pending_key.replace(Some(key));
        imp.statement_label.set_text(&one_line(statement));
        imp.taken_at_label.set_text(&i18n("Asking the server…"));
        self.apply_state(PlanState::Pending);
    }

    pub fn show_plan(&self, plan: &PlanNode, taken_at: &str) {
        let imp = self.imp();

        let rows = flatten(plan);
        if let Some(table) = imp.table.borrow().as_ref() {
            table.update(&rows);
        }
        imp.text_view.buffer().set_text(&render_text(plan));
        imp.taken_at_label
            .set_text(&i18n_f("Planned at {}", &[taken_at]));
        self.apply_state(PlanState::Shown);
    }

    /// The server's own message, verbatim. A plan is a diagnostic tool, and a
    /// generic failure in a diagnostic tool is worse than useless.
    pub fn show_error(&self, message: &str) {
        let imp = self.imp();
        imp.failed_page.set_description(Some(message));
        imp.taken_at_label.set_text("");
        self.apply_state(PlanState::Failed);
    }

    /// Drops the plan from the previously selected server, so a plan taken
    /// against one server cannot appear to describe another.
    pub fn clear(&self) {
        let imp = self.imp();
        imp.pending_key.replace(None);
        imp.statement_label.set_text("");
        imp.taken_at_label.set_text("");
        self.apply_state(PlanState::Empty);
    }

    fn apply_state(&self, state: PlanState) {
        let imp = self.imp();
        imp.state_stack
            .set_visible_child_name(state_for(state, imp.version_num.get()));
    }
}

impl Default for McpgPlanPage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(node_type: &str, relation: Option<&str>) -> PlanNode {
        PlanNode {
            node_type: node_type.to_string(),
            relation: relation.map(str::to_string),
            startup_cost: 0.0,
            total_cost: 1.0,
            rows: 1,
            width: 8,
            children: Vec::new(),
        }
    }

    #[test]
    fn an_old_server_says_so_whatever_else_has_happened() {
        for state in [
            PlanState::Empty,
            PlanState::Pending,
            PlanState::Failed,
            PlanState::Shown,
        ] {
            assert_eq!(state_for(state, 150000), "unsupported");
        }
    }

    #[test]
    fn the_state_follows_what_is_known_on_a_supported_server() {
        assert_eq!(state_for(PlanState::Empty, 160000), "empty");
        assert_eq!(state_for(PlanState::Pending, 160000), "pending");
        assert_eq!(state_for(PlanState::Failed, 160000), "failed");
        assert_eq!(state_for(PlanState::Shown, 180000), "plan");
    }

    #[test]
    fn a_node_is_labelled_with_its_relation_when_it_has_one() {
        assert_eq!(
            node_label(&node("Seq Scan", Some("orders")), 0),
            "Seq Scan on orders"
        );
        assert_eq!(node_label(&node("Aggregate", None), 0), "Aggregate");
    }

    #[test]
    fn depth_shows_as_indentation() {
        let deep = node_label(&node("Seq Scan", Some("orders")), 2);
        assert!(deep.starts_with("        "), "expected indent: {deep:?}");
        assert!(deep.trim_start().starts_with("Seq Scan"));
    }

    #[test]
    fn a_formatted_statement_is_collapsed_onto_one_line() {
        assert_eq!(
            one_line("SELECT *\n  FROM orders\n  WHERE id = $1"),
            "SELECT * FROM orders WHERE id = $1"
        );
    }
}

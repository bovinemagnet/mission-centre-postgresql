/* pages/locks.rs
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

use crate::collector::locks::{
    build_forest, LockEntry, LockInventorySample, LockNode, LockParticipant, LocksSample,
};
use crate::collector::snapshot::Session;
use crate::collector::worker::CollectorError;
use crate::connection::probe::Capabilities;
use crate::i18n::i18n;
use crate::table::{Column, Table};

/// One rendered line: a participant plus how deep it sits, so the first
/// column can indent it. `ColumnView` has no tree mode in this codebase's
/// usage, so the forest is flattened depth-first and depth carries the shape.
#[derive(Debug, Clone, PartialEq)]
pub struct LockRow {
    pub depth: usize,
    pub participant: LockParticipant,
    pub in_cycle: bool,
    pub is_stub: bool,
}

pub fn flatten(forest: &[LockNode]) -> Vec<LockRow> {
    let mut rows = Vec::new();
    for node in forest {
        push_node(node, 0, &mut rows);
    }
    rows
}

fn push_node(node: &LockNode, depth: usize, rows: &mut Vec<LockRow>) {
    rows.push(LockRow {
        depth,
        participant: node.participant.clone(),
        in_cycle: node.in_cycle,
        is_stub: node.is_stub,
    });
    for child in &node.children {
        push_node(child, depth + 1, rows);
    }
}

/// Whether the action bar may act on this row.
///
/// A stub row's backend was already gone when the sample was taken, so
/// signalling it could only ever report "no longer running". Saying so up
/// front is better than offering an action that cannot succeed.
pub fn actions_available(row: &LockRow, signal_allowed: bool) -> bool {
    signal_allowed && !row.is_stub
}

const COLUMNS: &[Column<LockRow>] = &[
    Column {
        title: "Blocked",
        render: |row| {
            let indent = "    ".repeat(row.depth);
            let suffix = if row.in_cycle {
                " (cycle)"
            } else if row.is_stub {
                " (gone)"
            } else {
                ""
            };
            format!("{indent}{}{suffix}", row.participant.pid)
        },
        sort_key: None,
        expand: false,
    },
    Column {
        title: "User",
        render: |row| row.participant.user_name.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Database",
        render: |row| row.participant.database.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "State",
        render: |row| row.participant.state.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Waiting",
        render: |row| match row.participant.wait_secs {
            Some(secs) if secs >= 1.0 => format!("{secs:.0}s"),
            Some(secs) => format!("{:.0}ms", secs * 1000.0),
            None => "—".to_string(),
        },
        sort_key: Some(|row| row.participant.wait_secs.unwrap_or(0.0)),
        expand: false,
    },
    Column {
        title: "Lock",
        render: |row| row.participant.lock_mode.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Object",
        render: |row| row.participant.relation.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Query",
        render: |row| {
            row.participant
                .query
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

/// The tree is ordered, so a row keys on its position as well as its pid: the
/// same backend can appear under two different blockers.
fn lock_key(row: &LockRow) -> String {
    format!("{}:{}", row.depth, row.participant.pid)
}

const INVENTORY_COLUMNS: &[Column<LockEntry>] = &[
    Column {
        title: "PID",
        render: |entry| entry.pid.map(|pid| pid.to_string()).unwrap_or_default(),
        sort_key: Some(|entry| entry.pid.unwrap_or(0) as f64),
        expand: false,
    },
    Column {
        title: "Type",
        render: |entry| entry.lock_type.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Mode",
        render: |entry| entry.mode.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Granted",
        render: |entry| {
            if entry.granted {
                "yes".to_string()
            } else {
                "waiting".to_string()
            }
        },
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Object",
        render: |entry| entry.relation.clone().unwrap_or_default(),
        sort_key: None,
        expand: true,
    },
    Column {
        title: "User",
        render: |entry| entry.user_name.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Database",
        render: |entry| entry.database.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
];

fn inventory_key(entry: &LockEntry) -> String {
    format!(
        "{}:{}:{}",
        entry.pid.unwrap_or(0),
        entry.lock_type.as_deref().unwrap_or(""),
        entry.relation.as_deref().unwrap_or("")
    )
}

/// The truncation notice, or `None` when the whole inventory is on screen.
/// Pure so the wording can be asserted without a live server.
pub fn truncation_notice(shown: usize, total: i64) -> Option<String> {
    (total > shown as i64).then(|| format!("Showing {shown} of {total} locks"))
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/locks_page.ui")]
    pub struct McpgLocksPage {
        #[template_child]
        pub view_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub tree_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub column_view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub inventory_view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub truncation_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub unavailable_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub signal_reason: TemplateChild<gtk::Label>,
        #[template_child]
        pub cancel_backend_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub terminate_backend_button: TemplateChild<gtk::Button>,

        pub table: RefCell<Option<Table<LockRow>>>,
        pub inventory_table: RefCell<Option<Table<LockEntry>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgLocksPage {
        const NAME: &'static str = "McpgLocksPage";
        type Type = super::McpgLocksPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgLocksPage {
        fn constructed(&self) {
            self.parent_constructed();

            let table = Table::attach(&self.column_view.get(), COLUMNS, |_| true, lock_key);
            self.table.replace(Some(table));

            let inventory = Table::attach(
                &self.inventory_view.get(),
                INVENTORY_COLUMNS,
                |_| true,
                inventory_key,
            );
            self.inventory_table.replace(Some(inventory));
        }
    }

    impl WidgetImpl for McpgLocksPage {}
    impl BoxImpl for McpgLocksPage {}
}

glib::wrapper! {
    pub struct McpgLocksPage(ObjectSubclass<imp::McpgLocksPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgLocksPage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// The selected backend's pid, or `None` when nothing is selected or the
    /// selected row is a stub whose backend has already gone.
    pub fn selected_pid(&self) -> Option<i32> {
        self.imp()
            .table
            .borrow()
            .as_ref()
            .and_then(|table| table.selected())
            .filter(|row| !row.is_stub)
            .map(|row| row.participant.pid)
    }

    /// The selected backend as a `Session`, so the confirmation dialog and the
    /// action machinery are shared with the Sessions page rather than
    /// duplicated. A lock participant carries the same identifying fields.
    pub fn selected_session(&self) -> Option<Session> {
        let row = self
            .imp()
            .table
            .borrow()
            .as_ref()
            .and_then(|table| table.selected())
            .filter(|row| !row.is_stub)?;

        Some(Session {
            pid: row.participant.pid,
            user_name: row.participant.user_name.clone(),
            application_name: None,
            client_addr: None,
            database: row.participant.database.clone(),
            state: row.participant.state.clone(),
            wait_event_type: None,
            wait_event: None,
            backend_type: None,
            query_duration_secs: row.participant.wait_secs,
            query: row.participant.query.clone(),
        })
    }

    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) {
        if let Some(table) = self.imp().table.borrow().as_ref() {
            table.connect_selection_changed(f);
        }
    }

    /// Reports whether the inventory view is on screen, both now and whenever
    /// it changes. The collector uses this to decide whether to run the
    /// expensive inventory query at all.
    pub fn connect_inventory_visibility(&self, f: impl Fn(bool) + 'static) {
        let stack = self.imp().view_stack.get();
        f(stack.visible_child_name().as_deref() == Some("inventory"));
        stack.connect_visible_child_name_notify(move |stack| {
            f(stack.visible_child_name().as_deref() == Some("inventory"));
        });
    }

    /// `None` means the view is not on screen and nothing was sampled, which
    /// leaves the table as it was rather than reporting a failure.
    pub fn update_inventory(
        &self,
        inventory: Option<&Result<LockInventorySample, CollectorError>>,
    ) {
        let imp = self.imp();

        let sample = match inventory {
            None => return,
            Some(Err(error)) => {
                imp.truncation_label.set_text(&i18n(&error.to_string()));
                imp.truncation_label.set_visible(true);
                return;
            }
            Some(Ok(sample)) => sample,
        };

        if let Some(table) = imp.inventory_table.borrow().as_ref() {
            table.update(&sample.locks);
        }

        match truncation_notice(sample.locks.len(), sample.total) {
            Some(notice) => {
                imp.truncation_label.set_text(&i18n(&notice));
                imp.truncation_label.set_visible(true);
            }
            None => imp.truncation_label.set_visible(false),
        }
    }

    /// Shows why the buttons are unavailable when the role cannot signal.
    /// A label rather than only a tooltip, because GTK does not deliver
    /// tooltips to insensitive widgets.
    pub fn set_capabilities(&self, capabilities: &Capabilities) {
        let imp = self.imp();
        imp.signal_reason.set_visible(!capabilities.signal_backend);
        imp.signal_reason.set_text(&i18n(
            "Cancelling and terminating backends requires membership of pg_signal_backend.",
        ));
    }

    /// `None` means no data this tick and leaves the page as it was. `Err`
    /// shows the server's own message, which is what makes a failure
    /// diagnosable rather than a dead end.
    pub fn update(&self, locks: Option<&Result<LocksSample, CollectorError>>) {
        let imp = self.imp();

        let sample = match locks {
            None => return,
            Some(Err(error)) => {
                imp.unavailable_page
                    .set_description(Some(&error.to_string()));
                imp.tree_stack.set_visible_child_name("unavailable");
                return;
            }
            Some(Ok(sample)) => sample,
        };

        let rows = flatten(&build_forest(&sample.participants));

        if let Some(table) = imp.table.borrow().as_ref() {
            table.update(&rows);
        }

        imp.tree_stack
            .set_visible_child_name(if rows.is_empty() { "empty" } else { "tree" });
    }
}

impl Default for McpgLocksPage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::locks::stub_participant;

    fn participant(pid: i32, blocked_by: &[i32]) -> LockParticipant {
        LockParticipant {
            blocked_by: blocked_by.to_vec(),
            ..stub_participant(pid)
        }
    }

    #[test]
    fn an_empty_forest_flattens_to_nothing() {
        assert!(flatten(&[]).is_empty());
    }

    #[test]
    fn a_chain_flattens_depth_first_with_increasing_depth() {
        let forest = build_forest(&[participant(100, &[]), participant(200, &[100])]);
        let rows = flatten(&forest);

        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].participant.pid, rows[0].depth), (100, 0));
        assert_eq!((rows[1].participant.pid, rows[1].depth), (200, 1));
    }

    #[test]
    fn siblings_share_a_depth_and_follow_their_parent() {
        let forest = build_forest(&[
            participant(100, &[]),
            participant(200, &[100]),
            participant(300, &[100]),
        ]);
        let rows = flatten(&forest);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 1);
    }

    #[test]
    fn a_stub_row_offers_no_actions_because_its_backend_has_gone() {
        let forest = build_forest(&[participant(200, &[999])]);
        let rows = flatten(&forest);

        let stub = rows
            .iter()
            .find(|row| row.participant.pid == 999)
            .expect("the vanished blocker is rendered as a stub");
        assert!(!actions_available(stub, true));

        let waiter = rows
            .iter()
            .find(|row| row.participant.pid == 200)
            .expect("the waiter is still rendered");
        assert!(actions_available(waiter, true));
    }

    #[test]
    fn no_row_offers_actions_without_the_signal_capability() {
        let forest = build_forest(&[participant(100, &[]), participant(200, &[100])]);
        let rows = flatten(&forest);

        assert!(rows.iter().all(|row| !actions_available(row, false)));
    }

    #[test]
    fn the_whole_inventory_needs_no_truncation_notice() {
        assert_eq!(truncation_notice(500, 500), None);
        assert_eq!(truncation_notice(12, 12), None);
    }

    #[test]
    fn a_truncated_inventory_says_how_much_it_is_hiding() {
        assert_eq!(
            truncation_notice(500, 4312).as_deref(),
            Some("Showing 500 of 4312 locks")
        );
    }

    #[test]
    fn a_deeper_row_is_indented_further_than_its_parent() {
        let forest = build_forest(&[participant(100, &[]), participant(200, &[100])]);
        let rows = flatten(&forest);

        let render = COLUMNS
            .iter()
            .find(|column| column.title == "Blocked")
            .expect("the Blocked column exists")
            .render;

        assert!(!render(&rows[0]).starts_with(' '));
        assert!(render(&rows[1]).starts_with(' '));
    }
}

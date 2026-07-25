/* pages/relations.rs
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

use crate::collector::relations::{IndexStats, RelationsSample, TableStats};
use crate::i18n::{i18n, i18n_f};
use crate::pages::format::{format_bytes, format_rate, format_ratio};
use crate::table::{Column, Table};

fn render_dead_ratio(table: &TableStats) -> String {
    format_ratio(table.dead_tuple_ratio())
}

fn render_seq_ratio(table: &TableStats) -> String {
    format_ratio(table.sequential_scan_ratio())
}

/// A coarse "how long ago" rather than a timestamp: the question the column
/// answers is whether a vacuum happened recently, not exactly when.
fn render_last_vacuum(table: &TableStats) -> String {
    let Some(secs) = table.secs_since_vacuum else {
        return i18n("never");
    };
    if !secs.is_finite() || secs < 0.0 {
        return "—".to_string();
    }
    if secs < 60.0 {
        i18n("just now")
    } else if secs < 3_600.0 {
        i18n_f("{}m ago", &[&format!("{:.0}", secs / 60.0)])
    } else if secs < 86_400.0 {
        i18n_f("{}h ago", &[&format!("{:.0}", secs / 3_600.0)])
    } else {
        i18n_f("{}d ago", &[&format!("{:.0}", secs / 86_400.0)])
    }
}

fn render_kind(index: &IndexStats) -> String {
    let base = if index.is_primary {
        i18n("primary")
    } else if index.is_unique {
        i18n("unique")
    } else {
        i18n("index")
    };
    if index.is_valid {
        base
    } else {
        i18n_f("{} (invalid)", &[&base])
    }
}

const TABLE_COLUMNS: &[Column<TableStats>] = &[
    Column {
        title: "Schema",
        render: |t| t.schema_name.clone(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Table",
        render: |t| t.table_name.clone(),
        sort_key: None,
        expand: true,
    },
    Column {
        title: "Size",
        render: |t| format_bytes(t.total_bytes),
        sort_key: Some(|t| t.total_bytes as f64),
        expand: false,
    },
    Column {
        title: "Live",
        render: |t| format_rate(t.n_live_tup as f64),
        sort_key: Some(|t| t.n_live_tup as f64),
        expand: false,
    },
    Column {
        title: "Dead",
        render: |t| format_rate(t.n_dead_tup as f64),
        sort_key: Some(|t| t.n_dead_tup as f64),
        expand: false,
    },
    Column {
        title: "Dead %",
        render: render_dead_ratio,
        // A table with no tuples has no ratio; sort it as zero rather than
        // letting it wander into the middle of the ranking.
        sort_key: Some(|t| t.dead_tuple_ratio().unwrap_or(0.0)),
        expand: false,
    },
    Column {
        title: "Seq scans",
        render: |t| format_rate(t.seq_scan as f64),
        sort_key: Some(|t| t.seq_scan as f64),
        expand: false,
    },
    Column {
        title: "Index scans",
        render: |t| format_rate(t.idx_scan as f64),
        sort_key: Some(|t| t.idx_scan as f64),
        expand: false,
    },
    Column {
        title: "Seq %",
        render: render_seq_ratio,
        sort_key: Some(|t| t.sequential_scan_ratio().unwrap_or(0.0)),
        expand: false,
    },
    Column {
        title: "Inserts",
        render: |t| format_rate(t.n_tup_ins as f64),
        sort_key: Some(|t| t.n_tup_ins as f64),
        expand: false,
    },
    Column {
        title: "Updates",
        render: |t| format_rate(t.n_tup_upd as f64),
        sort_key: Some(|t| t.n_tup_upd as f64),
        expand: false,
    },
    Column {
        title: "Deletes",
        render: |t| format_rate(t.n_tup_del as f64),
        sort_key: Some(|t| t.n_tup_del as f64),
        expand: false,
    },
    Column {
        title: "Last vacuum",
        render: render_last_vacuum,
        sort_key: Some(|t| t.secs_since_vacuum.unwrap_or(f64::MAX)),
        expand: false,
    },
];

const INDEX_COLUMNS: &[Column<IndexStats>] = &[
    Column {
        title: "Schema",
        render: |i| i.schema_name.clone(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Table",
        render: |i| i.table_name.clone(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Index",
        render: |i| i.index_name.clone(),
        sort_key: None,
        expand: true,
    },
    Column {
        title: "Kind",
        render: render_kind,
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Size",
        render: |i| format_bytes(i.bytes),
        sort_key: Some(|i| i.bytes as f64),
        expand: false,
    },
    Column {
        title: "Scans",
        render: |i| format_rate(i.idx_scan as f64),
        sort_key: Some(|i| i.idx_scan as f64),
        expand: false,
    },
    Column {
        title: "Tuples read",
        render: |i| format_rate(i.idx_tup_read as f64),
        sort_key: Some(|i| i.idx_tup_read as f64),
        expand: false,
    },
    Column {
        title: "Tuples fetched",
        render: |i| format_rate(i.idx_tup_fetch as f64),
        sort_key: Some(|i| i.idx_tup_fetch as f64),
        expand: false,
    },
];

fn table_key(table: &TableStats) -> String {
    format!("{}.{}", table.schema_name, table.table_name)
}

fn index_key(index: &IndexStats) -> String {
    format!(
        "{}.{}.{}",
        index.schema_name, index.table_name, index.index_name
    )
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/relations_page.ui")]
    pub struct McpgRelationsPage {
        #[template_child]
        pub error_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub database_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub inner_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub tables_filter: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub tables_view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub indexes_filter: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub unused_only_toggle: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub indexes_view: TemplateChild<gtk::ColumnView>,

        pub tables: RefCell<Option<Table<TableStats>>>,
        pub indexes: RefCell<Option<Table<IndexStats>>>,
        pub tables_filter_text: RefCell<String>,
        pub indexes_filter_text: RefCell<String>,
        pub unused_only: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgRelationsPage {
        const NAME: &'static str = "McpgRelationsPage";
        type Type = super::McpgRelationsPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgRelationsPage {
        fn constructed(&self) {
            self.parent_constructed();

            let page = self.obj().clone();
            let tables = Table::attach(
                &self.tables_view.get(),
                TABLE_COLUMNS,
                move |table| page.table_matches(table),
                table_key,
            );
            self.tables.replace(Some(tables));

            let page = self.obj().clone();
            let indexes = Table::attach(
                &self.indexes_view.get(),
                INDEX_COLUMNS,
                move |index| page.index_matches(index),
                index_key,
            );
            self.indexes.replace(Some(indexes));

            let page = self.obj().clone();
            self.tables_filter.connect_search_changed(move |entry| {
                page.imp()
                    .tables_filter_text
                    .replace(entry.text().to_lowercase().to_string());
                if let Some(table) = page.imp().tables.borrow().as_ref() {
                    table.refilter();
                }
            });

            let page = self.obj().clone();
            self.indexes_filter.connect_search_changed(move |entry| {
                page.imp()
                    .indexes_filter_text
                    .replace(entry.text().to_lowercase().to_string());
                if let Some(table) = page.imp().indexes.borrow().as_ref() {
                    table.refilter();
                }
            });

            let page = self.obj().clone();
            self.unused_only_toggle.connect_toggled(move |button| {
                page.imp().unused_only.set(button.is_active());
                if let Some(table) = page.imp().indexes.borrow().as_ref() {
                    table.refilter();
                }
            });
        }
    }

    impl WidgetImpl for McpgRelationsPage {}
    impl BoxImpl for McpgRelationsPage {}
}

glib::wrapper! {
    pub struct McpgRelationsPage(ObjectSubclass<imp::McpgRelationsPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgRelationsPage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn table_matches(&self, table: &TableStats) -> bool {
        let needle = self.imp().tables_filter_text.borrow();
        if needle.is_empty() {
            return true;
        }
        table.schema_name.to_lowercase().contains(needle.as_str())
            || table.table_name.to_lowercase().contains(needle.as_str())
    }

    fn index_matches(&self, index: &IndexStats) -> bool {
        let imp = self.imp();

        if imp.unused_only.get() && !index.is_unused() {
            return false;
        }

        let needle = imp.indexes_filter_text.borrow();
        if needle.is_empty() {
            return true;
        }
        index.schema_name.to_lowercase().contains(needle.as_str())
            || index.table_name.to_lowercase().contains(needle.as_str())
            || index.index_name.to_lowercase().contains(needle.as_str())
    }

    /// These views report the connected database only, which the page says
    /// rather than leaving the user to infer it from numbers that look small.
    pub fn set_database(&self, database: &str) {
        self.imp().database_note.set_text(&i18n_f(
            "Statistics for the database {} only.",
            &[&database],
        ));
    }

    /// Drops the tables and indexes from the previously selected server.
    /// Called on a server switch, not on a reconnect: until the new server's
    /// first slow sample arrives, the old server's rows would otherwise be
    /// presented under the new connection with nothing to mark them stale —
    /// indefinitely, if that connection never succeeds.
    pub fn clear(&self) {
        let imp = self.imp();
        if let Some(table) = imp.tables.borrow().as_ref() {
            table.update(&[]);
        }
        if let Some(table) = imp.indexes.borrow().as_ref() {
            table.update(&[]);
        }
        imp.database_note.set_text("");
        imp.error_banner.set_revealed(false);
    }

    pub fn update(&self, sample: &RelationsSample) {
        let imp = self.imp();
        imp.error_banner.set_revealed(false);
        if let Some(table) = imp.tables.borrow().as_ref() {
            table.update(&sample.tables);
        }
        if let Some(table) = imp.indexes.borrow().as_ref() {
            table.update(&sample.indexes);
        }
    }

    /// The slow tier ran and failed. The previous contents stay on screen
    /// under the banner: stale numbers with an explanation beat an empty
    /// table with none.
    pub fn set_error(&self, message: &str) {
        let imp = self.imp();
        imp.error_banner.set_title(message);
        imp.error_banner.set_revealed(true);
    }
}

impl Default for McpgRelationsPage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_stats() -> TableStats {
        TableStats {
            schema_name: "public".to_string(),
            table_name: "orders".to_string(),
            seq_scan: 30,
            seq_tup_read: 900,
            idx_scan: 70,
            idx_tup_fetch: 140,
            n_tup_ins: 10,
            n_tup_upd: 5,
            n_tup_del: 2,
            n_live_tup: 750,
            n_dead_tup: 250,
            secs_since_vacuum: Some(3_600.0),
            total_bytes: 1_048_576,
            can_maintain: true,
        }
    }

    fn index_stats() -> IndexStats {
        IndexStats {
            schema_name: "public".to_string(),
            table_name: "orders".to_string(),
            index_name: "orders_note_idx".to_string(),
            idx_scan: 0,
            idx_tup_read: 0,
            idx_tup_fetch: 0,
            bytes: 8_192,
            is_primary: false,
            is_unique: false,
            is_valid: true,
        }
    }

    #[test]
    fn a_table_with_no_tuples_shows_a_dash_not_zero_percent() {
        let mut empty = table_stats();
        empty.n_live_tup = 0;
        empty.n_dead_tup = 0;
        assert_eq!(render_dead_ratio(&empty), "—");
        assert_eq!(render_dead_ratio(&table_stats()), "25.0%");
    }

    #[test]
    fn a_never_vacuumed_table_says_so_rather_than_showing_a_duration() {
        let mut never = table_stats();
        never.secs_since_vacuum = None;
        assert_eq!(render_last_vacuum(&never), "never");
        assert_eq!(render_last_vacuum(&table_stats()), "1h ago");
    }

    #[test]
    fn a_plain_index_is_described_as_an_index() {
        assert_eq!(render_kind(&index_stats()), "index");
    }

    #[test]
    fn constraint_indexes_are_described_by_their_constraint() {
        let mut primary = index_stats();
        primary.is_primary = true;
        primary.is_unique = true;
        assert_eq!(render_kind(&primary), "primary");

        let mut unique = index_stats();
        unique.is_unique = true;
        assert_eq!(render_kind(&unique), "unique");
    }

    #[test]
    fn an_invalid_index_is_flagged() {
        // A failed CREATE INDEX CONCURRENTLY leaves an index that consumes
        // space and answers no queries.
        let mut invalid = index_stats();
        invalid.is_valid = false;
        assert_eq!(render_kind(&invalid), "index (invalid)");
    }
}

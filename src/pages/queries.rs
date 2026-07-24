/* pages/queries.rs
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

use crate::collector::statements::{Statement, StatementCounters, StatementsSample};
use crate::connection::probe::StatementsAvailability;
use crate::i18n::{i18n, i18n_f};
use crate::pages::format::{format_rate, format_ratio};
use crate::table::{Column, Table};

/// One table row: a sampled statement plus the mode the table is currently
/// displaying. The mode has to live on the row because column renderers are
/// plain function pointers with no access to page state.
#[derive(Clone)]
pub struct QueryRow {
    pub statement: Statement,
    pub delta_mode: bool,
}

/// Whether the table should actually render in interval mode: the user's
/// chosen mode, gated on at least one row carrying a delta. Until the first
/// delta exists there is nothing to show in interval mode, and cumulative
/// figures are better than a screen of dashes.
fn effective_delta_mode(requested: bool, statements: &[Statement]) -> bool {
    requested && statements.iter().any(|statement| statement.delta.is_some())
}

fn render_query(row: &QueryRow) -> String {
    row.statement
        .query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_calls(row: &QueryRow) -> String {
    match (row.delta_mode, row.statement.delta) {
        (true, Some(delta)) => i18n_f("{}/s", &[&format_rate(delta.calls_per_sec)]),
        (true, None) => "—".to_string(),
        (false, _) => format_rate(row.statement.cumulative.calls as f64),
    }
}

fn render_total_time(row: &QueryRow) -> String {
    match (row.delta_mode, row.statement.delta) {
        (true, Some(delta)) => i18n_f("{} ms/s", &[&format_rate(delta.exec_time_ms_per_sec)]),
        (true, None) => "—".to_string(),
        (false, _) => i18n_f(
            "{} ms",
            &[&format_rate(row.statement.cumulative.total_exec_time_ms)],
        ),
    }
}

/// `None` when the statement has never been called — no mean exists, and
/// reporting zero would claim one was measured.
fn cumulative_mean_ms(counters: &StatementCounters) -> Option<f64> {
    if counters.calls > 0 {
        Some(counters.total_exec_time_ms / counters.calls as f64)
    } else {
        None
    }
}

/// `None` when no shared blocks have been touched.
fn cumulative_cache_hit(counters: &StatementCounters) -> Option<f64> {
    let total = counters.shared_blks_hit + counters.shared_blks_read;
    if total > 0 {
        Some(counters.shared_blks_hit as f64 / total as f64)
    } else {
        None
    }
}

fn render_mean_time(row: &QueryRow) -> String {
    let mean = if row.delta_mode {
        row.statement
            .delta
            .and_then(|delta| delta.mean_exec_time_ms)
    } else {
        cumulative_mean_ms(&row.statement.cumulative)
    };
    match mean {
        Some(value) if value.is_finite() => format!("{value:.1} ms"),
        _ => "—".to_string(),
    }
}

fn render_rows(row: &QueryRow) -> String {
    match (row.delta_mode, row.statement.delta) {
        (true, Some(delta)) => i18n_f("{}/s", &[&format_rate(delta.rows_per_sec)]),
        (true, None) => "—".to_string(),
        (false, _) => format_rate(row.statement.cumulative.rows as f64),
    }
}

fn render_cache_hit(row: &QueryRow) -> String {
    if row.delta_mode {
        return match row.statement.delta {
            Some(delta) => format_ratio(delta.cache_hit_ratio),
            None => "—".to_string(),
        };
    }
    format_ratio(cumulative_cache_hit(&row.statement.cumulative))
}

/// Sort keys mirror the renderers: sorting by a cumulative figure while the
/// table displays an interval one would order the rows by numbers nobody
/// can see. A row with no delta sorts as zero in interval mode.
fn calls_key(row: &QueryRow) -> f64 {
    match (row.delta_mode, row.statement.delta) {
        (true, Some(delta)) => delta.calls_per_sec,
        (true, None) => 0.0,
        (false, _) => row.statement.cumulative.calls as f64,
    }
}

fn total_time_key(row: &QueryRow) -> f64 {
    match (row.delta_mode, row.statement.delta) {
        (true, Some(delta)) => delta.exec_time_ms_per_sec,
        (true, None) => 0.0,
        (false, _) => row.statement.cumulative.total_exec_time_ms,
    }
}

fn mean_time_key(row: &QueryRow) -> f64 {
    if row.delta_mode {
        return row
            .statement
            .delta
            .and_then(|delta| delta.mean_exec_time_ms)
            .unwrap_or(0.0);
    }
    cumulative_mean_ms(&row.statement.cumulative).unwrap_or(0.0)
}

fn rows_key(row: &QueryRow) -> f64 {
    match (row.delta_mode, row.statement.delta) {
        (true, Some(delta)) => delta.rows_per_sec,
        (true, None) => 0.0,
        (false, _) => row.statement.cumulative.rows as f64,
    }
}

fn cache_hit_key(row: &QueryRow) -> f64 {
    if row.delta_mode {
        return row
            .statement
            .delta
            .and_then(|delta| delta.cache_hit_ratio)
            .unwrap_or(0.0);
    }
    cumulative_cache_hit(&row.statement.cumulative).unwrap_or(0.0)
}

const COLUMNS: &[Column<QueryRow>] = &[
    Column {
        title: "Calls",
        render: render_calls,
        sort_key: Some(calls_key),
        expand: false,
    },
    Column {
        title: "Total time",
        render: render_total_time,
        sort_key: Some(total_time_key),
        expand: false,
    },
    Column {
        title: "Mean time",
        render: render_mean_time,
        sort_key: Some(mean_time_key),
        expand: false,
    },
    Column {
        title: "Rows",
        render: render_rows,
        sort_key: Some(rows_key),
        expand: false,
    },
    Column {
        title: "Cache hit",
        render: render_cache_hit,
        sort_key: Some(cache_hit_key),
        expand: false,
    },
    Column {
        title: "User",
        render: |row| row.statement.user_name.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Database",
        render: |row| row.statement.database.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Query",
        render: render_query,
        sort_key: None,
        expand: true,
    },
];

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/queries_page.ui")]
    pub struct McpgQueriesPage {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub status_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub filter_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub interval_toggle: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub privilege_note: TemplateChild<adw::Banner>,
        #[template_child]
        pub column_view: TemplateChild<gtk::ColumnView>,

        pub table: RefCell<Option<Table<QueryRow>>>,
        pub statements: RefCell<Vec<Statement>>,
        pub delta_mode: Cell<bool>,
        pub filter_text: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgQueriesPage {
        const NAME: &'static str = "McpgQueriesPage";
        type Type = super::McpgQueriesPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgQueriesPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.delta_mode.set(true);

            let page = self.obj().clone();
            let table = Table::attach(&self.column_view.get(), COLUMNS, move |row| {
                page.matches(row)
            });
            self.table.replace(Some(table));

            let page = self.obj().clone();
            self.filter_entry.connect_search_changed(move |entry| {
                page.imp()
                    .filter_text
                    .replace(entry.text().to_lowercase().to_string());
                if let Some(table) = page.imp().table.borrow().as_ref() {
                    table.refilter();
                }
            });

            let page = self.obj().clone();
            self.interval_toggle.connect_toggled(move |button| {
                page.imp().delta_mode.set(button.is_active());
                button.set_label(&if button.is_active() {
                    i18n("Last interval")
                } else {
                    i18n("Since reset")
                });
                // Every displayed number changes meaning, so rebuild the rows.
                page.rebuild_rows();
            });
        }
    }

    impl WidgetImpl for McpgQueriesPage {}
    impl BoxImpl for McpgQueriesPage {}
}

glib::wrapper! {
    pub struct McpgQueriesPage(ObjectSubclass<imp::McpgQueriesPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgQueriesPage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn matches(&self, row: &QueryRow) -> bool {
        let needle = self.imp().filter_text.borrow();
        if needle.is_empty() {
            return true;
        }
        let haystack = [
            Some(row.statement.query.as_str()),
            row.statement.user_name.as_deref(),
            row.statement.database.as_deref(),
        ];
        haystack
            .iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(needle.as_str()))
    }

    fn rebuild_rows(&self) {
        let imp = self.imp();
        let statements = imp.statements.borrow();
        // Self-heals on the next slow sample without touching the toggle,
        // which stays "Last interval" throughout: that is still what the
        // user selected, and what they get as soon as a delta exists.
        let delta_mode = effective_delta_mode(imp.delta_mode.get(), &statements);
        let rows: Vec<QueryRow> = statements
            .iter()
            .cloned()
            .map(|statement| QueryRow {
                statement,
                delta_mode,
            })
            .collect();
        if let Some(table) = imp.table.borrow().as_ref() {
            table.update(&rows);
        }
    }

    /// Shows the table, or the status page explaining why there is none.
    /// An absent extension is a fact about the server, not a fault in the
    /// connection, so this never raises the window's error banner.
    pub fn set_statements_availability(
        &self,
        availability: &StatementsAvailability,
        database: &str,
    ) {
        let imp = self.imp();
        match availability {
            StatementsAvailability::Available { .. } => {
                imp.stack.set_visible_child_name("table");
            }
            StatementsAvailability::NotInstalled => {
                imp.status_page
                    .set_title(&i18n("pg_stat_statements is not installed"));
                imp.status_page.set_description(Some(&i18n_f(
                    "The extension is not present in the database {}. Add pg_stat_statements to shared_preload_libraries, restart the server, then run CREATE EXTENSION pg_stat_statements.",
                    &[&database],
                )));
                imp.stack.set_visible_child_name("unavailable");
            }
            StatementsAvailability::TooOld { version } => {
                imp.status_page
                    .set_title(&i18n("pg_stat_statements is too old"));
                imp.status_page.set_description(Some(&i18n_f(
                    "Version {} is installed; version 1.8 or later is required. Run ALTER EXTENSION pg_stat_statements UPDATE.",
                    &[version],
                )));
                imp.stack.set_visible_child_name("unavailable");
            }
        }
    }

    pub fn set_privilege_limited(&self, limited: bool) {
        self.imp().privilege_note.set_revealed(limited);
    }

    /// Drops the statements from the previously selected server. Called on a
    /// server switch, not on a reconnect: until the new server's first slow
    /// sample arrives, the old server's rows would otherwise be presented
    /// under the new connection with nothing to mark them stale. Also resets
    /// the stack and status page: otherwise an unavailability or error
    /// message about the previous server (e.g. "the extension is not present
    /// in the database A_db") would go on being shown about the new one,
    /// including one that never finishes connecting.
    pub fn clear(&self) {
        let imp = self.imp();
        imp.statements.replace(Vec::new());
        imp.status_page.set_title("");
        imp.status_page.set_description(None);
        imp.stack.set_visible_child_name("table");
        self.rebuild_rows();
    }

    pub fn update(&self, sample: &StatementsSample) {
        self.imp().statements.replace(sample.statements.clone());
        self.imp().stack.set_visible_child_name("table");
        self.rebuild_rows();
    }

    /// The extension is present but its query failed — insufficient
    /// privilege, or the extension dropped mid-session.
    ///
    /// This discards the table for the status page rather than banner-over-
    /// stale-rows, unlike `McpgRelationsPage::set_error`: this page already
    /// owns a status-page slot for the availability gate, so a failure has
    /// somewhere to go that the relations page does not have, and reusing it
    /// keeps one explanation mechanism per page rather than two.
    pub fn set_error(&self, message: &str) {
        let imp = self.imp();
        imp.status_page
            .set_title(&i18n("Statement statistics are unavailable"));
        imp.status_page.set_description(Some(message));
        imp.stack.set_visible_child_name("unavailable");
    }
}

impl Default for McpgQueriesPage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::statements::{statement_key, StatementCounters, StatementDelta};

    fn row(delta_mode: bool, delta: Option<StatementDelta>) -> QueryRow {
        QueryRow {
            statement: Statement {
                key: statement_key(10, 20, Some(1), "SELECT 1"),
                query: "SELECT   1\n  FROM t".to_string(),
                user_name: Some("app".to_string()),
                database: Some("orders".to_string()),
                cumulative: StatementCounters {
                    calls: 400,
                    total_exec_time_ms: 2_000.0,
                    rows: 800,
                    shared_blks_hit: 900,
                    shared_blks_read: 100,
                    ..StatementCounters::default()
                },
                delta,
            },
            delta_mode,
        }
    }

    fn delta() -> StatementDelta {
        StatementDelta {
            calls_per_sec: 25.0,
            exec_time_ms_per_sec: 125.0,
            // Deliberately distinct from the cumulative mean (2,000/400 =
            // 5.0 ms) so a test that reads the wrong branch fails instead
            // of coincidentally passing.
            mean_exec_time_ms: Some(7.5),
            rows_per_sec: 50.0,
            cache_hit_ratio: Some(0.95),
        }
    }

    #[test]
    fn the_query_text_is_collapsed_onto_one_line() {
        assert_eq!(render_query(&row(false, None)), "SELECT 1 FROM t");
    }

    #[test]
    fn cumulative_mode_shows_totals_since_the_reset() {
        assert_eq!(render_calls(&row(false, Some(delta()))), "400");
        // Computed from the counters (2,000 ms / 400 calls), not read from
        // the delta's stored 7.5.
        assert_eq!(render_mean_time(&row(false, Some(delta()))), "5.0 ms");
        assert_eq!(render_total_time(&row(false, Some(delta()))), "2,000 ms");
        assert_eq!(render_rows(&row(false, Some(delta()))), "800");
        assert_eq!(render_cache_hit(&row(false, Some(delta()))), "90.0%");
    }

    #[test]
    fn interval_mode_shows_per_second_figures() {
        assert_eq!(render_calls(&row(true, Some(delta()))), "25/s");
        assert_eq!(render_rows(&row(true, Some(delta()))), "50/s");
        // Read straight from the delta's stored 7.5, not computed.
        assert_eq!(render_mean_time(&row(true, Some(delta()))), "7.5 ms");
    }

    #[test]
    fn interval_mode_shows_a_dash_when_there_is_no_delta_yet() {
        // The first slow sample after connecting has nothing to compare
        // against. A zero here would claim the statement stopped running.
        assert_eq!(render_calls(&row(true, None)), "—");
        assert_eq!(render_total_time(&row(true, None)), "—");
        assert_eq!(render_cache_hit(&row(true, None)), "—");
    }

    #[test]
    fn the_interval_mean_is_absent_when_the_statement_did_not_run() {
        let mut without_calls = delta();
        without_calls.mean_exec_time_ms = None;
        assert_eq!(render_mean_time(&row(true, Some(without_calls))), "—");
    }

    #[test]
    fn sort_keys_follow_the_mode() {
        assert_eq!(calls_key(&row(false, Some(delta()))), 400.0);
        assert_eq!(calls_key(&row(true, Some(delta()))), 25.0);
        // With no delta, an interval-mode row sorts to the bottom rather
        // than borrowing its cumulative figure.
        assert_eq!(calls_key(&row(true, None)), 0.0);
    }

    #[test]
    fn interval_mode_falls_back_to_cumulative_until_a_delta_exists() {
        // The first slow sample after connecting: every row's delta is
        // `None`. Rendering the requested interval mode here would produce a
        // full screen of dashes with no explanation, for up to a whole slow
        // interval.
        let no_deltas_yet = [row(true, None).statement];
        assert!(!effective_delta_mode(true, &no_deltas_yet));

        // Once any row carries a delta, interval mode renders as requested.
        let one_delta = [
            row(true, None).statement,
            row(true, Some(delta())).statement,
        ];
        assert!(effective_delta_mode(true, &one_delta));

        // Cumulative mode is unaffected either way.
        assert!(!effective_delta_mode(false, &no_deltas_yet));
    }
}

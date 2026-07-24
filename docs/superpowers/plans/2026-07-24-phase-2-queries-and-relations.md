# Mission Centre PostgreSQL — Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Queries page fed by `pg_stat_statements` and a Tables & Indexes page fed by `pg_stat_user_tables`/`pg_stat_user_indexes`, sampled on a second, slower cadence, on top of the shared `ColumnView` machinery the Sessions page moves onto first.

**Architecture:** The existing serial sample loop gains a slow tier that runs once every `slow-sample-interval-ms` on the same connection under the same `statement_timeout`. `Snapshot` gains two `Option<Result<…, CollectorError>>` fields — `None` means "not sampled this tick", `Err` means "sampled and failed" — so a permission error on one view degrades one page instead of disconnecting the server. A new `src/table/` module holds the store/filter/sorter/factory machinery for all four tables.

**Tech Stack:** Rust, gtk4-rs 0.11, libadwaita 0.9, Blueprint, Meson + Cargo, tokio-postgres 0.7, testcontainers 0.27.

**Spec:** `docs/superpowers/specs/2026-07-24-phase-2-queries-and-relations-design.md`
**Parent spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Repository:** `/home/paul/gitHUB/mission-centre-postgresql`.
- **Licence:** GPL-3.0-or-later. Every new source file carries the same GPL header as its neighbours, naming **Paul Snow** as author, ending `SPDX-License-Identifier: GPL-3.0-or-later`.
- **Version:** `0.0.0`.
- **PostgreSQL floor:** 14. Never refuse a connection on version grounds; gate at the page.
- **`pg_stat_statements` extension floor:** 1.8.
- **Spelling:** British English in all user-facing strings, comments and documentation (`behaviour`, `initialise`, `colour`).
- **Never log or display a password**, nor a full connection string containing one.
- **Never touch GTK widgets off the main thread.** Collector output reaches the UI only through the `async_channel`.
- **`cargo fmt` must produce no diff** before any commit.
- **File size:** no source file over ~800 lines.
- **Rates are per-interval deltas**, never cumulative-since-reset.
- **Cargo renames the GTK crates:** `Cargo.toml` maps `gtk` → `gtk4` and `adw` → `libadwaita`, so code says `gtk::` and `adw::`.
- **`glib::wrapper!` blocks for `CompositeTemplate` widgets must list** `gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget` (plus `gtk::Orientable` for `gtk::Box` subclasses).
- **Every new `.blp` file must be added in two places:** `resources/meson.build` (the `blueprints` input list) and `resources/mission-centre-pg.gresource.xml`.
- **Container tests need podman:** `export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"` before `cargo test --test portability`.

### Commands

| Purpose | Command |
|---------|---------|
| Unit tests | `cargo test --lib` |
| One unit test | `cargo test --lib <name> -- --exact --nocapture` |
| Container tests | `export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"; cargo test --test portability` |
| Format check | `cargo fmt --check` |
| Full build | `ninja -C build` |
| Run | `./build/src/mission-centre-pg` |

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/table/mod.rs` | Create. `Column<T>`, `Table<T>`, `McpgRowObject`, `compare_rows` — all four tables' machinery | 1 |
| `src/pages/sessions.rs` | Modify. Drops its inline table machinery, uses `Table<Session>` | 1 |
| `src/connection/probe.rs` | Modify. `StatementsAvailability`, extension version parsing, extended `PROBE_SQL` | 2 |
| `src/collector/statements.rs` | Create. `STATEMENTS_SQL`, `StatementKey`, counters, delta derivation, row mapping | 3 |
| `src/collector/relations.rs` | Create. `TABLES_SQL`, `INDEXES_SQL`, `TableStats`, `IndexStats`, ratio helpers | 4 |
| `src/collector/snapshot.rs` | Modify. The two new `Option<Result<…>>` fields | 5 |
| `src/collector/worker.rs` | Modify. `CollectorConfig`, slow-tier scheduling, error classification | 5 |
| `data/io.github.paulsnow.MissionCentrePg.gschema.xml` | Modify. Three new keys | 5 |
| `src/pages/queries.rs` + `resources/ui/queries_page.blp` | Create. Queries page | 6 |
| `src/pages/relations.rs` + `resources/ui/relations_page.blp` | Create. Tables & Indexes page | 7 |
| `src/window.rs` + `resources/ui/window.blp` | Modify. Two new stack pages, event routing | 6, 7 |
| `tests/portability.rs` | Modify. Container coverage for the new SQL on 14 and 18 | 2, 3, 4 |

---

## Task 1: Shared `table/` module, with Sessions migrated onto it

**Files:**
- Create: `src/table/mod.rs`
- Modify: `src/lib.rs`
- Modify: `src/pages/sessions.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `mission_centre_pg::table::Column<T> { title: &'static str, render: fn(&T) -> String, sort_key: Option<fn(&T) -> f64>, expand: bool }`
  - `mission_centre_pg::table::Table<T>` with `Table::<T>::attach(view: &gtk::ColumnView, columns: &[Column<T>], matches: impl Fn(&T) -> bool + 'static) -> Table<T>`, `update(&self, rows: &[T])`, `refilter(&self)`
  - `mission_centre_pg::table::compare_rows<T>(a: &T, b: &T, render: fn(&T) -> String, sort_key: Option<fn(&T) -> f64>) -> std::cmp::Ordering`
  - `mission_centre_pg::table::McpgRowObject` with `McpgRowObject::new<T: 'static>(row: T)` and `row<T: 'static>(&self) -> std::rc::Rc<T>`

**Why one row object rather than four:** GObject subclasses cannot be generic. The alternatives are a macro generating a row type per table, or one type erasing the payload. This takes the second: `McpgRowObject` holds `Rc<dyn Any>`, and `Table<T>` does the downcast so callers never see `Any`. Consequence: the `set_incremental(false)` workaround for the upstream GTK sort/filter crash exists in exactly one place.

- [ ] **Step 1: Write the failing test**

Create `src/table/mod.rs` with the standard GPL header (copy the header block from `src/pages/format.rs`, changing the first line to `/* table/mod.rs`), then this body:

```rust
use std::cmp::Ordering;

/// How a row renders as text in a column cell.
pub type Renderer<T> = fn(&T) -> String;
/// Extracts a numeric sort key from a row, for columns that must sort
/// numerically rather than lexically (so "10" sorts after "9").
pub type NumericKey<T> = fn(&T) -> f64;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Row {
        name: &'static str,
        count: i64,
    }

    fn name(row: &Row) -> String {
        row.name.to_string()
    }

    fn count(row: &Row) -> String {
        row.count.to_string()
    }

    fn count_key(row: &Row) -> f64 {
        row.count as f64
    }

    #[test]
    fn a_numeric_column_sorts_by_its_key_not_its_text() {
        let nine = Row { name: "a", count: 9 };
        let ten = Row { name: "b", count: 10 };

        assert_eq!(
            compare_rows(&nine, &ten, count as Renderer<Row>, Some(count_key as NumericKey<Row>)),
            Ordering::Less
        );
        // Guard against a regression to lexical sorting: as text, "10" < "9",
        // which is the wrong order the numeric key exists to prevent.
        assert_eq!(count(&ten).cmp(&count(&nine)), Ordering::Less);
    }

    #[test]
    fn a_column_without_a_key_sorts_lexically() {
        let alice = Row { name: "alice", count: 2 };
        let bob = Row { name: "bob", count: 1 };
        assert_eq!(
            compare_rows(&alice, &bob, name as Renderer<Row>, None),
            Ordering::Less
        );
    }

    #[test]
    fn a_non_comparable_key_leaves_the_order_unchanged() {
        // A NaN sort key must not panic. partial_cmp returns None and the
        // rows compare equal, leaving the existing order alone.
        fn nan_key(_: &Row) -> f64 {
            f64::NAN
        }
        let a = Row { name: "a", count: 1 };
        let b = Row { name: "b", count: 2 };
        assert_eq!(
            compare_rows(&a, &b, name as Renderer<Row>, Some(nan_key as NumericKey<Row>)),
            Ordering::Equal
        );
    }
}
```

Register the module by adding `pub mod table;` to `src/lib.rs`, in alphabetical position between `pub mod pages;` and `pub mod widgets;`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib table::`
Expected: FAIL — `cannot find function 'compare_rows' in this scope`.

- [ ] **Step 3: Implement `compare_rows`**

Insert immediately after the `NumericKey<T>` type alias in `src/table/mod.rs`:

```rust
/// Orders two rows for a column. Numeric columns compare by their numeric
/// key; the rest compare lexically on the rendered text. A pure function so
/// the numeric-versus-lexical behaviour is testable without a GTK widget in
/// the loop.
pub fn compare_rows<T>(
    a: &T,
    b: &T,
    render: Renderer<T>,
    sort_key: Option<NumericKey<T>>,
) -> Ordering {
    match sort_key {
        Some(key) => key(a).partial_cmp(&key(b)).unwrap_or(Ordering::Equal),
        None => render(a).cmp(&render(b)),
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib table::`
Expected: PASS, 3 tests.

- [ ] **Step 5: Add the row object and `Table<T>`**

Add these imports to the top of `src/table/mod.rs`, above the type aliases:

```rust
use std::any::Any;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::i18n::i18n;
```

Then append, after `compare_rows`:

```rust
/// A column: its heading, how a row renders in it, and — for columns whose
/// values are numbers — how to extract a numeric key so header clicks sort
/// numerically.
pub struct Column<T> {
    pub title: &'static str,
    pub render: Renderer<T>,
    pub sort_key: Option<NumericKey<T>>,
    pub expand: bool,
}

glib::wrapper! {
    pub struct McpgRowObject(ObjectSubclass<row_object::McpgRowObject>);
}

mod row_object {
    use super::*;

    #[derive(Default)]
    pub struct McpgRowObject {
        pub payload: RefCell<Option<Rc<dyn Any>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgRowObject {
        const NAME: &'static str = "McpgRowObject";
        type Type = super::McpgRowObject;
    }

    impl ObjectImpl for McpgRowObject {}
}

impl McpgRowObject {
    pub fn new<T: 'static>(row: T) -> Self {
        let object: Self = glib::Object::new();
        object.imp().payload.replace(Some(Rc::new(row)));
        object
    }

    /// The payload, downcast to the row type of the `Table` that made it.
    /// Only `Table<T>` constructs and reads these, so the type always matches.
    pub fn row<T: 'static>(&self) -> Rc<T> {
        self.imp()
            .payload
            .borrow()
            .clone()
            .expect("a row object always holds a payload")
            .downcast::<T>()
            .expect("the payload type matches the Table that created it")
    }
}

/// The store, filter and sorter behind one `ColumnView`. The type parameter
/// keeps the API typed even though the underlying row object erases it.
pub struct Table<T> {
    store: gio::ListStore,
    filter: gtk::CustomFilter,
    marker: PhantomData<T>,
}

impl<T: Clone + 'static> Table<T> {
    /// Builds the model, installs it on `view`, and appends one column per
    /// entry in `columns`. `matches` decides which rows the filter admits;
    /// it is re-evaluated on every `refilter()`.
    pub fn attach(
        view: &gtk::ColumnView,
        columns: &[Column<T>],
        matches: impl Fn(&T) -> bool + 'static,
    ) -> Self {
        let store = gio::ListStore::new::<McpgRowObject>();

        let filter = gtk::CustomFilter::new(move |object| {
            let row = object
                .downcast_ref::<McpgRowObject>()
                .expect("the model only holds McpgRowObject")
                .row::<T>();
            matches(&row)
        });

        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        // Incremental filtering plus rapid items-changed is the combination
        // implicated in the upstream GTK sort/filter crash; keep it off.
        filtered.set_incremental(false);

        let sorted = gtk::SortListModel::new(Some(filtered), view.sorter());
        sorted.set_incremental(false);

        view.set_model(Some(&gtk::NoSelection::new(Some(sorted))));

        for column in columns {
            append_column(view, column);
        }

        Table {
            store,
            filter,
            marker: PhantomData,
        }
    }

    /// Replaces the contents in one splice, keeping items-changed to a single
    /// emission per sample rather than one per row.
    pub fn update(&self, rows: &[T]) {
        let objects: Vec<McpgRowObject> = rows.iter().cloned().map(McpgRowObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objects);
    }

    pub fn refilter(&self) {
        self.filter.changed(gtk::FilterChange::Different);
    }
}

fn append_column<T: 'static>(view: &gtk::ColumnView, column: &Column<T>) {
    let render = column.render;
    let sort_key = column.sort_key;

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
        let row = item
            .item()
            .and_downcast::<McpgRowObject>()
            .expect("a McpgRowObject")
            .row::<T>();
        let text = render(&row);
        label.set_tooltip_text(Some(&text));
        label.set_text(&text);
    });

    // The SortListModel built in `attach` watches `view.sorter()`, which
    // tracks whichever column's sorter is currently active.
    let sorter = gtk::CustomSorter::new(move |a, b| {
        let a = a
            .downcast_ref::<McpgRowObject>()
            .expect("the model only holds McpgRowObject")
            .row::<T>();
        let b = b
            .downcast_ref::<McpgRowObject>()
            .expect("the model only holds McpgRowObject")
            .row::<T>();
        compare_rows(&a, &b, render, sort_key).into()
    });

    let view_column = gtk::ColumnViewColumn::new(Some(&i18n(column.title)), Some(&factory));
    view_column.set_resizable(true);
    view_column.set_expand(column.expand);
    view_column.set_sorter(Some(&sorter));
    view.append_column(&view_column);
}
```

Note `i18n(column.title)`: Phase 1 passed column headings through untranslated. Since every heading now flows through this one call site, translating them here costs nothing and matches every other user-facing string in the project.

- [ ] **Step 6: Verify it compiles and the tests still pass**

Run: `cargo test --lib table::`
Expected: PASS, 3 tests, no warnings about unused imports.

- [ ] **Step 7: Commit the module**

```bash
git add src/table/mod.rs src/lib.rs
git commit -m "feat: shared ColumnView table machinery"
```

- [ ] **Step 8: Migrate Sessions onto `Table<Session>`**

Replace the whole of `src/pages/sessions.rs` below the licence header with this. The `COLUMNS` content is unchanged in behaviour; what goes away is the `SessionObject` type, the `Renderer`/`NumericKey` aliases, `compare_sessions`, `build_model` and `build_columns`.

```rust
use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::collector::snapshot::Session;
use crate::table::{Column, Table};

const COLUMNS: &[Column<Session>] = &[
    Column {
        title: "PID",
        render: |s| s.pid.to_string(),
        sort_key: Some(|s| s.pid as f64),
        expand: false,
    },
    Column {
        title: "User",
        render: |s| s.user_name.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Database",
        render: |s| s.database.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Application",
        render: |s| s.application_name.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Client",
        render: |s| s.client_addr.clone().unwrap_or_else(|| "local".to_string()),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "State",
        render: |s| s.state.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Wait",
        render: |s| match (&s.wait_event_type, &s.wait_event) {
            (Some(kind), Some(event)) => format!("{kind}: {event}"),
            _ => String::new(),
        },
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Duration",
        render: |s| match s.query_duration_secs {
            Some(secs) if secs >= 1.0 => format!("{secs:.0}s"),
            Some(secs) => format!("{:.0}ms", secs * 1000.0),
            None => String::new(),
        },
        // A session with no running query has no duration; sort it as zero so
        // idle backends group at the short end rather than sorting by text.
        sort_key: Some(|s| s.query_duration_secs.unwrap_or(0.0)),
        expand: false,
    },
    Column {
        title: "Query",
        render: |s| {
            s.query
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

        pub table: RefCell<Option<Table<Session>>>,
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

            let page = self.obj().clone();
            let table = Table::attach(&self.column_view.get(), COLUMNS, move |session| {
                page.matches(session)
            });
            self.table.replace(Some(table));

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
        if let Some(table) = self.imp().table.borrow().as_ref() {
            table.refilter();
        }
    }

    pub fn set_hide_idle(&self, hide: bool) {
        self.imp().hide_idle_toggle.set_active(hide);
    }

    pub fn set_privilege_limited(&self, limited: bool) {
        self.imp().privilege_note.set_revealed(limited);
    }

    pub fn update(&self, sessions: &[Session]) {
        if let Some(table) = self.imp().table.borrow().as_ref() {
            table.update(sessions);
        }
    }
}

impl Default for McpgSessionsPage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::compare_rows;
    use std::cmp::Ordering;

    fn session_with_pid(pid: i32) -> Session {
        Session {
            pid,
            user_name: None,
            application_name: None,
            client_addr: None,
            database: None,
            state: None,
            wait_event_type: None,
            wait_event: None,
            backend_type: None,
            query_duration_secs: None,
            query: None,
        }
    }

    #[test]
    fn pid_sorts_numerically_not_lexically() {
        let column = &COLUMNS[0];
        assert_eq!(column.title, "PID");

        let nine = session_with_pid(9);
        let ten = session_with_pid(10);

        assert_eq!(
            compare_rows(&nine, &ten, column.render, column.sort_key),
            Ordering::Less
        );
        // Guard against a regression to lexical sorting: as text, "10" < "9".
        assert_eq!(
            (column.render)(&ten).cmp(&(column.render)(&nine)),
            Ordering::Less
        );
    }

    #[test]
    fn duration_sorts_numerically_by_seconds() {
        let column = COLUMNS
            .iter()
            .find(|c| c.title == "Duration")
            .expect("the Duration column exists");

        let mut short = session_with_pid(1);
        let mut long = session_with_pid(2);
        short.query_duration_secs = Some(2.0);
        long.query_duration_secs = Some(10.0);

        assert_eq!(
            compare_rows(&short, &long, column.render, column.sort_key),
            Ordering::Less
        );
    }

    #[test]
    fn text_columns_sort_lexically() {
        let column = COLUMNS
            .iter()
            .find(|c| c.title == "User")
            .expect("the User column exists");

        let mut alice = session_with_pid(1);
        let mut bob = session_with_pid(2);
        alice.user_name = Some("alice".to_string());
        bob.user_name = Some("bob".to_string());

        assert_eq!(
            compare_rows(&alice, &bob, column.render, column.sort_key),
            Ordering::Less
        );
    }
}
```

- [ ] **Step 9: Run the whole unit suite**

Run: `cargo test --lib`
Expected: PASS. The three Sessions comparator tests now exercise `table::compare_rows`; total count is 41 + 3 new table tests = 44.

- [ ] **Step 10: Build and check Sessions still behaves**

```bash
cargo fmt
ninja -C build
```

Expected: builds clean. Launch `./build/src/mission-centre-pg`, connect to the local server, open Sessions, and confirm: rows appear, clicking the PID header sorts numerically, typing in the filter narrows the list, and Hide idle is on by default.

- [ ] **Step 11: Commit**

```bash
git add src/pages/sessions.rs
git commit -m "refactor: move the Sessions table onto the shared table module"
```

---

## Task 2: `pg_stat_statements` availability probe

**Files:**
- Modify: `src/connection/probe.rs`
- Modify: `tests/portability.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `StatementsAvailability` enum with variants `Available { version: String }`, `TooOld { version: String }`, `NotInstalled`
  - `StatementsAvailability::classify(extversion: Option<&str>) -> StatementsAvailability`
  - `StatementsAvailability::is_available(&self) -> bool`
  - `ServerInfo.statements: StatementsAvailability`
  - `PROBE_SQL` extended with a `statements_version` column

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block at the bottom of `src/connection/probe.rs`:

```rust
    #[test]
    fn an_absent_extension_is_not_installed() {
        assert_eq!(
            StatementsAvailability::classify(None),
            StatementsAvailability::NotInstalled
        );
    }

    #[test]
    fn version_1_8_and_later_are_available() {
        for version in ["1.8", "1.9", "1.11"] {
            assert_eq!(
                StatementsAvailability::classify(Some(version)),
                StatementsAvailability::Available {
                    version: version.to_string()
                },
                "{version} should be usable"
            );
        }
    }

    #[test]
    fn version_1_10_is_available_despite_sorting_before_1_8_as_text() {
        // The case that catches lexical comparison: "1.10" < "1.8" as text,
        // so a string compare would reject a newer extension than the floor.
        assert_eq!(
            StatementsAvailability::classify(Some("1.10")),
            StatementsAvailability::Available {
                version: "1.10".to_string()
            }
        );
    }

    #[test]
    fn version_1_7_is_too_old() {
        // 1.7 predates total_exec_time, so the query fails on a missing
        // column rather than a missing view.
        assert_eq!(
            StatementsAvailability::classify(Some("1.7")),
            StatementsAvailability::TooOld {
                version: "1.7".to_string()
            }
        );
    }

    #[test]
    fn an_unparseable_version_is_treated_as_too_old() {
        // Better to show the upgrade remedy than to run a query every ten
        // seconds that is going to fail on a column we cannot prove exists.
        assert_eq!(
            StatementsAvailability::classify(Some("banana")),
            StatementsAvailability::TooOld {
                version: "banana".to_string()
            }
        );
    }

    #[test]
    fn only_the_available_variant_reports_itself_usable() {
        assert!(StatementsAvailability::classify(Some("1.9")).is_available());
        assert!(!StatementsAvailability::classify(Some("1.7")).is_available());
        assert!(!StatementsAvailability::classify(None).is_available());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib connection::probe`
Expected: FAIL — `cannot find type 'StatementsAvailability' in this scope`.

- [ ] **Step 3: Implement the type and extend the probe**

In `src/connection/probe.rs`, replace the `PROBE_SQL` constant with:

```rust
pub const PROBE_SQL: &str = "\
SELECT current_setting('server_version_num')::int AS version_num,
       pg_has_role(current_user, 'pg_monitor', 'member') AS is_monitor,
       COALESCE((SELECT rolsuper FROM pg_roles WHERE rolname = current_user), false) AS is_superuser,
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_stat_statements')
         AS statements_version";
```

Add, after the `PrivilegeLevel` block:

```rust
/// The `pg_stat_statements` columns this project reads — `total_exec_time`
/// and `mean_exec_time` — arrived in extension version 1.8. A server at or
/// above the PostgreSQL 14 floor can still carry 1.7 through a `pg_upgrade`
/// that never ran `ALTER EXTENSION … UPDATE`.
pub const MINIMUM_STATEMENTS_VERSION: (u32, u32) = (1, 8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementsAvailability {
    Available { version: String },
    TooOld { version: String },
    NotInstalled,
}

impl StatementsAvailability {
    pub fn classify(extversion: Option<&str>) -> Self {
        let Some(version) = extversion else {
            return StatementsAvailability::NotInstalled;
        };
        match parse_extension_version(version) {
            Some(parsed) if parsed >= MINIMUM_STATEMENTS_VERSION => {
                StatementsAvailability::Available {
                    version: version.to_string(),
                }
            }
            _ => StatementsAvailability::TooOld {
                version: version.to_string(),
            },
        }
    }

    pub fn is_available(&self) -> bool {
        matches!(self, StatementsAvailability::Available { .. })
    }
}

/// Extension versions are `major.minor`. Comparison must be numeric per
/// component: as text "1.10" sorts before "1.8", which would reject an
/// extension newer than the floor.
fn parse_extension_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor))
}
```

Extend `ServerInfo` and its mapper:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub version_num: i32,
    pub version_display: String,
    pub privilege: PrivilegeLevel,
    pub statements: StatementsAvailability,
}
```

```rust
pub fn map_server_info(row: &Row) -> ServerInfo {
    let version_num: i32 = row.get("version_num");
    let is_monitor: bool = row.get("is_monitor");
    let is_superuser: bool = row.get("is_superuser");
    let statements_version: Option<String> = row.get("statements_version");
    ServerInfo {
        version_num,
        version_display: format_version(version_num),
        privilege: PrivilegeLevel::classify(is_superuser, is_monitor),
        statements: StatementsAvailability::classify(statements_version.as_deref()),
    }
}
```

- [ ] **Step 4: Fix the three existing `ServerInfo` literals**

`recognises_a_server_below_the_supported_floor` in the same test module builds three `ServerInfo` values and will no longer compile. Add `statements: StatementsAvailability::NotInstalled,` as the last field of each of the three literals.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib connection::probe`
Expected: PASS, 10 tests.

- [ ] **Step 6: Add the container assertion that a plain server reports NotInstalled**

In `tests/portability.rs`, extend the import on line 30 to:

```rust
use mission_centre_pg::connection::probe::{
    map_server_info, PrivilegeLevel, StatementsAvailability, PROBE_SQL,
};
```

Then append this test at the end of the file:

```rust
#[tokio::test]
async fn a_server_without_the_extension_probes_as_not_installed() {
    // The gate must not depend on issuing the query and interpreting the
    // failure: a stock container has no pg_stat_statements at all.
    let (client, _container) = connect("18").await;

    let probe = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("probe failed");

    assert_eq!(
        map_server_info(&probe).statements,
        StatementsAvailability::NotInstalled
    );
}
```

- [ ] **Step 7: Run the container tests**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
```

Expected: PASS, 4 tests (3 existing + 1 new).

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add src/connection/probe.rs tests/portability.rs
git commit -m "feat: probe pg_stat_statements availability at connect"
```

---

## Task 3: `collector/statements.rs` — statement identity, counters and deltas

**Files:**
- Create: `src/collector/statements.rs`
- Modify: `src/collector/mod.rs`
- Modify: `tests/portability.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1–2.
- Produces:
  - `STATEMENTS_SQL: &str` — takes one `$1` parameter, an `i64` row limit
  - `StatementId { QueryId(i64), TextHash(u64) }`, `StatementKey { user_oid: i64, db_oid: i64, id: StatementId }`
  - `StatementCounters` (all fields `pub`, `Copy`, `Default`) with `went_backwards_from(&self, previous: &Self) -> bool`
  - `StatementDelta` (`Copy`) with `calls_per_sec`, `exec_time_ms_per_sec`, `mean_exec_time_ms: Option<f64>`, `rows_per_sec`, `cache_hit_ratio: Option<f64>`
  - `Statement { key, query, user_name, database, cumulative, delta }`
  - `StatementsSample { statements: Vec<Statement> }`
  - `statement_key(user_oid: i64, db_oid: i64, query_id: Option<i64>, query: &str) -> StatementKey`
  - `map_statement(row: &tokio_postgres::Row) -> Statement`
  - `derive_delta(prev: &StatementCounters, cur: &StatementCounters, elapsed: Duration) -> Option<StatementDelta>`
  - `apply_deltas(statements: &mut [Statement], previous: &HashMap<StatementKey, StatementCounters>, elapsed: Duration)`
  - `counters_by_key(statements: &[Statement]) -> HashMap<StatementKey, StatementCounters>`

- [ ] **Step 1: Write the failing tests**

Create `src/collector/statements.rs` with the GPL header, then:

```rust
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use tokio_postgres::Row;

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(calls: i64, time_ms: f64, rows: i64, hit: i64, read: i64) -> StatementCounters {
        StatementCounters {
            calls,
            total_exec_time_ms: time_ms,
            rows,
            shared_blks_hit: hit,
            shared_blks_read: read,
            ..StatementCounters::default()
        }
    }

    #[test]
    fn derives_per_second_rates_from_the_delta() {
        let prev = counters(100, 1_000.0, 500, 900, 100);
        let cur = counters(300, 3_000.0, 1_500, 2_700, 300);
        let delta = derive_delta(&prev, &cur, Duration::from_secs(2)).unwrap();

        assert_eq!(delta.calls_per_sec, 100.0);
        assert_eq!(delta.exec_time_ms_per_sec, 1_000.0);
        assert_eq!(delta.rows_per_sec, 500.0);
    }

    #[test]
    fn the_interval_mean_is_the_intervals_time_over_its_calls() {
        // Lifetime mean would be 3000/300 = 10ms. The interval mean is
        // 2000/200 = 10ms here only by coincidence, so use figures that differ.
        let prev = counters(100, 1_000.0, 0, 0, 0);
        let cur = counters(200, 5_000.0, 0, 0, 0);
        let delta = derive_delta(&prev, &cur, Duration::from_secs(1)).unwrap();

        assert_eq!(delta.mean_exec_time_ms, Some(40.0));
    }

    #[test]
    fn a_statement_not_called_during_the_interval_has_no_interval_mean() {
        let prev = counters(100, 1_000.0, 0, 0, 0);
        let cur = counters(100, 1_000.0, 0, 0, 0);
        let delta = derive_delta(&prev, &cur, Duration::from_secs(1)).unwrap();

        assert_eq!(delta.mean_exec_time_ms, None);
        assert_eq!(delta.calls_per_sec, 0.0);
    }

    #[test]
    fn a_cache_ratio_needs_blocks_to_have_been_touched() {
        let prev = counters(1, 1.0, 0, 100, 10);
        let cur = counters(2, 2.0, 0, 100, 10);
        assert_eq!(
            derive_delta(&prev, &cur, Duration::from_secs(1))
                .unwrap()
                .cache_hit_ratio,
            None
        );

        let cur = counters(2, 2.0, 0, 190, 20);
        assert_eq!(
            derive_delta(&prev, &cur, Duration::from_secs(1))
                .unwrap()
                .cache_hit_ratio,
            Some(0.9)
        );
    }

    #[test]
    fn counters_going_backwards_yield_no_delta() {
        // pg_stat_statements_reset(), or the entry was evicted at
        // pg_stat_statements.max and its slot reused by another statement.
        let prev = counters(300, 3_000.0, 0, 0, 0);
        let cur = counters(10, 100.0, 0, 0, 0);
        assert_eq!(derive_delta(&prev, &cur, Duration::from_secs(2)), None);
    }

    #[test]
    fn zero_elapsed_time_yields_no_delta() {
        let prev = counters(100, 1_000.0, 0, 0, 0);
        let cur = counters(200, 2_000.0, 0, 0, 0);
        assert_eq!(derive_delta(&prev, &cur, Duration::ZERO), None);
    }

    #[test]
    fn a_null_query_id_falls_back_to_a_hash_of_the_text() {
        let key = statement_key(10, 20, None, "VACUUM orders");
        assert!(matches!(key.id, StatementId::TextHash(_)));

        // The same text must produce the same key, or no row ever matches
        // its previous sample and no delta is ever derived.
        assert_eq!(key, statement_key(10, 20, None, "VACUUM orders"));
    }

    #[test]
    fn different_query_texts_do_not_collide_onto_one_key() {
        assert_ne!(
            statement_key(10, 20, None, "VACUUM orders"),
            statement_key(10, 20, None, "VACUUM customers")
        );
    }

    #[test]
    fn a_present_query_id_is_used_in_preference_to_the_text() {
        assert_eq!(
            statement_key(10, 20, Some(42), "SELECT 1"),
            StatementKey {
                user_oid: 10,
                db_oid: 20,
                id: StatementId::QueryId(42),
            }
        );
    }

    #[test]
    fn the_same_query_id_under_a_different_role_is_a_different_statement() {
        assert_ne!(
            statement_key(10, 20, Some(42), "SELECT 1"),
            statement_key(11, 20, Some(42), "SELECT 1")
        );
    }

    fn statement(key: StatementKey, cumulative: StatementCounters) -> Statement {
        Statement {
            key,
            query: "SELECT 1".to_string(),
            user_name: None,
            database: None,
            cumulative,
            delta: None,
        }
    }

    #[test]
    fn apply_deltas_fills_in_rows_seen_in_the_previous_sample() {
        let key = statement_key(10, 20, Some(1), "SELECT 1");
        let mut statements = vec![statement(key, counters(200, 2_000.0, 0, 0, 0))];

        let mut previous = HashMap::new();
        previous.insert(key, counters(100, 1_000.0, 0, 0, 0));

        apply_deltas(&mut statements, &previous, Duration::from_secs(1));

        assert_eq!(statements[0].delta.unwrap().calls_per_sec, 100.0);
    }

    #[test]
    fn apply_deltas_leaves_a_new_statement_without_one() {
        let key = statement_key(10, 20, Some(2), "SELECT 2");
        let mut statements = vec![statement(key, counters(5, 50.0, 0, 0, 0))];

        apply_deltas(&mut statements, &HashMap::new(), Duration::from_secs(1));

        assert_eq!(statements[0].delta, None);
    }

    #[test]
    fn counters_by_key_indexes_every_statement() {
        let first = statement_key(10, 20, Some(1), "SELECT 1");
        let second = statement_key(10, 20, Some(2), "SELECT 2");
        let statements = vec![
            statement(first, counters(1, 1.0, 0, 0, 0)),
            statement(second, counters(2, 2.0, 0, 0, 0)),
        ];

        let indexed = counters_by_key(&statements);
        assert_eq!(indexed.len(), 2);
        assert_eq!(indexed[&second].calls, 2);
    }
}
```

Register the module: add `pub mod statements;` to `src/collector/mod.rs`, after `pub mod snapshot;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib collector::statements`
Expected: FAIL — `cannot find type 'StatementCounters' in this scope`.

- [ ] **Step 3: Implement the types and the derivation**

Insert above the `#[cfg(test)]` block in `src/collector/statements.rs`:

```rust
/// Top statements by cumulative execution time.
///
/// Three details worth keeping in view:
///   * `wal_bytes` is `numeric`; casting to float8 avoids pulling in a
///     decimal crate for one approximate size column.
///   * the `NOT LIKE` filter excludes this very statement from the report it
///     produces. It also excludes genuine user queries that mention the view
///     by name — accepted, because the alternative is the monitor
///     permanently ranking itself.
///   * `pg_roles` and `pg_database` are LEFT JOINed: a role or database
///     dropped after the statement was recorded leaves the OID dangling.
pub const STATEMENTS_SQL: &str = "\
SELECT s.queryid,
       s.userid::int8      AS user_oid,
       s.dbid::int8        AS db_oid,
       r.rolname::text     AS user_name,
       d.datname::text     AS database,
       left(s.query, 2000) AS query,
       s.calls,
       s.total_exec_time,
       s.rows,
       s.shared_blks_hit,
       s.shared_blks_read,
       s.shared_blks_dirtied,
       s.shared_blks_written,
       s.temp_blks_read,
       s.temp_blks_written,
       s.wal_bytes::float8 AS wal_bytes
  FROM pg_stat_statements s
  LEFT JOIN pg_roles    r ON r.oid = s.userid
  LEFT JOIN pg_database d ON d.oid = s.dbid
 WHERE s.query NOT LIKE '%pg_stat_statements%'
 ORDER BY s.total_exec_time DESC
 LIMIT $1";

/// How a statement is identified across samples. `queryid` is NULL for some
/// utility statements, so those fall back to a hash of the query text —
/// without a stable key no row can be matched to its previous reading and no
/// delta can be derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatementId {
    QueryId(i64),
    TextHash(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatementKey {
    pub user_oid: i64,
    pub db_oid: i64,
    pub id: StatementId,
}

pub fn statement_key(
    user_oid: i64,
    db_oid: i64,
    query_id: Option<i64>,
    query: &str,
) -> StatementKey {
    let id = match query_id {
        Some(query_id) => StatementId::QueryId(query_id),
        None => {
            let mut hasher = DefaultHasher::new();
            query.hash(&mut hasher);
            StatementId::TextHash(hasher.finish())
        }
    };
    StatementKey {
        user_oid,
        db_oid,
        id,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StatementCounters {
    pub calls: i64,
    pub total_exec_time_ms: f64,
    pub rows: i64,
    pub shared_blks_hit: i64,
    pub shared_blks_read: i64,
    pub shared_blks_dirtied: i64,
    pub shared_blks_written: i64,
    pub temp_blks_read: i64,
    pub temp_blks_written: i64,
    pub wal_bytes: f64,
}

impl StatementCounters {
    /// True if any counter is lower than in `previous`, which means the
    /// statistics were reset or this entry was evicted and its slot reused.
    pub fn went_backwards_from(&self, previous: &Self) -> bool {
        self.calls < previous.calls
            || self.total_exec_time_ms < previous.total_exec_time_ms
            || self.rows < previous.rows
            || self.shared_blks_hit < previous.shared_blks_hit
            || self.shared_blks_read < previous.shared_blks_read
            || self.shared_blks_dirtied < previous.shared_blks_dirtied
            || self.shared_blks_written < previous.shared_blks_written
            || self.temp_blks_read < previous.temp_blks_read
            || self.temp_blks_written < previous.temp_blks_written
            || self.wal_bytes < previous.wal_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatementDelta {
    pub calls_per_sec: f64,
    /// Milliseconds of execution time accrued per second. 1000 means this
    /// statement kept one core busy for the whole interval.
    pub exec_time_ms_per_sec: f64,
    /// Mean over the interval, not over the statement's lifetime. `None`
    /// when the statement was not called during the interval.
    pub mean_exec_time_ms: Option<f64>,
    pub rows_per_sec: f64,
    /// `None` when no shared blocks were touched in the interval.
    pub cache_hit_ratio: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub key: StatementKey,
    pub query: String,
    /// `None` when the recorded role or database has since been dropped.
    pub user_name: Option<String>,
    pub database: Option<String>,
    pub cumulative: StatementCounters,
    pub delta: Option<StatementDelta>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatementsSample {
    pub statements: Vec<Statement>,
}

/// Derive per-interval figures from two consecutive readings of one
/// statement. `None` when no rate can honestly be reported.
pub fn derive_delta(
    prev: &StatementCounters,
    cur: &StatementCounters,
    elapsed: Duration,
) -> Option<StatementDelta> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    if cur.went_backwards_from(prev) {
        return None;
    }

    let calls_delta = cur.calls - prev.calls;
    let time_delta = cur.total_exec_time_ms - prev.total_exec_time_ms;
    let hit_delta = cur.shared_blks_hit - prev.shared_blks_hit;
    let read_delta = cur.shared_blks_read - prev.shared_blks_read;
    let block_delta = hit_delta + read_delta;

    Some(StatementDelta {
        calls_per_sec: calls_delta as f64 / secs,
        exec_time_ms_per_sec: time_delta / secs,
        mean_exec_time_ms: if calls_delta > 0 {
            Some(time_delta / calls_delta as f64)
        } else {
            None
        },
        rows_per_sec: (cur.rows - prev.rows) as f64 / secs,
        cache_hit_ratio: if block_delta > 0 {
            Some(hit_delta as f64 / block_delta as f64)
        } else {
            None
        },
    })
}

/// Fills in `delta` for every statement that was present in the previous
/// slow sample. Statements new since then keep `None`.
pub fn apply_deltas(
    statements: &mut [Statement],
    previous: &HashMap<StatementKey, StatementCounters>,
    elapsed: Duration,
) {
    for statement in statements.iter_mut() {
        statement.delta = previous
            .get(&statement.key)
            .and_then(|prev| derive_delta(prev, &statement.cumulative, elapsed));
    }
}

pub fn counters_by_key(statements: &[Statement]) -> HashMap<StatementKey, StatementCounters> {
    statements
        .iter()
        .map(|statement| (statement.key, statement.cumulative))
        .collect()
}

pub fn map_statement(row: &Row) -> Statement {
    let user_oid: i64 = row.get("user_oid");
    let db_oid: i64 = row.get("db_oid");
    let query_id: Option<i64> = row.get("queryid");
    // A role without pg_monitor sees the literal text
    // "<insufficient privilege>" rather than NULL, but map defensively: the
    // parser must never panic on server output.
    let query: String = row.get::<_, Option<String>>("query").unwrap_or_default();

    Statement {
        key: statement_key(user_oid, db_oid, query_id, &query),
        query,
        user_name: row.get("user_name"),
        database: row.get("database"),
        cumulative: StatementCounters {
            calls: row.get("calls"),
            total_exec_time_ms: row.get("total_exec_time"),
            rows: row.get("rows"),
            shared_blks_hit: row.get("shared_blks_hit"),
            shared_blks_read: row.get("shared_blks_read"),
            shared_blks_dirtied: row.get("shared_blks_dirtied"),
            shared_blks_written: row.get("shared_blks_written"),
            temp_blks_read: row.get("temp_blks_read"),
            temp_blks_written: row.get("temp_blks_written"),
            wal_bytes: row.get("wal_bytes"),
        },
        delta: None,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib collector::statements`
Expected: PASS, 13 tests.

- [ ] **Step 5: Prove the SQL runs on 14 and 18**

In `tests/portability.rs`, add this helper after `connect`:

```rust
/// A container with pg_stat_statements preloaded and the extension created.
/// The library must be in shared_preload_libraries before the server starts;
/// CREATE EXTENSION alone is not enough.
async fn connect_with_statements(
    tag: &str,
) -> (
    tokio_postgres::Client,
    testcontainers::ContainerAsync<Postgres>,
) {
    let container = Postgres::default()
        .with_tag(tag)
        .with_cmd(["postgres", "-c", "shared_preload_libraries=pg_stat_statements"])
        .start()
        .await
        .expect("failed to start the PostgreSQL container");
    let client = connect_as(&container, "postgres", "postgres").await;
    client
        .batch_execute("CREATE EXTENSION pg_stat_statements")
        .await
        .expect("failed to create the extension");
    (client, container)
}
```

Then append these tests:

```rust
async fn assert_statements_sql_runs(tag: &str) {
    let (client, _container) = connect_with_statements(tag).await;

    let probe = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("probe failed");
    assert!(
        map_server_info(&probe).statements.is_available(),
        "the extension should probe as available once created"
    );

    // Give pg_stat_statements something of our own to record.
    client
        .batch_execute("SELECT 1; SELECT 1; SELECT 1")
        .await
        .expect("failed to run a sample workload");

    let rows = client
        .query(STATEMENTS_SQL, &[&200i64])
        .await
        .expect("pg_stat_statements query failed");
    assert!(!rows.is_empty(), "pg_stat_statements returned no rows");

    let statements: Vec<_> = rows.iter().map(map_statement).collect();
    assert!(
        statements.iter().all(|s| s.cumulative.calls > 0),
        "every recorded statement should have been called at least once"
    );
    assert!(
        statements.iter().all(|s| s.delta.is_none()),
        "a single sample has nothing to derive a delta from"
    );
}

#[tokio::test]
async fn statements_sql_runs_on_postgres_14() {
    assert_statements_sql_runs("14").await;
}

#[tokio::test]
async fn statements_sql_runs_on_postgres_18() {
    assert_statements_sql_runs("18").await;
}

#[tokio::test]
async fn a_delta_is_derived_across_two_statement_samples() {
    let (client, _container) = connect_with_statements("18").await;

    let first: Vec<_> = client
        .query(STATEMENTS_SQL, &[&200i64])
        .await
        .expect("first statements query failed")
        .iter()
        .map(map_statement)
        .collect();
    let previous = counters_by_key(&first);

    client
        .batch_execute("SELECT count(*) FROM pg_class")
        .await
        .expect("failed to run a workload between samples");

    let mut second: Vec<_> = client
        .query(STATEMENTS_SQL, &[&200i64])
        .await
        .expect("second statements query failed")
        .iter()
        .map(map_statement)
        .collect();
    apply_deltas(&mut second, &previous, Duration::from_secs(1));

    assert!(
        second.iter().any(|s| s.delta.is_some()),
        "at least one statement seen in both samples should carry a delta"
    );
}
```

Add the imports these need at the top of `tests/portability.rs`:

```rust
use std::time::Duration;

use mission_centre_pg::collector::statements::{
    apply_deltas, counters_by_key, map_statement, STATEMENTS_SQL,
};
```

- [ ] **Step 6: Run the container tests**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
```

Expected: PASS, 7 tests.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add src/collector/statements.rs src/collector/mod.rs tests/portability.rs
git commit -m "feat: pg_stat_statements sampling with per-interval deltas"
```

---

## Task 4: `collector/relations.rs` — table and index statistics

**Files:**
- Create: `src/collector/relations.rs`
- Modify: `src/collector/mod.rs`
- Modify: `tests/portability.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1–3.
- Produces:
  - `TABLES_SQL: &str`, `INDEXES_SQL: &str` — each takes one `$1` parameter, an `i64` row limit
  - `TableStats` with `dead_tuple_ratio(&self) -> Option<f64>` and `sequential_scan_ratio(&self) -> Option<f64>`
  - `IndexStats` with `is_unused(&self) -> bool`
  - `RelationsSample { tables: Vec<TableStats>, indexes: Vec<IndexStats> }`
  - `map_table_stats(row: &tokio_postgres::Row) -> TableStats`, `map_index_stats(row: &tokio_postgres::Row) -> IndexStats`

- [ ] **Step 1: Write the failing tests**

Create `src/collector/relations.rs` with the GPL header, then:

```rust
use tokio_postgres::Row;

#[cfg(test)]
mod tests {
    use super::*;

    fn table(live: i64, dead: i64, seq: i64, idx: i64) -> TableStats {
        TableStats {
            schema_name: "public".to_string(),
            table_name: "orders".to_string(),
            seq_scan: seq,
            seq_tup_read: 0,
            idx_scan: idx,
            idx_tup_fetch: 0,
            n_tup_ins: 0,
            n_tup_upd: 0,
            n_tup_del: 0,
            n_live_tup: live,
            n_dead_tup: dead,
            secs_since_vacuum: None,
            total_bytes: 0,
        }
    }

    #[test]
    fn dead_tuple_ratio_is_dead_over_the_total() {
        assert_eq!(table(750, 250, 0, 0).dead_tuple_ratio(), Some(0.25));
        assert_eq!(table(0, 100, 0, 0).dead_tuple_ratio(), Some(1.0));
    }

    #[test]
    fn an_empty_table_has_no_dead_tuple_ratio() {
        // Reporting 0% for a table with no tuples would claim a measurement
        // that was never taken, the same lie as a zero cache hit ratio.
        assert_eq!(table(0, 0, 0, 0).dead_tuple_ratio(), None);
    }

    #[test]
    fn sequential_scan_ratio_is_seq_over_all_scans() {
        assert_eq!(table(0, 0, 30, 70).sequential_scan_ratio(), Some(0.3));
        assert_eq!(table(0, 0, 10, 0).sequential_scan_ratio(), Some(1.0));
    }

    #[test]
    fn a_never_scanned_table_has_no_scan_ratio() {
        assert_eq!(table(0, 0, 0, 0).sequential_scan_ratio(), None);
    }

    fn index(scans: i64, primary: bool, unique: bool) -> IndexStats {
        IndexStats {
            schema_name: "public".to_string(),
            table_name: "orders".to_string(),
            index_name: "orders_pkey".to_string(),
            idx_scan: scans,
            idx_tup_read: 0,
            idx_tup_fetch: 0,
            bytes: 0,
            is_primary: primary,
            is_unique: unique,
            is_valid: true,
        }
    }

    #[test]
    fn an_index_with_no_scans_and_no_constraint_is_unused() {
        assert!(index(0, false, false).is_unused());
    }

    #[test]
    fn a_scanned_index_is_not_unused() {
        assert!(!index(1, false, false).is_unused());
    }

    #[test]
    fn an_unscanned_primary_key_is_not_reported_as_unused() {
        // Including these makes the report useless: a primary key is not a
        // removal candidate however few scans it has served.
        assert!(!index(0, true, true).is_unused());
    }

    #[test]
    fn an_unscanned_unique_index_is_not_reported_as_unused() {
        assert!(!index(0, false, true).is_unused());
    }
}
```

Register the module: add `pub mod relations;` to `src/collector/mod.rs`, after `pub mod queries;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib collector::relations`
Expected: FAIL — `cannot find type 'TableStats' in this scope`.

- [ ] **Step 3: Implement the types, the SQL and the mappers**

Insert above the `#[cfg(test)]` block:

```rust
/// Table statistics for the connected database. `pg_stat_user_tables` is
/// per-database; there is no server-wide equivalent.
///
/// `idx_scan` is NULL for a table with no indexes, and COALESCE to zero is
/// correct: a table with no indexes has had no index scans. `GREATEST` over
/// the two vacuum timestamps is NULL only when neither route has ever
/// vacuumed the table, which is itself the interesting answer.
pub const TABLES_SQL: &str = "\
SELECT t.schemaname::text AS schema_name,
       t.relname::text    AS table_name,
       t.seq_scan,
       t.seq_tup_read,
       COALESCE(t.idx_scan, 0)      AS idx_scan,
       COALESCE(t.idx_tup_fetch, 0) AS idx_tup_fetch,
       t.n_tup_ins,
       t.n_tup_upd,
       t.n_tup_del,
       t.n_live_tup,
       t.n_dead_tup,
       EXTRACT(EPOCH FROM (now() - GREATEST(t.last_vacuum, t.last_autovacuum)))::float8
         AS secs_since_vacuum,
       pg_total_relation_size(t.relid)::int8 AS total_bytes
  FROM pg_stat_user_tables t
 ORDER BY total_bytes DESC
 LIMIT $1";

/// Index statistics joined to `pg_index` for the constraint flags. Those
/// flags are what stop every primary key being reported as an unused index.
pub const INDEXES_SQL: &str = "\
SELECT i.schemaname::text   AS schema_name,
       i.relname::text      AS table_name,
       i.indexrelname::text AS index_name,
       COALESCE(i.idx_scan, 0)      AS idx_scan,
       COALESCE(i.idx_tup_read, 0)  AS idx_tup_read,
       COALESCE(i.idx_tup_fetch, 0) AS idx_tup_fetch,
       pg_relation_size(i.indexrelid)::int8 AS bytes,
       x.indisprimary AS is_primary,
       x.indisunique  AS is_unique,
       x.indisvalid   AS is_valid
  FROM pg_stat_user_indexes i
  JOIN pg_index x ON x.indexrelid = i.indexrelid
 ORDER BY bytes DESC
 LIMIT $1";

#[derive(Debug, Clone, PartialEq)]
pub struct TableStats {
    pub schema_name: String,
    pub table_name: String,
    pub seq_scan: i64,
    pub seq_tup_read: i64,
    pub idx_scan: i64,
    pub idx_tup_fetch: i64,
    pub n_tup_ins: i64,
    pub n_tup_upd: i64,
    pub n_tup_del: i64,
    pub n_live_tup: i64,
    pub n_dead_tup: i64,
    /// `None` when neither a manual nor an automatic vacuum has ever run.
    pub secs_since_vacuum: Option<f64>,
    pub total_bytes: i64,
}

impl TableStats {
    /// Dead tuples as a fraction of all tuples. `None` for a table with no
    /// tuples at all — this is the component of bloat a statistic can see,
    /// which is why the column is named for dead tuples and not for bloat.
    pub fn dead_tuple_ratio(&self) -> Option<f64> {
        let total = self.n_live_tup + self.n_dead_tup;
        if total > 0 {
            Some(self.n_dead_tup as f64 / total as f64)
        } else {
            None
        }
    }

    /// Sequential scans as a fraction of all scans. `None` when the table has
    /// never been scanned by either route.
    pub fn sequential_scan_ratio(&self) -> Option<f64> {
        let total = self.seq_scan + self.idx_scan;
        if total > 0 {
            Some(self.seq_scan as f64 / total as f64)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexStats {
    pub schema_name: String,
    pub table_name: String,
    pub index_name: String,
    pub idx_scan: i64,
    pub idx_tup_read: i64,
    pub idx_tup_fetch: i64,
    pub bytes: i64,
    pub is_primary: bool,
    pub is_unique: bool,
    pub is_valid: bool,
}

impl IndexStats {
    /// Zero scans and backing no constraint. The flag checks are the point:
    /// an unscanned primary key is not a removal candidate, and including
    /// those rows drowns the real answers.
    pub fn is_unused(&self) -> bool {
        self.idx_scan == 0 && !self.is_primary && !self.is_unique
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelationsSample {
    pub tables: Vec<TableStats>,
    pub indexes: Vec<IndexStats>,
}

pub fn map_table_stats(row: &Row) -> TableStats {
    TableStats {
        schema_name: row.get("schema_name"),
        table_name: row.get("table_name"),
        seq_scan: row.get("seq_scan"),
        seq_tup_read: row.get("seq_tup_read"),
        idx_scan: row.get("idx_scan"),
        idx_tup_fetch: row.get("idx_tup_fetch"),
        n_tup_ins: row.get("n_tup_ins"),
        n_tup_upd: row.get("n_tup_upd"),
        n_tup_del: row.get("n_tup_del"),
        n_live_tup: row.get("n_live_tup"),
        n_dead_tup: row.get("n_dead_tup"),
        secs_since_vacuum: row.get("secs_since_vacuum"),
        total_bytes: row.get("total_bytes"),
    }
}

pub fn map_index_stats(row: &Row) -> IndexStats {
    IndexStats {
        schema_name: row.get("schema_name"),
        table_name: row.get("table_name"),
        index_name: row.get("index_name"),
        idx_scan: row.get("idx_scan"),
        idx_tup_read: row.get("idx_tup_read"),
        idx_tup_fetch: row.get("idx_tup_fetch"),
        bytes: row.get("bytes"),
        is_primary: row.get("is_primary"),
        is_unique: row.get("is_unique"),
        is_valid: row.get("is_valid"),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib collector::relations`
Expected: PASS, 8 tests.

- [ ] **Step 5: Prove the SQL runs on 14 and 18**

Append to `tests/portability.rs`:

```rust
async fn assert_relations_sql_runs(tag: &str) {
    let (client, _container) = connect(tag).await;

    // pg_stat_user_tables excludes system catalogues, so a stock container
    // has nothing to report until a user table exists.
    client
        .batch_execute(
            "CREATE TABLE orders (id bigserial PRIMARY KEY, note text);
             CREATE INDEX orders_note_idx ON orders (note);
             INSERT INTO orders (note) SELECT 'n' || g FROM generate_series(1, 500) g;
             DELETE FROM orders WHERE id % 5 = 0;
             ANALYZE orders;",
        )
        .await
        .expect("failed to create the sample schema");

    let rows = client
        .query(TABLES_SQL, &[&200i64])
        .await
        .expect("pg_stat_user_tables query failed");
    let tables: Vec<_> = rows.iter().map(map_table_stats).collect();
    let orders = tables
        .iter()
        .find(|t| t.table_name == "orders")
        .expect("the orders table should be reported");
    assert!(orders.total_bytes > 0, "the table should have a size");
    assert!(
        orders.dead_tuple_ratio().is_some(),
        "a table with tuples should have a dead-tuple ratio"
    );

    let rows = client
        .query(INDEXES_SQL, &[&200i64])
        .await
        .expect("pg_stat_user_indexes query failed");
    let indexes: Vec<_> = rows.iter().map(map_index_stats).collect();
    assert!(
        indexes.iter().any(|i| i.index_name == "orders_pkey" && i.is_primary),
        "the primary key should be reported and flagged"
    );
    assert!(
        indexes
            .iter()
            .any(|i| i.index_name == "orders_note_idx" && i.is_unused()),
        "the never-queried secondary index should be reported as unused"
    );
    assert!(
        !indexes
            .iter()
            .any(|i| i.index_name == "orders_pkey" && i.is_unused()),
        "an unscanned primary key must never be reported as unused"
    );
}

#[tokio::test]
async fn relations_sql_runs_on_postgres_14() {
    assert_relations_sql_runs("14").await;
}

#[tokio::test]
async fn relations_sql_runs_on_postgres_18() {
    assert_relations_sql_runs("18").await;
}
```

Add the import at the top of `tests/portability.rs`:

```rust
use mission_centre_pg::collector::relations::{
    map_index_stats, map_table_stats, INDEXES_SQL, TABLES_SQL,
};
```

- [ ] **Step 6: Run the container tests**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
```

Expected: PASS, 9 tests.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add src/collector/relations.rs src/collector/mod.rs tests/portability.rs
git commit -m "feat: table and index statistics sampling"
```

---

## Task 5: The slow tier in the collector

**Files:**
- Modify: `src/collector/snapshot.rs`
- Modify: `src/collector/worker.rs`
- Modify: `data/io.github.paulsnow.MissionCentrePg.gschema.xml`
- Modify: `src/window.rs`

**Interfaces:**
- Consumes: `StatementsSample`, `counters_by_key`, `apply_deltas`, `map_statement`, `STATEMENTS_SQL`, `StatementKey`, `StatementCounters` (Task 3); `RelationsSample`, `map_table_stats`, `map_index_stats`, `TABLES_SQL`, `INDEXES_SQL` (Task 4); `ServerInfo.statements` (Task 2).
- Produces:
  - `Snapshot.statements: Option<Result<StatementsSample, CollectorError>>`
  - `Snapshot.relations: Option<Result<RelationsSample, CollectorError>>`
  - `CollectorConfig { interval: Duration, slow_interval: Duration, statements_limit: i64, relations_limit: i64 }`
  - `spawn(params: ConnectionParams, password: String, config: CollectorConfig) -> CollectorHandle` — **signature change**
  - `is_slow_tick(last_slow: Option<Instant>, now: Instant, slow_interval: Duration) -> bool`

**Note on module cycles:** `snapshot.rs` will import `CollectorError` from `worker.rs`, which already imports `Snapshot` from `snapshot.rs`. Rust permits mutual references between modules of one crate; this compiles and is intentional rather than a mistake to be refactored around.

- [ ] **Step 1: Write the failing test for the tick schedule**

Append to the `mod tests` block at the bottom of `src/collector/worker.rs`:

```rust
    #[test]
    fn the_first_sample_of_a_connection_always_runs_the_slow_tier() {
        // Otherwise both heavy pages sit blank for the whole slow interval
        // after connecting.
        let now = Instant::now();
        assert!(is_slow_tick(None, now, Duration::from_secs(10)));
    }

    #[test]
    fn the_slow_tier_waits_for_its_interval() {
        let now = Instant::now();
        let recent = now - Duration::from_secs(3);
        assert!(!is_slow_tick(Some(recent), now, Duration::from_secs(10)));
    }

    #[test]
    fn the_slow_tier_runs_once_the_interval_has_elapsed() {
        let now = Instant::now();
        let stale = now - Duration::from_secs(11);
        assert!(is_slow_tick(Some(stale), now, Duration::from_secs(10)));
    }

    #[test]
    fn a_slow_tier_query_error_degrades_one_page_not_the_connection() {
        // A permission error on pg_stat_statements must not count towards
        // the three-strike disconnect.
        let classified = classify_slow(Err::<(), _>(CollectorError::Query(
            "permission denied for view pg_stat_statements".to_string(),
        )));
        assert!(matches!(classified, Ok(Err(CollectorError::Query(_)))));
    }

    #[test]
    fn a_slow_tier_timeout_still_fails_the_sample() {
        assert!(matches!(
            classify_slow(Err::<(), _>(CollectorError::Timeout)),
            Err(CollectorError::Timeout)
        ));
    }

    #[test]
    fn a_slow_tier_connection_loss_still_fails_the_sample() {
        assert!(matches!(
            classify_slow(Err::<(), _>(CollectorError::LostConnection)),
            Err(CollectorError::LostConnection)
        ));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib collector::worker`
Expected: FAIL — `cannot find function 'is_slow_tick' in this scope`.

- [ ] **Step 3: Add the two new `Snapshot` fields**

In `src/collector/snapshot.rs`, add these imports below `use std::time::Instant;`:

```rust
use super::relations::RelationsSample;
use super::statements::StatementsSample;
use super::worker::CollectorError;
```

and extend the struct:

```rust
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub taken_at: Instant,
    pub totals: DatabaseCounters,
    pub rates: Option<DatabaseRates>,
    pub connected_database_size_bytes: Option<i64>,
    pub session_counts: SessionCounts,
    pub sessions: Vec<Session>,
    pub settings: ServerSettings,
    /// `None` on a fast tick — the page keeps its previous contents.
    /// `Err` carries the reason the page renders in place of its table.
    pub statements: Option<Result<StatementsSample, CollectorError>>,
    pub relations: Option<Result<RelationsSample, CollectorError>>,
}
```

- [ ] **Step 4: Implement the scheduling and classification helpers**

In `src/collector/worker.rs`, extend the imports:

```rust
use std::collections::HashMap;

use crate::collector::relations::{
    map_index_stats, map_table_stats, RelationsSample, INDEXES_SQL, TABLES_SQL,
};
use crate::collector::statements::{
    apply_deltas, counters_by_key, map_statement, StatementCounters, StatementKey,
    StatementsSample, STATEMENTS_SQL,
};
```

Add, immediately after `backoff_delay`:

```rust
/// How the collector is configured for one connection. A struct rather than
/// four positional arguments, since three of the four are durations or
/// limits that would be easy to transpose.
#[derive(Debug, Clone, Copy)]
pub struct CollectorConfig {
    pub interval: Duration,
    pub slow_interval: Duration,
    pub statements_limit: i64,
    pub relations_limit: i64,
}

/// True when this tick should also run the slow tier: always for the first
/// sample of a connection, so the heavy pages populate immediately, then
/// once `slow_interval` has elapsed.
pub fn is_slow_tick(last_slow: Option<Instant>, now: Instant, slow_interval: Duration) -> bool {
    match last_slow {
        None => true,
        Some(previous) => now.duration_since(previous) >= slow_interval,
    }
}

/// A slow-tier failure degrades one page rather than the connection. Only a
/// timeout or a lost connection is allowed to fail the whole sample; a query
/// error — insufficient privilege, an extension dropped mid-session, a
/// relation dropped between the catalogue read and the size call — is
/// captured into the snapshot for its page to render.
fn classify_slow<T>(
    result: Result<T, CollectorError>,
) -> Result<Result<T, CollectorError>, CollectorError> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(error) => match error {
            CollectorError::Timeout | CollectorError::LostConnection => Err(error),
            _ => Ok(Err(error)),
        },
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib collector::worker`
Expected: FAIL to compile at this point only if Step 6 is skipped — `sample` and `spawn` still need updating. Complete Step 6 before re-running.

- [ ] **Step 6: Thread the config and the slow tier through `spawn`, `run`, `sample_loop` and `sample`**

Replace `spawn` and `run`'s signature plumbing in `src/collector/worker.rs`:

```rust
pub fn spawn(
    params: ConnectionParams,
    password: String,
    config: CollectorConfig,
) -> CollectorHandle {
    let (event_tx, event_rx) = async_channel::bounded(32);
    let (stop_tx, stop_rx) = async_channel::bounded(1);

    std::thread::Builder::new()
        .name("mcpg-collector".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build the collector runtime");
            runtime.block_on(run(params, password, config, event_tx, stop_rx));
        })
        .expect("failed to spawn the collector thread");

    CollectorHandle {
        events: event_rx,
        stop: stop_tx,
    }
}
```

In `run`, change the parameter `interval: Duration` to `config: CollectorConfig`, and change the `sample_loop` call to pass both the config and whether statements are available:

```rust
                let statements_available = info.statements.is_available();
                if !emit(&events, &stop, CollectorEvent::Connected(info)).await {
                    return;
                }
                match sample_loop(&client, config, statements_available, &events, &stop).await {
```

Note the ordering: `info` is moved into the event, so read `statements.is_available()` before emitting.

Replace `sample_loop` with:

```rust
async fn sample_loop(
    client: &Client,
    config: CollectorConfig,
    statements_available: bool,
    events: &async_channel::Sender<CollectorEvent>,
    stop: &async_channel::Receiver<()>,
) -> Exit {
    let mut previous: Option<(DatabaseCounters, Instant)> = None;
    let mut previous_statements: Option<(HashMap<StatementKey, StatementCounters>, Instant)> = None;
    let mut last_slow: Option<Instant> = None;
    let mut consecutive_failures = 0u32;
    let mut had_success = false;

    loop {
        if stop_requested(stop) {
            return Exit::Stopped;
        }

        let slow = is_slow_tick(last_slow, Instant::now(), config.slow_interval).then_some(SlowTier {
            statements_available,
            statements_limit: config.statements_limit,
            relations_limit: config.relations_limit,
            previous_statements: previous_statements
                .as_ref()
                .map(|(counters, at)| (counters, *at)),
        });

        match sample(client, previous, slow).await {
            Ok(snapshot) => {
                consecutive_failures = 0;
                had_success = true;
                previous = Some((snapshot.totals, snapshot.taken_at));
                if let Some(Ok(sample)) = snapshot.statements.as_ref() {
                    previous_statements =
                        Some((counters_by_key(&sample.statements), snapshot.taken_at));
                }
                // Mark the slow tier as run whether or not its queries
                // succeeded: retrying a failing view every two seconds would
                // turn one broken page into a load problem.
                if snapshot.statements.is_some() || snapshot.relations.is_some() {
                    last_slow = Some(snapshot.taken_at);
                }
                emit_sample(events, Box::new(snapshot));
            }
            Err(e) => {
                consecutive_failures += 1;
                if !emit(events, stop, CollectorEvent::Error(e)).await {
                    return Exit::Stopped;
                }
                if consecutive_failures >= FAILURES_BEFORE_DISCONNECT {
                    return Exit::Failed { had_success };
                }
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(config.interval) => {}
            _ = stop.recv() => return Exit::Stopped,
        }
    }
}

/// What the slow tier needs for one run. Present only on a slow tick.
struct SlowTier<'a> {
    statements_available: bool,
    statements_limit: i64,
    relations_limit: i64,
    previous_statements: Option<(&'a HashMap<StatementKey, StatementCounters>, Instant)>,
}
```

Extend `sample` — add the third parameter and the two new fields:

```rust
async fn sample(
    client: &Client,
    previous: Option<(DatabaseCounters, Instant)>,
    slow: Option<SlowTier<'_>>,
) -> Result<Snapshot, CollectorError> {
    let taken_at = Instant::now();

    // … the four Phase 1 queries are unchanged, up to and including `rates` …

    let (statements, relations) = match slow {
        None => (None, None),
        Some(slow) => {
            let statements = if slow.statements_available {
                Some(classify_slow(
                    sample_statements(client, &slow, taken_at).await,
                )?)
            } else {
                // The page learns from ServerInfo that the extension is
                // missing; there is nothing to report here.
                None
            };
            let relations = Some(classify_slow(
                sample_relations(client, slow.relations_limit).await,
            )?);
            (statements, relations)
        }
    };

    Ok(Snapshot {
        taken_at,
        totals,
        rates,
        connected_database_size_bytes,
        session_counts,
        sessions,
        settings,
        statements,
        relations,
    })
}

async fn sample_statements(
    client: &Client,
    slow: &SlowTier<'_>,
    taken_at: Instant,
) -> Result<StatementsSample, CollectorError> {
    let rows = client
        .query(STATEMENTS_SQL, &[&slow.statements_limit])
        .await
        .map_err(map_query_error)?;
    let mut statements: Vec<_> = rows.iter().map(map_statement).collect();

    if let Some((previous, previous_at)) = slow.previous_statements {
        apply_deltas(
            &mut statements,
            previous,
            taken_at.duration_since(previous_at),
        );
    }

    Ok(StatementsSample { statements })
}

async fn sample_relations(
    client: &Client,
    limit: i64,
) -> Result<RelationsSample, CollectorError> {
    let rows = client
        .query(TABLES_SQL, &[&limit])
        .await
        .map_err(map_query_error)?;
    let tables = rows.iter().map(map_table_stats).collect();

    let rows = client
        .query(INDEXES_SQL, &[&limit])
        .await
        .map_err(map_query_error)?;
    let indexes = rows.iter().map(map_index_stats).collect();

    Ok(RelationsSample { tables, indexes })
}
```

- [ ] **Step 7: Add the GSettings keys**

In `data/io.github.paulsnow.MissionCentrePg.gschema.xml`, insert after the `sample-interval-ms` key:

```xml
    <key name="slow-sample-interval-ms" type="i">
      <range min="2000" max="300000"/>
      <default>10000</default>
      <summary>Minimum gap between slow-tier samples in milliseconds</summary>
      <description>Controls how often the Queries and Tables and Indexes pages are refreshed. These statistics are far more expensive to collect than the Overview and Sessions data, so they are sampled less often.</description>
    </key>
    <key name="statements-limit" type="i">
      <range min="10" max="1000"/>
      <default>200</default>
      <summary>Rows fetched from pg_stat_statements per slow sample</summary>
    </key>
    <key name="relations-limit" type="i">
      <range min="10" max="1000"/>
      <default>200</default>
      <summary>Rows fetched from each relation statistics view per slow sample</summary>
    </key>
```

- [ ] **Step 8: Build the config in the window**

In `src/window.rs`, change the import on line 28 to:

```rust
use mission_centre_pg::collector::worker::{spawn, CollectorConfig, CollectorEvent, CollectorHandle};
```

and replace the interval block inside `select_server` with:

```rust
        let settings = self.settings();
        let config = CollectorConfig {
            interval: std::time::Duration::from_millis(
                settings.int("sample-interval-ms").max(500) as u64
            ),
            slow_interval: std::time::Duration::from_millis(
                settings.int("slow-sample-interval-ms").max(2000) as u64
            ),
            statements_limit: settings.int("statements-limit").max(10) as i64,
            relations_limit: settings.int("relations-limit").max(10) as i64,
        };
```

and the spawn call with:

```rust
        let handle = spawn(params, password, config);
```

- [ ] **Step 9: Run the unit suite**

Run: `cargo test --lib`
Expected: PASS. `collector::worker` now has 6 new tests on top of its existing 5.

- [ ] **Step 10: Verify the slow tier actually fires**

```bash
cargo fmt
ninja -C build
glib-compile-schemas data/
GSETTINGS_SCHEMA_DIR=data ./build/src/mission-centre-pg
```

Connect to the local PostgreSQL 18.4 server. The Overview graphs must keep updating every two seconds — the slow tier must not visibly stall them. There is no UI for the new data yet; the check here is only that the application runs without stutter or error for a minute.

- [ ] **Step 11: Commit**

```bash
git add src/collector/snapshot.rs src/collector/worker.rs src/window.rs data/io.github.paulsnow.MissionCentrePg.gschema.xml
git commit -m "feat: sample statements and relation statistics on a slow tier"
```

---

## Task 6: Queries page

**Files:**
- Create: `resources/ui/queries_page.blp`
- Create: `src/pages/queries.rs`
- Modify: `resources/meson.build`, `resources/mission-centre-pg.gresource.xml`
- Modify: `src/pages/mod.rs`
- Modify: `resources/ui/window.blp`, `src/window.rs`

**Interfaces:**
- Consumes: `Table`, `Column` (Task 1); `StatementsAvailability` (Task 2); `Statement`, `StatementsSample` (Task 3); `Snapshot.statements` (Task 5); `format_rate` and `format_ratio` from `crate::pages::format`.
- Produces:
  - `McpgQueriesPage` with `set_statements_availability(&self, availability: &StatementsAvailability, database: &str)`, `update(&self, sample: &StatementsSample)`, `set_error(&self, message: &str)`

**Why the row type carries the mode:** `Column<T>` renderers are plain function pointers with no access to page state, so the delta/cumulative toggle cannot be read from inside them. The page therefore keeps the sampled `Vec<Statement>` and rebuilds a `Vec<QueryRow>` — each pairing a statement with the current mode — whenever the sample or the toggle changes. Toggling re-splices, which the table already does once per sample anyway.

- [ ] **Step 1: Write the blueprint**

Create `resources/ui/queries_page.blp`:

```blueprint
using Gtk 4.0;
using Adw 1;

template $McpgQueriesPage: Gtk.Box {
  orientation: vertical;

  Gtk.Stack stack {
    Gtk.StackPage {
      name: "table";

      child: Gtk.Box {
        orientation: vertical;

        Gtk.Box {
          spacing: 12;
          margin-start: 12;
          margin-end: 12;
          margin-top: 12;
          margin-bottom: 6;

          Gtk.SearchEntry filter_entry {
            hexpand: true;
            placeholder-text: _("Filter by query text, user or database");
          }

          Gtk.ToggleButton interval_toggle {
            label: _("Last interval");
            active: true;
            tooltip-text: _("Show activity during the last sampling interval rather than totals since the statistics were reset");
          }
        }

        Adw.Banner privilege_note {
          revealed: false;
          title: _("Query text for other users' statements is hidden without pg_monitor.");
        }

        Gtk.ScrolledWindow {
          vexpand: true;

          child: Gtk.ColumnView column_view {
            show-column-separators: true;
            reorderable: false;
          };
        }
      };
    }

    Gtk.StackPage {
      name: "unavailable";

      child: Adw.StatusPage status_page {
        icon-name: "dialog-information-symbolic";
        vexpand: true;
      };
    }
  }
}
```

Add `'ui/queries_page.blp',` to the `input: files(...)` list in `resources/meson.build`, and
`<file preprocess="xml-stripblanks">ui/queries_page.ui</file>` to `resources/mission-centre-pg.gresource.xml`.

- [ ] **Step 2: Write the failing test for the renderers**

Create `src/pages/queries.rs` with the GPL header, then this test module (the implementation follows in Step 4):

```rust
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
            mean_exec_time_ms: Some(5.0),
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
        assert_eq!(render_mean_time(&row(false, Some(delta()))), "5.0 ms");
    }

    #[test]
    fn interval_mode_shows_per_second_figures() {
        assert_eq!(render_calls(&row(true, Some(delta()))), "25/s");
        assert_eq!(render_rows(&row(true, Some(delta()))), "50/s");
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
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --lib pages::queries`
Expected: FAIL — `cannot find type 'QueryRow' in this scope`.

- [ ] **Step 4: Implement the row type, renderers and columns**

Insert above the test module in `src/pages/queries.rs`:

```rust
use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::collector::statements::{Statement, StatementsSample};
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

fn render_mean_time(row: &QueryRow) -> String {
    let mean = if row.delta_mode {
        row.statement.delta.and_then(|delta| delta.mean_exec_time_ms)
    } else {
        let counters = &row.statement.cumulative;
        if counters.calls > 0 {
            Some(counters.total_exec_time_ms / counters.calls as f64)
        } else {
            None
        }
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
    let counters = &row.statement.cumulative;
    let total = counters.shared_blks_hit + counters.shared_blks_read;
    format_ratio(if total > 0 {
        Some(counters.shared_blks_hit as f64 / total as f64)
    } else {
        None
    })
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
    let counters = &row.statement.cumulative;
    if counters.calls > 0 {
        counters.total_exec_time_ms / counters.calls as f64
    } else {
        0.0
    }
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
    let counters = &row.statement.cumulative;
    let total = counters.shared_blks_hit + counters.shared_blks_read;
    if total > 0 {
        counters.shared_blks_hit as f64 / total as f64
    } else {
        0.0
    }
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
```

- [ ] **Step 5: Run the renderer tests to verify they pass**

Run: `cargo test --lib pages::queries`
Expected: PASS, 6 tests.

- [ ] **Step 6: Implement the widget**

Append to `src/pages/queries.rs`, between the column list and the test module:

```rust
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
        let delta_mode = imp.delta_mode.get();
        let rows: Vec<QueryRow> = imp
            .statements
            .borrow()
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

    pub fn update(&self, sample: &StatementsSample) {
        self.imp()
            .statements
            .replace(sample.statements.clone());
        self.imp().stack.set_visible_child_name("table");
        self.rebuild_rows();
    }

    /// The extension is present but its query failed — insufficient
    /// privilege, or the extension dropped mid-session.
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
```

Register the page: in `src/pages/mod.rs` add `pub mod queries;` after `pub mod overview;`, and `pub use queries::McpgQueriesPage;` alongside the other re-exports.

- [ ] **Step 7: Wire the page into the window**

In `resources/ui/window.blp`, add a third `Adw.ViewStackPage` after the sessions one:

```blueprint
          Adw.ViewStackPage {
            name: "queries";
            title: _("Queries");
            icon-name: "format-justify-left-symbolic";
            child: $McpgQueriesPage queries_page {};
          }
```

In `src/window.rs`:

1. Extend the page import: `use mission_centre_pg::pages::{McpgOverviewPage, McpgQueriesPage, McpgSessionsPage};`
2. Add the template child to `imp::MissionCentrePgWindow`:

```rust
        #[template_child]
        pub queries_page: TemplateChild<McpgQueriesPage>,
```

3. Add `McpgQueriesPage::ensure_type();` to `class_init`, beside the other two.
4. Add a field to remember the connected database, since the status page names it:

```rust
        /// The database of the currently selected server, for messages that
        /// name it. Extension presence is a per-database property.
        pub connected_database: RefCell<String>,
```

5. In `select_server`, after `let Some(params) = … else { return; };`, add:

```rust
        imp.connected_database.replace(params.database.clone());
```

6. In `handle_event`, in the `Connected` arm, after the privilege lines:

```rust
                imp.queries_page.set_privilege_limited(limited);
                imp.queries_page.set_statements_availability(
                    &info.statements,
                    &imp.connected_database.borrow(),
                );
```

7. In the `Sample` arm, after `imp.sessions_page.update(&snapshot.sessions);`:

```rust
                // None means this was a fast tick, so the page keeps what it
                // has. Err means the slow tier ran and failed.
                match snapshot.statements.as_ref() {
                    Some(Ok(sample)) => imp.queries_page.update(sample),
                    Some(Err(error)) => {
                        imp.queries_page.set_error(&i18n(&error.to_string()))
                    }
                    None => {}
                }
```

- [ ] **Step 8: Build and verify against a live server**

```bash
cargo fmt
cargo test --lib
ninja -C build
glib-compile-schemas data/
GSETTINGS_SCHEMA_DIR=data ./build/src/mission-centre-pg
```

Verify, against the local PostgreSQL 18.4 server:

- If the extension is absent, the Queries page shows the status page naming the database, and no error banner appears. Overview and Sessions keep working.
- Install it (`CREATE EXTENSION pg_stat_statements` after adding it to `shared_preload_libraries` and restarting), reconnect, and confirm rows appear within ten seconds.
- Click **Total time** to sort; click **Last interval** to toggle to **Since reset** and confirm every number changes.
- Type in the filter and confirm the list narrows.

- [ ] **Step 9: Commit**

```bash
git add src/pages/queries.rs src/pages/mod.rs resources/ui/queries_page.blp resources/ui/window.blp resources/meson.build resources/mission-centre-pg.gresource.xml src/window.rs
git commit -m "feat: Queries page over pg_stat_statements"
```

---

## Task 7: Tables & Indexes page

**Files:**
- Create: `resources/ui/relations_page.blp`
- Create: `src/pages/relations.rs`
- Modify: `resources/meson.build`, `resources/mission-centre-pg.gresource.xml`
- Modify: `src/pages/mod.rs`
- Modify: `resources/ui/window.blp`, `src/window.rs`

**Interfaces:**
- Consumes: `Table`, `Column` (Task 1); `TableStats`, `IndexStats`, `RelationsSample` (Task 4); `Snapshot.relations` (Task 5); `format_rate`, `format_bytes`, `format_ratio`.
- Produces:
  - `McpgRelationsPage` with `update(&self, sample: &RelationsSample)`, `set_error(&self, message: &str)`, `set_database(&self, database: &str)`

- [ ] **Step 1: Write the blueprint**

Create `resources/ui/relations_page.blp`:

```blueprint
using Gtk 4.0;
using Adw 1;

template $McpgRelationsPage: Gtk.Box {
  orientation: vertical;

  Adw.Banner error_banner {
    revealed: false;
  }

  Gtk.Box {
    halign: center;
    margin-top: 12;

    Adw.ViewSwitcher {
      stack: inner_stack;
      policy: wide;
    }
  }

  Gtk.Label database_note {
    margin-top: 6;
    xalign: 0.5;
    styles ["dim-label", "caption"]
  }

  Adw.ViewStack inner_stack {
    vexpand: true;

    Adw.ViewStackPage {
      name: "tables";
      title: _("Tables");
      icon-name: "view-grid-symbolic";

      child: Gtk.Box {
        orientation: vertical;

        Gtk.SearchEntry tables_filter {
          hexpand: true;
          margin-start: 12;
          margin-end: 12;
          margin-top: 12;
          margin-bottom: 6;
          placeholder-text: _("Filter by schema or table name");
        }

        Gtk.ScrolledWindow {
          vexpand: true;

          child: Gtk.ColumnView tables_view {
            show-column-separators: true;
            reorderable: false;
          };
        }
      };
    }

    Adw.ViewStackPage {
      name: "indexes";
      title: _("Indexes");
      icon-name: "view-list-symbolic";

      child: Gtk.Box {
        orientation: vertical;

        Gtk.Box {
          spacing: 12;
          margin-start: 12;
          margin-end: 12;
          margin-top: 12;
          margin-bottom: 6;

          Gtk.SearchEntry indexes_filter {
            hexpand: true;
            placeholder-text: _("Filter by schema, table or index name");
          }

          Gtk.ToggleButton unused_only_toggle {
            label: _("Unused only");
            tooltip-text: _("Show only indexes with no scans that back no constraint");
          }
        }

        Gtk.ScrolledWindow {
          vexpand: true;

          child: Gtk.ColumnView indexes_view {
            show-column-separators: true;
            reorderable: false;
          };
        }
      };
    }
  }
}
```

Add `'ui/relations_page.blp',` to `resources/meson.build` and
`<file preprocess="xml-stripblanks">ui/relations_page.ui</file>` to the gresource manifest.

- [ ] **Step 2: Write the failing tests for the renderers**

Create `src/pages/relations.rs` with the GPL header, then:

```rust
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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib pages::relations`
Expected: FAIL — `cannot find function 'render_dead_ratio' in this scope`.

- [ ] **Step 4: Implement the renderers and columns**

Insert above the test module:

```rust
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
```

- [ ] **Step 5: Run the renderer tests to verify they pass**

Run: `cargo test --lib pages::relations`
Expected: PASS, 5 tests.

- [ ] **Step 6: Implement the widget**

Append, between the column lists and the test module:

```rust
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
            let tables = Table::attach(&self.tables_view.get(), TABLE_COLUMNS, move |table| {
                page.table_matches(table)
            });
            self.tables.replace(Some(tables));

            let page = self.obj().clone();
            let indexes = Table::attach(&self.indexes_view.get(), INDEX_COLUMNS, move |index| {
                page.index_matches(index)
            });
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
```

Register the page: in `src/pages/mod.rs` add `pub mod relations;` and `pub use relations::McpgRelationsPage;`.

- [ ] **Step 7: Wire the page into the window**

In `resources/ui/window.blp`, add a fourth `Adw.ViewStackPage`:

```blueprint
          Adw.ViewStackPage {
            name: "relations";
            title: _("Tables & Indexes");
            icon-name: "drive-harddisk-symbolic";
            child: $McpgRelationsPage relations_page {};
          }
```

In `src/window.rs`:

1. Extend the import to `use mission_centre_pg::pages::{McpgOverviewPage, McpgQueriesPage, McpgRelationsPage, McpgSessionsPage};`
2. Add the template child:

```rust
        #[template_child]
        pub relations_page: TemplateChild<McpgRelationsPage>,
```

3. Add `McpgRelationsPage::ensure_type();` to `class_init`.
4. In the `Connected` arm of `handle_event`, alongside the queries wiring:

```rust
                imp.relations_page
                    .set_database(&imp.connected_database.borrow());
```

5. In the `Sample` arm, after the statements match:

```rust
                match snapshot.relations.as_ref() {
                    Some(Ok(sample)) => imp.relations_page.update(sample),
                    Some(Err(error)) => {
                        imp.relations_page.set_error(&i18n(&error.to_string()))
                    }
                    None => {}
                }
```

- [ ] **Step 8: Build and verify against a live server**

```bash
cargo fmt
cargo test --lib
ninja -C build
glib-compile-schemas data/
GSETTINGS_SCHEMA_DIR=data ./build/src/mission-centre-pg
```

Verify against a database with user tables (create one if the local server has none — the `orders` schema from Task 4 Step 5 works):

- The Tables tab lists tables with sizes, live and dead tuple counts and a Dead % column.
- The subtitle names the connected database.
- Clicking **Size** sorts by size numerically, not by the formatted text.
- The Indexes tab lists indexes; enabling **Unused only** hides primary keys and keeps the never-scanned secondary index.

- [ ] **Step 9: Commit**

```bash
git add src/pages/relations.rs src/pages/mod.rs resources/ui/relations_page.blp resources/ui/window.blp resources/meson.build resources/mission-centre-pg.gresource.xml src/window.rs
git commit -m "feat: Tables & Indexes page"
```

---

## Task 8: Full verification against the success criteria

**Files:** none created or modified unless a check fails.

**Interfaces:**
- Consumes: everything from Tasks 1–7.
- Produces: evidence, and a `.superpowers/sdd/progress.md` entry.

- [ ] **Step 1: Formatting and unit tests**

```bash
cargo fmt --check
cargo test --lib
```

Expected: `cargo fmt --check` silent; all unit tests pass. Record the count.

- [ ] **Step 2: Container tests on both version bounds**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
```

Expected: PASS, 9 tests, covering PostgreSQL 14 and 18 for the statements SQL, the relations SQL and the availability probe.

- [ ] **Step 3: Check no file has outgrown the ceiling**

```bash
wc -l src/*.rs src/**/*.rs | sort -rn | head -10
```

Expected: no file over ~800 lines. `src/widgets/graph_widget.rs` (787, vendored) is the largest and must stay unchanged. If `src/pages/queries.rs` or `src/collector/worker.rs` has crossed the line, split the status-page construction or the slow-tier sampling functions into their own module before continuing.

- [ ] **Step 4: Full build**

```bash
ninja -C build
glib-compile-schemas data/
```

Expected: builds clean, no Blueprint or GResource warnings.

- [ ] **Step 5: Walk the success criteria against a live server**

Run `GSETTINGS_SCHEMA_DIR=data ./build/src/mission-centre-pg` and confirm each of the spec's §13 criteria, noting the evidence for each:

1. Binary runs, window opens.
2. Queries page lists statements, sorts on a header click, filters on typed text, and the **Last interval / Since reset** toggle changes every number.
3. Against a server without the extension: status page with the remedy, no error banner, Overview and Sessions unaffected.
4. Tables tab shows size, live, dead, Dead % and scan counts; Indexes tab's **Unused only** excludes primary keys.
5. Overview graphs and the Sessions table keep updating at the two-second cadence while the slow tier runs — watch for ten seconds across a slow tick and confirm no stutter.
6. Sessions still sorts, filters and hides idle by default after the `table/` migration.
7. Connect as a role without `pg_monitor` (the `watcher` role from Phase 1's testing): the Queries page degrades — either an error or `<insufficient privilege>` text — while the connection survives and Overview keeps sampling.
8. Formatting, unit and container tests as recorded in Steps 1–2.

- [ ] **Step 6: Record the outcome**

Append a `=== PHASE 2 ===` section to `.superpowers/sdd/progress.md` recording, for each of the eight criteria, whether it was verified and with what evidence. Anything unverified is recorded as unverified, not as passing.

- [ ] **Step 7: Commit**

```bash
git add .superpowers/sdd/progress.md
git commit -m "docs: record Phase 2 verification"
```

---

## Self-Review Notes

Checked against `docs/superpowers/specs/2026-07-24-phase-2-queries-and-relations-design.md`:

- §3.1 slow cadence — Task 5, `is_slow_tick`, with the first-tick-always rule tested.
- §3.2 snapshot additions — Task 5 Step 3, including `StatementsSample`/`RelationsSample` defined in Tasks 3 and 4.
- §3.3 failure isolation — Task 5, `classify_slow`, with three tests covering the query/timeout/lost-connection split.
- §3.4 cost control — `LIMIT $1` in both SQL constants, `left(query, 2000)`, limits from GSettings (Task 5 Step 7).
- §4 availability probing — Task 2, including the 1.10-versus-1.8 lexical trap and the page gate in Task 6 Step 6.
- §5.1 identity and matching — Task 3, `statement_key` with the NULL-`queryid` fallback and a collision test.
- §5.2 cumulative and per-interval — Task 3 `derive_delta`/`apply_deltas`, Task 6 the toggle.
- §5.3 columns — Task 6 `COLUMNS`, with sort keys following the mode.
- §5.4 SQL — Task 3 Step 3, with all three recorded pitfalls in the doc comment.
- §5.5 ranking window — the limit is a setting, not a constant; documented in the spec rather than worked around here.
- §6.2/§6.3 tables and indexes — Task 4 for the data, Task 7 for the columns and the unused-only filter.
- §6.4 cumulative only — no toggle on the relations page, by design.
- §6.5 dropped relations — covered by `classify_slow`; no special handling, as specified.
- §7 shared table module — Task 1, Sessions migrated first so its tests validate the extraction.
- §8 persistence — Task 5 Step 7, three keys with the specified ranges and defaults.
- §11 error handling — the availability states in Task 6, the inline error paths in Tasks 6 and 7, the classification in Task 5.
- §12 testing — unit tests in Tasks 1–7, container tests in Tasks 2–4.
- §13 success criteria — Task 8 walks all eight.

Two soft spots, flagged rather than hidden:

1. **`spawn`'s signature change ripples.** Task 5 changes `spawn(params, password, interval)` to take a `CollectorConfig`. The only caller is `src/window.rs`, updated in the same task, but a stale call site elsewhere would surface as a compile error rather than silently — which is the desired failure mode.
2. **The `Adw.ViewSwitcher` inside a page is unusual.** Task 7's blueprint places a switcher in the page body rather than a header bar. If libadwaita rejects the binding to a stack declared later in the same template, declare `inner_stack` before the switcher box, or move the switcher into an `Adw.ToolbarView` top bar within the page.

---

## Execution Handoff

Two execution options:

1. **Subagent-Driven (recommended)** — a fresh subagent per task, reviewed between tasks.
2. **Inline Execution** — tasks executed in this session with checkpoints.

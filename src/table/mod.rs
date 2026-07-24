/* table/mod.rs
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

use std::any::Any;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::marker::PhantomData;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::i18n::i18n;

/// How a row renders as text in a column cell.
pub type Renderer<T> = fn(&T) -> String;
/// Extracts a numeric sort key from a row, for columns that must sort
/// numerically rather than lexically (so "10" sorts after "9").
pub type NumericKey<T> = fn(&T) -> f64;

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
        compare_rows(a.as_ref(), b.as_ref(), render, sort_key).into()
    });

    let view_column = gtk::ColumnViewColumn::new(Some(&i18n(column.title)), Some(factory));
    view_column.set_resizable(true);
    view_column.set_expand(column.expand);
    view_column.set_sorter(Some(&sorter));
    view.append_column(&view_column);
}

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
        let nine = Row {
            name: "a",
            count: 9,
        };
        let ten = Row {
            name: "b",
            count: 10,
        };

        assert_eq!(
            compare_rows(
                &nine,
                &ten,
                count as Renderer<Row>,
                Some(count_key as NumericKey<Row>)
            ),
            Ordering::Less
        );
        // Guard against a regression to lexical sorting: as text, "10" < "9",
        // which is the wrong order the numeric key exists to prevent.
        assert_eq!(count(&ten).cmp(&count(&nine)), Ordering::Less);
    }

    #[test]
    fn a_column_without_a_key_sorts_lexically() {
        let alice = Row {
            name: "alice",
            count: 2,
        };
        let bob = Row {
            name: "bob",
            count: 1,
        };
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
        let a = Row {
            name: "a",
            count: 1,
        };
        let b = Row {
            name: "b",
            count: 2,
        };
        assert_eq!(
            compare_rows(
                &a,
                &b,
                name as Renderer<Row>,
                Some(nan_key as NumericKey<Row>)
            ),
            Ordering::Equal
        );
    }
}

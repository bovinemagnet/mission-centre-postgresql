/* pages/replication.rs
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
use std::sync::atomic::{AtomicI32, Ordering};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::collector::replication::{ReplicationSample, Slot, Standby, Subscription, WalReceiver};
use crate::collector::worker::CollectorError;
use crate::i18n::{i18n, i18n_f};
use crate::pages::format::format_bytes;
use crate::table::{Column, Table};

/// The first version that reports how long a slot has been inactive. Verified
/// against containers rather than recalled: `conflicting` arrives in 16, but
/// `inactive_since` not until 17.
const INACTIVE_SINCE_VERSION: i32 = 170000;

/// The connected server's version, for the one column whose rendering depends
/// on it.
///
/// `Column::render` is a plain function pointer, not a closure, so it cannot
/// capture the page. A process-wide value is sound here because the window
/// samples one server at a time, and it is rewritten on every connect. The
/// alternative — making `Renderer<T>` a boxed closure — would change the
/// shared table widget for every page to serve one column.
static SERVER_VERSION: AtomicI32 = AtomicI32::new(0);

/// What the inactive-duration cell shows. A server too old to report it is a
/// different thing from a slot that is currently active, and neither may
/// render as a blank — a blank reads as "zero" or "the tool does not know".
pub fn inactive_cell(slot: &Slot, version_num: i32) -> String {
    if version_num < INACTIVE_SINCE_VERSION {
        return "—".to_string();
    }
    match slot.inactive_since_secs {
        Some(secs) if secs >= 3600.0 => format!("{:.0}h", secs / 3600.0),
        Some(secs) if secs >= 60.0 => format!("{:.0}m", secs / 60.0),
        Some(secs) => format!("{secs:.0}s"),
        None => "active".to_string(),
    }
}

/// Both units, because they answer different questions: seconds say how stale
/// the replica is, which a failover decision needs; bytes say how much
/// write-ahead log catching up will take, which a capacity decision needs.
pub fn lag_cell(standby: &Standby) -> String {
    match (standby.replay_lag_secs, standby.replay_lag_bytes) {
        (Some(secs), Some(bytes)) => format!("{secs:.1}s / {}", format_bytes(bytes)),
        (Some(secs), None) => format!("{secs:.1}s"),
        (None, Some(bytes)) => format_bytes(bytes),
        (None, None) => "—".to_string(),
    }
}

/// Which sections the page shows. A primary has no upstream and a standby has
/// no standbys of its own, so showing both would leave half the page
/// permanently empty.
pub fn visible_sections(sample: &ReplicationSample) -> Vec<&'static str> {
    let mut sections = Vec::new();
    if sample.in_recovery {
        sections.push("receiver");
    } else {
        sections.push("standbys");
    }
    sections.push("slots");
    if !sample.subscriptions.is_empty() || !sample.publications.is_empty() {
        sections.push("logical");
    }
    sections
}

/// The upstream summary, rendered as a sentence rather than a one-row table:
/// there is only ever one receiver, and a table of one row reads oddly.
pub fn receiver_summary(receiver: &WalReceiver) -> String {
    let mut lines = Vec::new();

    if let Some(host) = receiver.sender_host.as_deref() {
        lines.push(i18n_f("Streaming from {}", &[host]));
    }
    if let Some(status) = receiver.status.as_deref() {
        lines.push(i18n_f("Status {}", &[status]));
    }
    if let Some(secs) = receiver.replay_delay_secs {
        lines.push(i18n_f(
            "Replaying changes from {} seconds ago",
            &[&format!("{secs:.1}")],
        ));
    }
    if let (Some(received), Some(replayed)) = (
        receiver.received_lsn.as_deref(),
        receiver.replayed_lsn.as_deref(),
    ) {
        lines.push(i18n_f("Received {}, replayed {}", &[received, replayed]));
    }

    lines.join("\n")
}

const STANDBY_COLUMNS: &[Column<Standby>] = &[
    Column {
        title: "Application",
        render: |s| s.application_name.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Client",
        render: |s| s.client_addr.clone().unwrap_or_default(),
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
        title: "Sync",
        render: |s| s.sync_state.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Write lag",
        render: |s| match s.write_lag_secs {
            Some(secs) => format!("{secs:.1}s"),
            None => "—".to_string(),
        },
        sort_key: Some(|s| s.write_lag_secs.unwrap_or(0.0)),
        expand: false,
    },
    Column {
        title: "Flush lag",
        render: |s| match s.flush_lag_secs {
            Some(secs) => format!("{secs:.1}s"),
            None => "—".to_string(),
        },
        sort_key: Some(|s| s.flush_lag_secs.unwrap_or(0.0)),
        expand: false,
    },
    Column {
        title: "Replay behind",
        render: lag_cell,
        sort_key: Some(|s| s.replay_lag_bytes.unwrap_or(0) as f64),
        expand: true,
    },
];

const SUBSCRIPTION_COLUMNS: &[Column<Subscription>] = &[
    Column {
        title: "Subscription",
        render: |s| s.subname.clone(),
        sort_key: None,
        expand: true,
    },
    Column {
        title: "Worker",
        render: |s| s.worker_type.clone().unwrap_or_else(|| "—".to_string()),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Behind by",
        render: |s| match s.latest_end_lag_secs {
            Some(secs) => format!("{secs:.1}s"),
            None => "—".to_string(),
        },
        sort_key: Some(|s| s.latest_end_lag_secs.unwrap_or(0.0)),
        expand: false,
    },
    Column {
        title: "Apply errors",
        render: |s| match s.apply_error_count {
            Some(count) => count.to_string(),
            None => "—".to_string(),
        },
        sort_key: Some(|s| s.apply_error_count.unwrap_or(0) as f64),
        expand: false,
    },
    Column {
        title: "Sync errors",
        render: |s| match s.sync_error_count {
            Some(count) => count.to_string(),
            None => "—".to_string(),
        },
        sort_key: Some(|s| s.sync_error_count.unwrap_or(0) as f64),
        expand: false,
    },
];

fn standby_key(standby: &Standby) -> String {
    standby.pid.to_string()
}

fn subscription_key(subscription: &Subscription) -> String {
    subscription.subname.clone()
}

fn slot_key(slot: &Slot) -> String {
    slot.slot_name.clone()
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/replication_page.ui")]
    pub struct McpgReplicationPage {
        #[template_child]
        pub error_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub standbys_group: TemplateChild<gtk::Box>,
        #[template_child]
        pub standbys_empty: TemplateChild<gtk::Label>,
        #[template_child]
        pub standbys_view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub receiver_group: TemplateChild<gtk::Box>,
        #[template_child]
        pub receiver_detail: TemplateChild<gtk::Label>,
        #[template_child]
        pub slots_group: TemplateChild<gtk::Box>,
        #[template_child]
        pub slots_empty: TemplateChild<gtk::Label>,
        #[template_child]
        pub slots_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub slots_view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub logical_group: TemplateChild<gtk::Box>,
        #[template_child]
        pub subscriptions_view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub publications_note: TemplateChild<gtk::Label>,

        pub standbys: RefCell<Option<Table<Standby>>>,
        pub slots: RefCell<Option<Table<Slot>>>,
        pub subscriptions: RefCell<Option<Table<Subscription>>>,
        pub version_num: Cell<i32>,
        pub database: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgReplicationPage {
        const NAME: &'static str = "McpgReplicationPage";
        type Type = super::McpgReplicationPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgReplicationPage {
        fn constructed(&self) {
            self.parent_constructed();

            let standbys = Table::attach(
                &self.standbys_view.get(),
                STANDBY_COLUMNS,
                |_| true,
                standby_key,
            );
            self.standbys.replace(Some(standbys));

            let page = self.obj().clone();
            let slots = Table::attach(
                &self.slots_view.get(),
                page.slot_columns(),
                |_| true,
                slot_key,
            );
            self.slots.replace(Some(slots));

            let subscriptions = Table::attach(
                &self.subscriptions_view.get(),
                SUBSCRIPTION_COLUMNS,
                |_| true,
                subscription_key,
            );
            self.subscriptions.replace(Some(subscriptions));
        }
    }

    impl WidgetImpl for McpgReplicationPage {}
    impl BoxImpl for McpgReplicationPage {}
}

glib::wrapper! {
    pub struct McpgReplicationPage(ObjectSubclass<imp::McpgReplicationPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

/// Slot columns are a function rather than a constant so the inactive column
/// can consult the server version, which is not known at compile time.
static SLOT_COLUMNS: &[Column<Slot>] = &[
    Column {
        title: "Slot",
        render: |slot| slot.slot_name.clone(),
        sort_key: None,
        expand: true,
    },
    Column {
        title: "Type",
        render: |slot| slot.slot_type.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Plugin",
        render: |slot| slot.plugin.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Database",
        render: |slot| slot.database.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Active",
        render: |slot| {
            if slot.active {
                "yes".to_string()
            } else {
                "no".to_string()
            }
        },
        sort_key: None,
        expand: false,
    },
    Column {
        title: "WAL status",
        render: |slot| slot.wal_status.clone().unwrap_or_default(),
        sort_key: None,
        expand: false,
    },
    Column {
        title: "Safe WAL size",
        render: |slot| match slot.safe_wal_size {
            Some(bytes) => format_bytes(bytes),
            None => "—".to_string(),
        },
        sort_key: Some(|slot| slot.safe_wal_size.unwrap_or(0) as f64),
        expand: false,
    },
    Column {
        title: "Inactive",
        render: |slot| inactive_cell(slot, SERVER_VERSION.load(Ordering::Relaxed)),
        // An abandoned slot sorts to the top by duration as well as by the
        // default order, so the longest-abandoned is the most prominent.
        sort_key: Some(|slot| slot.inactive_since_secs.unwrap_or(0.0)),
        expand: false,
    },
];

impl McpgReplicationPage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn slot_columns(&self) -> &'static [Column<Slot>] {
        SLOT_COLUMNS
    }

    /// The connected database, named on the publications note because
    /// pg_publication is not a shared catalogue: publications in other
    /// databases are invisible from here, and silence would read as "none".
    pub fn set_database(&self, database: &str) {
        self.imp().database.replace(database.to_string());
    }

    pub fn set_version(&self, version_num: i32) {
        self.imp().version_num.set(version_num);
        SERVER_VERSION.store(version_num, Ordering::Relaxed);
    }

    pub fn update(&self, replication: Option<&Result<ReplicationSample, CollectorError>>) {
        let imp = self.imp();

        let sample = match replication {
            None => return,
            Some(Err(error)) => {
                imp.error_banner.set_title(&i18n(&error.to_string()));
                imp.error_banner.set_revealed(true);
                return;
            }
            Some(Ok(sample)) => sample,
        };
        imp.error_banner.set_revealed(false);

        let sections = visible_sections(sample);
        imp.standbys_group
            .set_visible(sections.contains(&"standbys"));
        imp.receiver_group
            .set_visible(sections.contains(&"receiver"));
        imp.logical_group.set_visible(sections.contains(&"logical"));

        if let Some(table) = imp.standbys.borrow().as_ref() {
            table.update(&sample.standbys);
        }
        imp.standbys_empty.set_visible(sample.standbys.is_empty());
        imp.standbys_view.set_visible(!sample.standbys.is_empty());

        match sample.receiver.as_ref() {
            Some(receiver) => imp.receiver_detail.set_text(&receiver_summary(receiver)),
            None => imp
                .receiver_detail
                .set_text(&i18n("No upstream connection is established.")),
        }

        if let Some(table) = imp.slots.borrow().as_ref() {
            table.update(&sample.slots);
        }
        imp.slots_empty.set_visible(sample.slots.is_empty());
        imp.slots_view.set_visible(!sample.slots.is_empty());

        // The version gate is stated rather than left as an empty column, so
        // "we cannot tell you" is never mistaken for "nothing to report".
        let too_old = imp.version_num.get() < INACTIVE_SINCE_VERSION;
        imp.slots_note
            .set_visible(too_old && !sample.slots.is_empty());
        if too_old {
            imp.slots_note
                .set_text(&i18n("Inactive duration requires PostgreSQL 17 or later."));
        }

        if let Some(table) = imp.subscriptions.borrow().as_ref() {
            table.update(&sample.subscriptions);
        }

        let database = imp.database.borrow().clone();
        let names: Vec<String> = sample
            .publications
            .iter()
            .map(|publication| publication.pubname.clone())
            .collect();
        imp.publications_note.set_text(&if names.is_empty() {
            i18n_f("No publications in {}.", &[&database])
        } else {
            i18n_f("Publications in {}: {}", &[&database, &names.join(", ")])
        });
    }
}

impl Default for McpgReplicationPage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(inactive_since_secs: Option<f64>) -> Slot {
        Slot {
            slot_name: "s".to_string(),
            slot_type: Some("physical".to_string()),
            plugin: None,
            database: None,
            active: inactive_since_secs.is_none(),
            wal_status: Some("reserved".to_string()),
            safe_wal_size: None,
            inactive_since_secs,
            conflicting: None,
        }
    }

    fn standby(replay_lag_secs: Option<f64>, replay_lag_bytes: Option<i64>) -> Standby {
        Standby {
            pid: 1,
            application_name: Some("walreceiver".to_string()),
            client_addr: None,
            state: Some("streaming".to_string()),
            sync_state: Some("async".to_string()),
            write_lag_secs: None,
            flush_lag_secs: None,
            replay_lag_secs,
            replay_lag_bytes,
        }
    }

    #[test]
    fn before_17_the_inactive_cell_states_that_the_server_cannot_report_it() {
        assert_eq!(inactive_cell(&slot(None), 140000), "—");
        assert_eq!(inactive_cell(&slot(Some(90.0)), 160000), "—");
    }

    #[test]
    fn an_active_slot_on_17_says_active_rather_than_a_duration() {
        assert_eq!(inactive_cell(&slot(None), 170000), "active");
    }

    #[test]
    fn an_inactive_slot_reports_its_duration_in_readable_units() {
        assert_eq!(inactive_cell(&slot(Some(45.0)), 170000), "45s");
        assert_eq!(inactive_cell(&slot(Some(90.0)), 170000), "2m");
        assert_eq!(inactive_cell(&slot(Some(7200.0)), 170000), "2h");
    }

    #[test]
    fn lag_shows_both_units_when_both_are_known() {
        let cell = lag_cell(&standby(Some(1.5), Some(1024)));
        assert!(cell.contains("1.5s"), "seconds are shown: {cell}");
        assert!(
            cell.contains("1.0 KiB") || cell.contains("1 KiB"),
            "bytes are shown: {cell}"
        );
    }

    #[test]
    fn lag_falls_back_rather_than_showing_a_blank() {
        assert_eq!(lag_cell(&standby(None, None)), "—");
        assert_eq!(lag_cell(&standby(Some(2.0), None)), "2.0s");
    }

    #[test]
    fn a_primary_shows_standbys_and_a_standby_shows_its_upstream() {
        let primary = ReplicationSample::default();
        assert!(visible_sections(&primary).contains(&"standbys"));
        assert!(!visible_sections(&primary).contains(&"receiver"));

        let standby = ReplicationSample {
            in_recovery: true,
            ..Default::default()
        };
        assert!(visible_sections(&standby).contains(&"receiver"));
        assert!(!visible_sections(&standby).contains(&"standbys"));
    }

    #[test]
    fn slots_are_always_shown_and_logical_only_when_it_is_used() {
        let sample = ReplicationSample::default();
        assert!(visible_sections(&sample).contains(&"slots"));
        assert!(!visible_sections(&sample).contains(&"logical"));
    }

    #[test]
    fn the_inactive_column_renders_through_the_version_gate() {
        let render = SLOT_COLUMNS
            .iter()
            .find(|column| column.title == "Inactive")
            .expect("the Inactive column exists")
            .render;

        SERVER_VERSION.store(140000, Ordering::Relaxed);
        assert_eq!(render(&slot(Some(90.0))), "—");

        SERVER_VERSION.store(180000, Ordering::Relaxed);
        assert_eq!(render(&slot(Some(90.0))), "2m");
    }

    #[test]
    fn the_upstream_summary_names_the_host_and_the_delay() {
        let receiver = WalReceiver {
            status: Some("streaming".to_string()),
            sender_host: Some("primary.internal".to_string()),
            received_lsn: Some("0/3000000".to_string()),
            replayed_lsn: Some("0/2F00000".to_string()),
            replay_delay_secs: Some(4.0),
        };
        let summary = receiver_summary(&receiver);

        assert!(summary.contains("primary.internal"));
        assert!(summary.contains("4.0"));
    }
}

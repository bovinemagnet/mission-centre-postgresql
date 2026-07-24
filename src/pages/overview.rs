/* pages/overview.rs
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

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib};

use crate::collector::snapshot::Snapshot;
use crate::i18n::i18n_f;
use crate::pages::format::{format_bytes, format_rate, format_ratio};
use crate::widgets::graph_widget::GraphWidget;
use crate::widgets::graph_widget_utils::DatasetGroup;

const DEFAULT_POINTS: u32 = 300;

/// A distinct base colour per graph, in the order `graphs()` returns them:
/// connections (blue), transactions (green), cache hit (purple), tuples
/// (orange). Without a base colour the widget draws in black, which is
/// invisible on a dark background.
const GRAPH_COLOURS: [(f32, f32, f32); 4] = [
    (0.30, 0.56, 0.90),
    (0.28, 0.76, 0.44),
    (0.68, 0.44, 0.90),
    (0.95, 0.60, 0.20),
];

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/overview_page.ui")]
    pub struct McpgOverviewPage {
        #[template_child]
        pub connections_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub connections_graph: TemplateChild<GraphWidget>,
        #[template_child]
        pub tps_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub tps_graph: TemplateChild<GraphWidget>,
        #[template_child]
        pub cache_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub cache_graph: TemplateChild<GraphWidget>,
        #[template_child]
        pub tuples_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub tuples_graph: TemplateChild<GraphWidget>,
        #[template_child]
        pub database_size_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub deadlocks_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub temp_value: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgOverviewPage {
        const NAME: &'static str = "McpgOverviewPage";
        type Type = super::McpgOverviewPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            GraphWidget::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgOverviewPage {
        fn constructed(&self) {
            self.parent_constructed();
            for (graph, &(r, g, b)) in self.obj().graphs().iter().zip(GRAPH_COLOURS.iter()) {
                graph.set_data_points(DEFAULT_POINTS);
                graph.add_dataset(DatasetGroup::new());
                graph.set_base_color(gdk::RGBA::new(r, g, b, 1.0));
            }
            // A ratio is always 0-100, so pin the scale rather than letting it
            // auto-fit and make a flat 99% line look dramatic.
            self.cache_graph.set_dataset_max_scale(0, 100.0);
        }
    }

    impl WidgetImpl for McpgOverviewPage {}
    impl BoxImpl for McpgOverviewPage {}
}

glib::wrapper! {
    pub struct McpgOverviewPage(ObjectSubclass<imp::McpgOverviewPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgOverviewPage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn graphs(&self) -> [GraphWidget; 4] {
        let imp = self.imp();
        [
            imp.connections_graph.get(),
            imp.tps_graph.get(),
            imp.cache_graph.get(),
            imp.tuples_graph.get(),
        ]
    }

    pub fn set_graph_points(&self, points: u32) {
        for graph in self.graphs() {
            graph.set_data_points(points);
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        let imp = self.imp();

        let connections = snapshot.session_counts.total() as f64;
        let max_connections = snapshot.settings.max_connections;
        imp.connections_value.set_text(&i18n_f(
            "{} / {}",
            &[&format_rate(connections), &max_connections.to_string()],
        ));
        imp.connections_graph
            .set_dataset_max_scale(0, max_connections as f32);
        imp.connections_graph
            .add_data_point(vec![vec![connections as f32]]);

        imp.database_size_value.set_text(
            &snapshot
                .connected_database_size_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "—".to_string()),
        );

        // The first sample after connecting has no previous reading, so there
        // are no rates yet. Push nothing rather than a fabricated zero.
        let Some(rates) = snapshot.rates else {
            for label in [
                &imp.tps_value,
                &imp.cache_value,
                &imp.tuples_value,
                &imp.deadlocks_value,
                &imp.temp_value,
            ] {
                label.set_text("—");
            }
            return;
        };

        imp.tps_value
            .set_text(&format_rate(rates.transactions_per_sec));
        // A rate is a delta divided by an elapsed duration; a near-zero
        // duration can in principle yield NaN or infinity. Narrowing that to
        // f32 and feeding it to the graph would draw a bogus spike, so skip
        // the push rather than lie about what was measured.
        if rates.transactions_per_sec.is_finite() {
            imp.tps_graph
                .add_data_point(vec![vec![rates.transactions_per_sec as f32]]);
        }

        imp.cache_value
            .set_text(&format_ratio(rates.cache_hit_ratio));
        if let Some(ratio) = rates.cache_hit_ratio {
            if ratio.is_finite() {
                imp.cache_graph
                    .add_data_point(vec![vec![(ratio * 100.0) as f32]]);
            }
        }

        imp.tuples_value
            .set_text(&format_rate(rates.tuples_returned_per_sec));
        if rates.tuples_returned_per_sec.is_finite() {
            imp.tuples_graph
                .add_data_point(vec![vec![rates.tuples_returned_per_sec as f32]]);
        }

        imp.deadlocks_value
            .set_text(&format_rate(rates.deadlocks_per_sec));
        // format_bytes takes an i64, so a non-finite rate must be caught
        // here: casting NaN/infinity to i64 saturates silently instead of
        // producing the em dash a fabricated measurement deserves.
        imp.temp_value
            .set_text(&if rates.temp_bytes_per_sec.is_finite() {
                format_bytes(rates.temp_bytes_per_sec as i64)
            } else {
                "—".to_string()
            });
    }

    /// Drop all four graph series. Called on a server switch before the new
    /// server's preload arrives, so one server's shape never bleeds into the
    /// next.
    pub fn reset(&self) {
        for graph in self.graphs() {
            graph.clear_datasets();
            graph.add_dataset(DatasetGroup::new());
        }
    }

    /// Fill the graph buffers from stored history so they open already drawn.
    /// Only the connections and cache-hit graphs are preloaded: the persisted
    /// schema (pg-console's) stores neither TPS nor tuple rates, so those two
    /// graphs build up live. Samples arrive oldest-first.
    pub fn preload(&self, samples: &[crate::history::SystemHistorySample]) {
        let imp = self.imp();
        if let Some(last) = samples.last() {
            imp.connections_graph
                .set_dataset_max_scale(0, last.max_connections as f32);
        }
        for sample in samples {
            imp.connections_graph
                .add_data_point(vec![vec![sample.total_connections as f32]]);
            if let Some(ratio) = sample.cache_hit_ratio {
                if ratio.is_finite() {
                    imp.cache_graph
                        .add_data_point(vec![vec![(ratio * 100.0) as f32]]);
                }
            }
        }
    }
}

impl Default for McpgOverviewPage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preload_and_reset_are_public_no_op_safe_signatures() {
        // Constructing the widget needs a GTK main context, which unit tests
        // do not have. This test exists to keep the method signatures honest
        // against the window's calls; behaviour is verified by running the app.
        fn _assert_signatures(
            page: &McpgOverviewPage,
            samples: &[crate::history::SystemHistorySample],
        ) {
            page.reset();
            page.preload(samples);
        }
    }
}

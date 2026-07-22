/* performance_page/widgets/graph_widget_utils.rs
 *
 * Copyright 2026 Mission Center Developers
 *
 * Vendored into Mission Centre PostgreSQL from Mission Center
 * (https://gitlab.com/mission-center-devs/mission-center) at commit 050213c,
 * originally src/performance_page/widgets/graph_widget_utils.rs.
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

use std::cmp::PartialEq;

use adw::gdk;
use gtk::gsk::{FillRule, PathBuilder, Stroke};
use gtk::prelude::SnapshotExt;
use gtk::Snapshot;

use crate::widgets::graph_widget::GraphWidget;

/// Upstream sources these from `crate::preferences`; Phase 1 has no
/// preferences module, so the same values live here.
pub const MAX_POINTS: i32 = 600;
pub const MIN_POINTS: i32 = 10;

/// Per-frame animation snapshot delivered to graph widgets.
#[derive(Clone, Copy, Debug)]
pub struct AnimationFrame {
    /// Animation progress within the current refresh cycle, in [0.0, 1.0].
    /// A value of 0.0 also signals the start of a new cycle — see
    /// GraphWidget::update_animation.
    pub progress: f32,
    /// Window-global gridline phase counter; consumed only when progress == 0.0.
    pub grid_offset: u32,
}

#[derive(Default, Clone, PartialEq)]
pub enum ScalingSettings {
    #[default]
    Fixed,
    ScaleUp,
    ScaleDown,
    ScaleUpDown,
    StickyUp,
    StickyDown,
    StickyUpDown,
    StickyUpDownEqualMagnitude,
    Stacking,
}

#[derive(Default, Clone, PartialEq)]
pub enum RoundingSettings {
    #[default]
    NoRounding,
    Pow2,
    Pow2Base10,
    Integer,
    Pow10,
}

impl RoundingSettings {
    fn round_up_to_next_power_of_two(num: f32) -> f32 {
        let num = num as u64;

        if num == 0 {
            return 0.;
        }

        let mut n = num - 1;
        n |= n >> 1;
        n |= n >> 2;
        n |= n >> 4;
        n |= n >> 8;
        n |= n >> 16;

        (n + 1) as f32
    }

    fn round_up_to_next_power_of_two_base_10(num: f32) -> f32 {
        if num == 0. {
            return 0.;
        }

        // take the power of two amount w.r.t. the last power of 1000
        let log1000 = (num.log10() / 3.) as i32;

        let num_below = 1000f32.powi(log1000);

        Self::round_up_to_next_power_of_two((num / num_below).ceil()).min(1000.) * num_below
    }

    fn round_up_to_next_power_of_ten(mut n: f32) -> f32 {
        let mut negative_multiplier = 1.;

        if n == 0. {
            return 0.;
        } else if n < 0. {
            negative_multiplier = -1.;
            n = n.abs()
        }

        let magnitude = 10_f32.powi(n.log10().floor() as i32);
        (n / magnitude).ceil() * magnitude * negative_multiplier
    }

    pub fn apply_up_rounding(&self, f: f32) -> f32 {
        match self {
            RoundingSettings::NoRounding => f,
            RoundingSettings::Pow2 => Self::round_up_to_next_power_of_two(f),
            RoundingSettings::Pow2Base10 => Self::round_up_to_next_power_of_two_base_10(f),
            RoundingSettings::Integer => f.ceil(),
            RoundingSettings::Pow10 => Self::round_up_to_next_power_of_ten(f),
        }
    }

    pub fn apply_down_rounding(&self, f: f32) -> f32 {
        match self {
            RoundingSettings::NoRounding => f,
            RoundingSettings::Integer => f.floor(),
            RoundingSettings::Pow10 => Self::round_up_to_next_power_of_ten(f),
            _ => f, // generally rounding down on logs is problematic
        }
    }
}

#[derive(Default, Clone, PartialEq)]
pub enum FillingSettings {
    #[default]
    FillToBottom,
    FillToTop,
    FillToZero,
    None,
}

#[derive(Clone)]
pub struct DatasetSettings {
    pub dashed: bool,
    pub visible: bool,
    pub fill: FillingSettings,
    pub opacity: f32,
    pub vertical_dropoff_lines: bool,

    pub scaling_settings: ScalingSettings,
    pub rounding_settings: RoundingSettings,
    pub low_watermark: f32,
    pub high_watermark: f32,

    // for example, when using pow2base10 scaling, we want to ensure we are using the correct reference point to calculate that nearest thousand.
    // we keep this as a float to not exclude other uses
    pub watermarking_multiplier: f32,

    pub following: Option<usize>,
    pub followed: Option<usize>,
}

#[derive(Clone)]
pub struct DatasetGroup {
    pub dataset_settings: DatasetSettings,

    pub datas: Vec<Dataset>,
}

impl DatasetGroup {
    pub fn new() -> Self {
        Self {
            dataset_settings: DatasetSettings {
                dashed: false,
                fill: Default::default(),
                visible: true,
                opacity: 100. / 255.,
                vertical_dropoff_lines: true,
                scaling_settings: Default::default(),
                rounding_settings: Default::default(),
                low_watermark: 0.0,
                high_watermark: 100.0,
                watermarking_multiplier: 1.,
                following: None,
                followed: None,
            },
            datas: vec![Dataset::default()],
        }
    }

    pub fn new_with_datas(d: Vec<Vec<(f32, f32)>>) -> Self {
        let mut datas = Vec::with_capacity(d.len());
        for v in d {
            datas.push(Dataset::new_with_data(v));
        }
        Self {
            dataset_settings: DatasetSettings {
                dashed: false,
                fill: Default::default(),
                visible: true,
                opacity: 100. / 255.,
                vertical_dropoff_lines: true,
                scaling_settings: Default::default(),
                rounding_settings: Default::default(),
                low_watermark: 0.0,
                high_watermark: 100.0,
                watermarking_multiplier: 1.,
                following: None,
                followed: None,
            },
            datas,
        }
    }

    pub fn new_with_fill(v: f32) -> Self {
        Self {
            dataset_settings: DatasetSettings {
                dashed: false,
                fill: Default::default(),
                visible: true,
                opacity: 100. / 255.,
                vertical_dropoff_lines: true,
                scaling_settings: Default::default(),
                rounding_settings: Default::default(),
                low_watermark: 0.0,
                high_watermark: 100.0,
                watermarking_multiplier: 1.,
                following: None,
                followed: None,
            },
            datas: vec![Dataset::new_with_fill(v)],
        }
    }
}

#[derive(Clone)]
pub struct Dataset {
    data: Vec<f32>,
    x_points: Vec<f32>,
    pub used_data: usize,
}

#[derive(Clone, Debug)]
pub struct DatasetPoints {
    x: f32,
    y: f32,
}

impl DatasetGroup {
    pub fn set_datasets(&mut self, sets: usize) {
        for _ in self.datas.len()..sets {
            self.datas.push(Dataset::default());
        }
    }

    pub fn add_data(&mut self, points: &Vec<f32>) {
        for (idx, set) in points.iter().enumerate() {
            self.update_single_scaling(idx, *set);
        }

        self.update_expensive_scaling();
    }

    pub fn reset_auto_scaling(&mut self) {
        // todo this better with down scaling support
        self.dataset_settings.high_watermark = 0.0;
        self.update_expensive_scaling()
    }

    fn update_expensive_scaling(&mut self) {
        match self.dataset_settings.scaling_settings {
            ScalingSettings::ScaleUp => {
                self.dataset_settings.high_watermark =
                    self.dataset_settings.rounding_settings.apply_up_rounding(
                        self.get_maximum() * self.dataset_settings.watermarking_multiplier,
                    ) / self.dataset_settings.watermarking_multiplier;
            }
            ScalingSettings::ScaleDown => {
                self.dataset_settings.low_watermark =
                    self.dataset_settings.rounding_settings.apply_down_rounding(
                        self.get_minimum() * self.dataset_settings.watermarking_multiplier,
                    ) / self.dataset_settings.watermarking_multiplier;
            }
            ScalingSettings::ScaleUpDown => {
                self.dataset_settings.high_watermark =
                    self.dataset_settings.rounding_settings.apply_up_rounding(
                        self.get_maximum() * self.dataset_settings.watermarking_multiplier,
                    ) / self.dataset_settings.watermarking_multiplier;
                self.dataset_settings.low_watermark =
                    self.dataset_settings.rounding_settings.apply_down_rounding(
                        self.get_minimum() * self.dataset_settings.watermarking_multiplier,
                    ) / self.dataset_settings.watermarking_multiplier;
            }
            ScalingSettings::StickyUp => {}
            ScalingSettings::StickyDown => {}
            ScalingSettings::StickyUpDown => {}
            ScalingSettings::StickyUpDownEqualMagnitude => {}
            ScalingSettings::Stacking => {}
            ScalingSettings::Fixed => {}
        }
    }

    pub fn apply_following_rules(&mut self, other: Option<&Self>) -> bool {
        let Some(other) = other else {
            return false;
        };

        let mut changed = false;
        if other.dataset_settings.high_watermark > self.dataset_settings.high_watermark {
            self.dataset_settings.high_watermark = other.dataset_settings.high_watermark;
            changed = true;
        }

        if other.dataset_settings.low_watermark < self.dataset_settings.low_watermark {
            self.dataset_settings.low_watermark = other.dataset_settings.low_watermark;
            changed = true;
        }

        changed
    }

    fn get_minimum(&self) -> f32 {
        self.datas
            .iter()
            .filter_map(|set| set.get_data_removed().iter().map(|f| *f).reduce(f32::min))
            .reduce(f32::min)
            .unwrap_or(self.dataset_settings.low_watermark)
    }

    fn get_maximum(&self) -> f32 {
        self.datas
            .iter()
            .filter_map(|set| set.get_data_removed().iter().map(|f| *f).reduce(f32::max))
            .reduce(f32::max)
            .unwrap_or(self.dataset_settings.high_watermark)
    }

    // do cheap updates whenever a new point is added
    fn update_single_scaling(&mut self, idx: usize, point: f32) {
        self.datas[idx].data.rotate_right(1);
        self.datas[idx].data[0] = point;

        // do scaling up
        match self.dataset_settings.scaling_settings {
            /* these require searching, wait for the expensive call */
            ScalingSettings::ScaleUp => {}
            ScalingSettings::ScaleDown => {}
            ScalingSettings::ScaleUpDown => {}
            ScalingSettings::StickyUp => {
                let max = self
                    .dataset_settings
                    .rounding_settings
                    .apply_up_rounding(point * self.dataset_settings.watermarking_multiplier)
                    / self.dataset_settings.watermarking_multiplier;

                if max > self.dataset_settings.high_watermark {
                    self.dataset_settings.high_watermark = max
                }
            }
            ScalingSettings::StickyDown => {
                let min = self
                    .dataset_settings
                    .rounding_settings
                    .apply_down_rounding(point * self.dataset_settings.watermarking_multiplier)
                    / self.dataset_settings.watermarking_multiplier;

                if min < self.dataset_settings.low_watermark {
                    self.dataset_settings.low_watermark = min
                }
            }
            ScalingSettings::StickyUpDown => {
                let max = self
                    .dataset_settings
                    .rounding_settings
                    .apply_up_rounding(point * self.dataset_settings.watermarking_multiplier)
                    / self.dataset_settings.watermarking_multiplier;

                if max > self.dataset_settings.high_watermark {
                    self.dataset_settings.high_watermark = max
                }

                let min = self
                    .dataset_settings
                    .rounding_settings
                    .apply_down_rounding(point * self.dataset_settings.watermarking_multiplier)
                    / self.dataset_settings.watermarking_multiplier;

                if min < self.dataset_settings.low_watermark {
                    self.dataset_settings.low_watermark = min
                }
            }
            ScalingSettings::StickyUpDownEqualMagnitude => {
                let max = self
                    .dataset_settings
                    .rounding_settings
                    .apply_up_rounding(point * self.dataset_settings.watermarking_multiplier)
                    / self.dataset_settings.watermarking_multiplier;

                if max > self.dataset_settings.high_watermark {
                    self.dataset_settings.low_watermark -=
                        max - self.dataset_settings.high_watermark;
                    self.dataset_settings.high_watermark = max;
                }

                let min = self
                    .dataset_settings
                    .rounding_settings
                    .apply_down_rounding(point * self.dataset_settings.watermarking_multiplier)
                    / self.dataset_settings.watermarking_multiplier;

                if min < self.dataset_settings.low_watermark {
                    self.dataset_settings.high_watermark +=
                        self.dataset_settings.low_watermark - min;
                    self.dataset_settings.low_watermark = min;
                }
            }
            ScalingSettings::Stacking => {}
            ScalingSettings::Fixed => {}
        }
    }

    pub fn update_data_points(&mut self, new_points: usize) {
        self.datas
            .iter_mut()
            .for_each(|set| set.update_data_points(new_points));
    }

    pub fn plot(&self, snapshot: &Snapshot, width: f32, height: f32, parent: &GraphWidget) {
        if !self.dataset_settings.visible {
            return;
        }

        let mut dataset_points: Vec<Vec<DatasetPoints>> = self
            .datas
            .iter()
            .map(|pts| pts.plot(width, height, &self.dataset_settings, parent))
            .collect();

        dataset_points = match self.dataset_settings.scaling_settings {
            ScalingSettings::Stacking => {
                // First, we pop the first dataset
                let mut stacked_points: Vec<Vec<DatasetPoints>> =
                    dataset_points.drain(0..1).collect();

                // for the remaining datasets, offset their y value off of the previous
                for (series_index, series) in dataset_points.iter().enumerate() {
                    let stacked_series = &stacked_points[series_index];
                    assert_eq!(series.len(), stacked_series.len());

                    let mut newset = Vec::new();

                    for (point_index, point) in series.iter().enumerate() {
                        newset.push(DatasetPoints {
                            x: point.x,
                            y: stacked_series[point_index].y + point.y - height,
                        })
                    }

                    stacked_points.push(newset);
                }

                stacked_points
            }
            _ => dataset_points,
        };

        let color = parent.base_color();

        let stroke_color = gdk::RGBA::new(color.red(), color.green(), color.blue(), 1.);
        let fill_color = gdk::RGBA::new(
            color.red(),
            color.green(),
            color.blue(),
            self.dataset_settings.opacity,
        );

        let stroke = Stroke::new(1.);

        if self.dataset_settings.dashed {
            stroke.set_dash(&[5., 5.]);
        }

        for (set_index, set) in dataset_points.iter().enumerate() {
            let set_split = set.split(|set| set.y.is_nan());

            for set in set_split {
                let path_builder = PathBuilder::new();

                let (Some(first_point), Some(last_point)) = (set.first(), set.last()) else {
                    continue;
                };

                path_builder.move_to(first_point.x, first_point.y);

                let (mut lastx, mut lasty) = (first_point.x, first_point.y);

                for point in set.iter().skip(1) {
                    if parent.smooth_graphs() {
                        let deltax = point.x - lastx;
                        path_builder.cubic_to(
                            lastx + deltax / 2f32,
                            lasty,
                            lastx + deltax / 2f32,
                            point.y,
                            point.x,
                            point.y,
                        );

                        lastx = point.x;
                        lasty = point.y;
                    } else {
                        path_builder.line_to(point.x, point.y);
                    }
                }

                if self.dataset_settings.vertical_dropoff_lines {
                    match self.dataset_settings.fill {
                        FillingSettings::FillToBottom | FillingSettings::None => {
                            path_builder.line_to(last_point.x, height);
                            path_builder.line_to(first_point.x, height);
                        }
                        FillingSettings::FillToTop => {
                            path_builder.line_to(last_point.x, 0.);
                            path_builder.line_to(first_point.x, 0.);
                        }
                        FillingSettings::FillToZero => {
                            if self.dataset_settings.low_watermark >= 0.
                                || self.dataset_settings.low_watermark
                                    >= self.dataset_settings.high_watermark
                            {
                                path_builder.line_to(last_point.x, height);
                                path_builder.line_to(first_point.x, height);
                            } else {
                                let zeroheight = height * (self.dataset_settings.high_watermark)
                                    / (self.dataset_settings.high_watermark
                                        - self.dataset_settings.low_watermark);
                                path_builder.line_to(last_point.x, zeroheight);
                                path_builder.line_to(first_point.x, zeroheight);
                            }
                        }
                    }

                    path_builder.close();

                    let path = path_builder.to_path();

                    if self.dataset_settings.fill != FillingSettings::None
                        && (self.dataset_settings.scaling_settings != ScalingSettings::Stacking
                            || set_index == dataset_points.len() - 1)
                    {
                        snapshot.append_fill(&path, FillRule::Winding, &fill_color);
                    }

                    snapshot.append_stroke(&path, &stroke, &stroke_color);
                } else {
                    let line = path_builder.to_path();

                    let path_builder = PathBuilder::new();

                    path_builder.move_to(first_point.x, first_point.y);

                    // builder.add_path(&line) doesn't work with the fill

                    let (mut lastx, mut lasty) = (first_point.x, first_point.y);

                    for point in set.iter().skip(1) {
                        if parent.smooth_graphs() {
                            let deltax = point.x - lastx;
                            path_builder.cubic_to(
                                lastx + deltax / 2f32,
                                lasty,
                                lastx + deltax / 2f32,
                                point.y,
                                point.x,
                                point.y,
                            );

                            lastx = point.x;
                            lasty = point.y;
                        } else {
                            path_builder.line_to(point.x, point.y);
                        }
                    }

                    match self.dataset_settings.fill {
                        FillingSettings::FillToBottom | FillingSettings::None => {
                            path_builder.line_to(last_point.x, height);
                            path_builder.line_to(first_point.x, height);
                        }
                        FillingSettings::FillToTop => {
                            path_builder.line_to(last_point.x, 0.);
                            path_builder.line_to(first_point.x, 0.);
                        }
                        FillingSettings::FillToZero => {
                            if self.dataset_settings.low_watermark >= 0.
                                || self.dataset_settings.low_watermark
                                    >= self.dataset_settings.high_watermark
                            {
                                path_builder.line_to(last_point.x, height);
                                path_builder.line_to(first_point.x, height);
                            } else {
                                let zeroheight = height * (self.dataset_settings.high_watermark)
                                    / (self.dataset_settings.high_watermark
                                        - self.dataset_settings.low_watermark);
                                path_builder.line_to(last_point.x, zeroheight);
                                path_builder.line_to(first_point.x, zeroheight);
                            }
                        }
                    }

                    path_builder.close();

                    let path = path_builder.to_path();

                    if self.dataset_settings.fill != FillingSettings::None
                        && (self.dataset_settings.scaling_settings != ScalingSettings::Stacking
                            || set_index == dataset_points.len() - 1)
                    {
                        snapshot.append_fill(&path, FillRule::Winding, &fill_color);
                    }

                    snapshot.append_stroke(&line, &stroke, &stroke_color);
                }
            }
        }
    }
}

impl Dataset {
    pub fn new_with_data(d: Vec<(f32, f32)>) -> Self {
        let (x_points, data) = d.into_iter().unzip();
        Dataset {
            data,
            x_points,
            used_data: 0,
        }
    }

    pub fn new_with_fill(v: f32) -> Self {
        let data = vec![v; MAX_POINTS as usize];
        Self {
            data,
            x_points: (0..MAX_POINTS as usize).map(|x| x as f32).collect(),
            used_data: MIN_POINTS as usize,
        }
    }

    pub fn update_data_points(&mut self, new_points: usize) {
        self.used_data = new_points;
    }

    pub fn get_data(&self) -> Vec<f32> {
        self.data
            .iter()
            .take(self.used_data)
            .map(|v| v.clone())
            .collect()
    }

    pub fn get_data_removed(&self) -> Vec<f32> {
        self.data
            .iter()
            .take(self.used_data)
            .filter(|v| v.is_normal())
            .map(|v| v.clone())
            .collect()
    }

    pub fn get_data_sanitized(&self, low_watermark: f32) -> Vec<f32> {
        self.data
            .iter()
            .take(self.used_data)
            .map(|v| {
                if !v.is_normal() && !(v.is_nan() || v == &0.0) {
                    low_watermark
                } else {
                    v.clone()
                }
            })
            .collect()
    }

    pub fn plot(
        &self,
        width: f32,
        height: f32,
        settings: &DatasetSettings,
        parent: &GraphWidget,
    ) -> Vec<DatasetPoints> {
        let val_min = settings.low_watermark;
        let val_max = settings.high_watermark.max(val_min + 1.);

        let spacing = width * parent.point_spacing_factor();

        let points: Vec<_> = self
            .x_points
            .iter()
            .zip(
                self.get_data_sanitized(val_min)
                    .iter()
                    .map(|y| (*y - val_min) / (val_max - val_min)),
            )
            .map(|(x, y)| (width - x * spacing, (1. - y) * height))
            .map(|(x, y)| DatasetPoints { x, y })
            .collect();

        points
    }
}

impl Default for Dataset {
    fn default() -> Self {
        let mut data = vec![f32::NAN; MAX_POINTS as usize];
        if let Some(v) = data.first_mut() {
            // so there's no vertical drop on first refresh
            *v = 0.0
        }
        Self {
            data,
            x_points: (0..MAX_POINTS as usize).map(|x| x as f32).collect(),
            used_data: MIN_POINTS as usize,
        }
    }
}

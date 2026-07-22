/* pages/format.rs
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

pub fn format_rate(value: f64) -> String {
    if value > 0.0 && value < 10.0 {
        return format!("{value:.1}");
    }
    let rounded = value.round() as i64;
    let digits = rounded.abs().to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if rounded < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `None` renders as an em dash. Rendering it as 0% would assert that every
/// block read missed cache, which is not what an absent ratio means.
pub fn format_ratio(ratio: Option<f64>) -> String {
    match ratio {
        Some(value) => format!("{:.1}%", value * 100.0),
        None => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_rates_with_thousands_separators() {
        assert_eq!(format_rate(1284.0), "1,284");
        assert_eq!(format_rate(0.0), "0");
        assert_eq!(format_rate(999.0), "999");
        assert_eq!(format_rate(1_234_567.0), "1,234,567");
    }

    #[test]
    fn formats_small_rates_with_one_decimal() {
        assert_eq!(format_rate(0.4), "0.4");
        assert_eq!(format_rate(9.6), "9.6");
    }

    #[test]
    fn formats_bytes_in_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1_048_576), "1.0 MiB");
        assert_eq!(format_bytes(3_221_225_472), "3.0 GiB");
    }

    #[test]
    fn formats_a_ratio_as_a_percentage() {
        assert_eq!(format_ratio(Some(0.9987)), "99.9%");
        assert_eq!(format_ratio(Some(0.0)), "0.0%");
    }

    #[test]
    fn an_absent_ratio_renders_as_a_dash_not_zero() {
        // No blocks were accessed this interval. Showing 0% would claim every
        // read missed cache, which is not what happened.
        assert_eq!(format_ratio(None), "—");
    }
}

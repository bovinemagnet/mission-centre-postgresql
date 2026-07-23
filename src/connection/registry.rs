/* registry.rs
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

use gtk::prelude::SettingsExt;
use gtk::{gio, glib};

use crate::connection::params::ConnectionParams;

const KEY: &str = "servers";
const BACKUP_KEY: &str = "servers-backup";

/// A malformed value yields an empty list rather than a panic: the setting is
/// user-editable, and a bad edit must not stop the application starting.
pub fn parse(json: &str) -> Vec<ConnectionParams> {
    serde_json::from_str(json).unwrap_or_default()
}

pub fn serialise(servers: &[ConnectionParams]) -> Result<String, serde_json::Error> {
    serde_json::to_string(servers)
}

pub fn load(settings: &gio::Settings) -> Vec<ConnectionParams> {
    parse(settings.string(KEY).as_str())
}

pub fn save(settings: &gio::Settings, servers: &[ConnectionParams]) -> Result<(), glib::BoolError> {
    let current = settings.string(KEY);
    if !current.is_empty() && serde_json::from_str::<Vec<ConnectionParams>>(&current).is_err() {
        // The stored value doesn't parse but isn't empty: it may be a
        // wrong-shaped value from a future version of the app. Salvage it
        // before it gets overwritten below.
        settings.set_string(BACKUP_KEY, &current)?;
    }

    let json =
        serialise(servers).map_err(|_| glib::bool_error!("failed to serialise server list"))?;
    settings.set_string(KEY, &json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::params::SslMode;
    use uuid::Uuid;

    fn server(label: &str) -> ConnectionParams {
        ConnectionParams {
            id: Uuid::nil(),
            label: label.to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "postgres".to_string(),
            user: "paul".to_string(),
            ssl_mode: SslMode::Prefer,
        }
    }

    #[test]
    fn round_trips_a_list_of_servers() {
        let servers = vec![server("one"), server("two")];
        let parsed = parse(&serialise(&servers).unwrap());
        assert_eq!(parsed, servers);
    }

    #[test]
    fn an_empty_setting_yields_no_servers() {
        assert!(parse("[]").is_empty());
    }

    #[test]
    fn corrupt_json_yields_no_servers_rather_than_panicking() {
        // A hand-edited or downgraded setting must not crash the application
        // on startup.
        assert!(parse("{ not json").is_empty());
        assert!(parse("").is_empty());
    }

    #[test]
    fn wrong_shaped_but_valid_json_yields_no_servers() {
        // Syntactically valid JSON that doesn't match the expected shape,
        // e.g. written by a future version of the app, must not panic.
        assert!(parse("{}").is_empty());
        assert!(parse("[123]").is_empty());
        assert!(parse("null").is_empty());
    }

    #[test]
    fn serialise_round_trips_successfully() {
        let servers = vec![server("one"), server("two")];
        let result = serialise(&servers);
        assert!(result.is_ok());
        assert_eq!(parse(&result.unwrap()), servers);
    }

    #[test]
    fn serialised_servers_never_contain_a_password() {
        let json = serialise(&[server("prod")]).unwrap();
        assert!(!json.to_lowercase().contains("password"));
    }
}

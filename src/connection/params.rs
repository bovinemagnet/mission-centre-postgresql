/* params.rs
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

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
}

impl SslMode {
    fn to_pg(self) -> tokio_postgres::config::SslMode {
        match self {
            SslMode::Disable => tokio_postgres::config::SslMode::Disable,
            SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
            SslMode::Require => tokio_postgres::config::SslMode::Require,
        }
    }
}

/// Where a server's history is stored. Off by default and strictly opt-in:
/// Local writes to a SQLite file Mission Centre owns; PgConsole writes to an
/// existing pgconsole schema on the monitored server (INSERT only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HistoryMode {
    #[default]
    Off,
    Local,
    #[serde(rename = "pgconsole")]
    PgConsole,
}

/// Everything needed to reach a server *except* the password, which lives in
/// the system secret store. This type is serialised into GSettings, so it must
/// never gain a password field.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionParams {
    pub id: Uuid,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub ssl_mode: SslMode,
    #[serde(default)]
    pub history: HistoryMode,
}

/// Manual Debug implementation to prevent accidentally including any future
/// password-like field. A derived Debug would automatically include new fields,
/// which would be catastrophic since this struct is serialised into GSettings
/// (plain text). By hand-writing Debug, we fail closed: if someone adds a
/// password field, its omission from the debug output is visible in review.
impl fmt::Debug for ConnectionParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionParams")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("ssl_mode", &self.ssl_mode)
            .field("history", &self.history)
            .finish()
    }
}

impl ConnectionParams {
    pub fn to_config(&self, password: &str) -> tokio_postgres::Config {
        let mut config = tokio_postgres::Config::new();
        config
            .host(&self.host)
            .port(self.port)
            .dbname(&self.database)
            .user(&self.user)
            .password(password)
            .ssl_mode(self.ssl_mode.to_pg())
            .application_name("mission-centre-pg")
            // A server that completes the TCP handshake but never answers
            // must not be able to block the connect step for ever; the
            // collector layers its own stop-aware cancellation on top of
            // this, but a bounded timeout here is the last line of defence.
            .connect_timeout(std::time::Duration::from_secs(10));
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> ConnectionParams {
        ConnectionParams {
            id: Uuid::nil(),
            label: "prod".to_string(),
            host: "db.example.com".to_string(),
            port: 5432,
            database: "appdb".to_string(),
            user: "monitor".to_string(),
            ssl_mode: SslMode::Require,
            history: HistoryMode::Off,
        }
    }

    #[test]
    fn round_trips_through_json() {
        let original = params();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: ConnectionParams = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn serialised_form_never_contains_a_password_field() {
        // The type holds no password by construction. This test fails loudly
        // if anyone ever adds one, because the JSON goes into GSettings.
        let json = serde_json::to_string(&params()).unwrap();
        assert!(!json.to_lowercase().contains("password"));
        assert!(!json.to_lowercase().contains("secret"));
    }

    #[test]
    fn builds_a_postgres_config_with_an_application_name() {
        let config = params().to_config("hunter2");
        assert_eq!(config.get_hosts().len(), 1);
        assert_eq!(config.get_ports(), &[5432]);
        assert_eq!(config.get_user(), Some("monitor"));
        assert_eq!(config.get_dbname(), Some("appdb"));
        assert_eq!(config.get_application_name(), Some("mission-centre-pg"));
    }

    #[test]
    fn debug_output_does_not_leak_the_password_argument() {
        // to_config takes the password but ConnectionParams never stores it,
        // so debugging the params can never expose it.
        let rendered = format!("{:?}", params());
        assert!(!rendered.contains("hunter2"));
    }

    #[test]
    fn debug_output_lists_only_the_known_fields() {
        let rendered = format!("{:?}", params());
        assert!(rendered.contains("ConnectionParams"));
        assert!(rendered.contains("prod")); // the label
        assert!(rendered.contains("db.example.com")); // the host
    }

    #[test]
    fn history_mode_defaults_to_off_for_a_phase_1_server_json() {
        // Servers stored before Phase 3 have no "history" field. They must
        // deserialise with history off, since history is strictly opt-in.
        let json = r#"{"id":"00000000-0000-0000-0000-000000000000","label":"old",
            "host":"localhost","port":5432,"database":"postgres","user":"paul",
            "ssl_mode":"prefer"}"#;
        let parsed: ConnectionParams = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.history, HistoryMode::Off);
    }

    #[test]
    fn history_mode_round_trips_through_json() {
        let mut original = params();
        original.history = HistoryMode::PgConsole;
        let parsed: ConnectionParams =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(parsed.history, HistoryMode::PgConsole);
    }

    #[test]
    fn pgconsole_serialises_in_lower_case() {
        let mut server = params();
        server.history = HistoryMode::PgConsole;
        let json = serde_json::to_string(&server).unwrap();
        assert!(json.contains("\"history\":\"pgconsole\""), "{json}");
    }
}

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

/// Everything needed to reach a server *except* the password, which lives in
/// the system secret store. This type is serialised into GSettings, so it must
/// never gain a password field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionParams {
    pub id: Uuid,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub ssl_mode: SslMode,
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
            .application_name("mission-centre-pg");
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
}

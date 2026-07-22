/* collect.rs
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

use std::time::Duration;

use mission_centre_pg::collector::worker::{spawn, CollectorEvent};
use mission_centre_pg::connection::params::{ConnectionParams, SslMode};

fn main() {
    let params = ConnectionParams {
        id: uuid::Uuid::nil(),
        label: "local".to_string(),
        host: "/run/postgresql".to_string(),
        port: 5432,
        database: std::env::var("PGDATABASE").unwrap_or_else(|_| "postgres".to_string()),
        user: std::env::var("USER").unwrap_or_else(|_| "postgres".to_string()),
        ssl_mode: SslMode::Disable,
    };

    let handle = spawn(params, String::new(), Duration::from_secs(1));
    for _ in 0..4 {
        match handle.events.recv_blocking() {
            Ok(CollectorEvent::Connected(info)) => println!("connected: {info:?}"),
            Ok(CollectorEvent::Sample(s)) => {
                println!("sessions={} rates={:?}", s.sessions.len(), s.rates)
            }
            Ok(other) => println!("{other:?}"),
            Err(_) => break,
        }
    }
    handle.stop();
}

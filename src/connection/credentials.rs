/* credentials.rs
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

use keyring::Entry;
use uuid::Uuid;

const SERVICE: &str = "mission-centre-pg";

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("secret store unavailable: {0}")]
    Store(#[from] keyring::Error),
}

fn entry(id: &Uuid) -> Result<Entry, CredentialError> {
    Ok(Entry::new(SERVICE, &id.to_string())?)
}

pub fn store_password(id: &Uuid, password: &str) -> Result<(), CredentialError> {
    entry(id)?.set_password(password)?;
    Ok(())
}

/// `Ok(None)` means no password has been stored for this server, which is a
/// normal state — not an error.
pub fn fetch_password(id: &Uuid) -> Result<Option<String>, CredentialError> {
    match entry(id)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(CredentialError::Store(e)),
    }
}

pub fn delete_password(id: &Uuid) -> Result<(), CredentialError> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(CredentialError::Store(e)),
    }
}

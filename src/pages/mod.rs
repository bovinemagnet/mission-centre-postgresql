/* pages/mod.rs
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

pub mod format;
pub mod locks;
pub mod overview;
pub mod plan;
pub mod queries;
pub mod relations;
pub mod replication;
pub mod sessions;

pub use locks::McpgLocksPage;
pub use overview::McpgOverviewPage;
pub use plan::McpgPlanPage;
pub use queries::McpgQueriesPage;
pub use relations::McpgRelationsPage;
pub use replication::McpgReplicationPage;
pub use sessions::McpgSessionsPage;

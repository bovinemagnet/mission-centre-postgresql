/* collector/action_runner.rs
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

use crate::actions::sql::plan_for;
use crate::actions::{signal_outcome, Action, ActionOutcome};
use tokio_postgres::SimpleQueryMessage;

use crate::collector::worker::{connect, map_query_error, CollectorError};
use crate::connection::params::ConnectionParams;

/// Runs one action on a connection of its own.
///
/// The sampler's client is deliberately not reused: a VACUUM would hold it for
/// minutes, flatlining the graphs and reaching the consecutive-failure
/// threshold that declares the connection lost. A connection per action costs
/// one connect round trip on something the user triggered by hand, which is
/// invisible beside the action itself.
///
/// `connect` also runs `PROBE_SQL`, which this path does not need. Reusing it
/// anyway keeps the TLS-mode and rustls handling in one place; a probe-free
/// variant would duplicate all of it to save a round trip nobody can perceive.
pub async fn run_action(
    params: &ConnectionParams,
    password: &str,
    action: &Action,
) -> ActionOutcome {
    let client = match connect(params, password).await {
        Ok((client, _info)) => client,
        Err(e) => return ActionOutcome::Failed(e.to_string()),
    };

    let plan = plan_for(action);

    if let Err(e) = client.batch_execute(&plan.setup).await {
        return ActionOutcome::Failed(map_query_error(e).to_string());
    }

    if plan.batch {
        // VACUUM cannot run inside a transaction block, and the extended
        // protocol wraps its statement in an implicit one.
        match client.batch_execute(&plan.sql).await {
            Ok(()) => ActionOutcome::Succeeded,
            Err(e) => ActionOutcome::Failed(map_query_error(e).to_string()),
        }
    } else if let Some(pid) = plan.pid {
        match client.query_one(plan.sql.as_str(), &[&pid]).await {
            Ok(row) => signal_outcome(row.get(0)),
            Err(e) => ActionOutcome::Failed(map_query_error(e).to_string()),
        }
    } else {
        // `execute` runs the statement without decoding its result, which
        // matters for pg_stat_statements_reset(): it returns void before
        // extension version 1.11 and timestamptz from 1.11 on.
        match client.execute(plan.sql.as_str(), &[]).await {
            Ok(_) => ActionOutcome::Succeeded,
            Err(e) => ActionOutcome::Failed(map_query_error(e).to_string()),
        }
    }
}

/// Runs one `EXPLAIN` on a connection of its own and returns the plan as JSON.
///
/// Its own connection for the same reason an action has one: an EXPLAIN
/// against a large catalogue can outlast a sampling interval, and the sampler
/// must not be queued behind it. A failure here is returned to the caller
/// rather than counted against the connection's failure budget — a statement
/// that cannot be planned says nothing about the health of the connection.
pub async fn run_explain(
    params: &ConnectionParams,
    password: &str,
    sql: &str,
) -> Result<String, CollectorError> {
    let (client, _info) = connect(params, password).await?;

    client
        .batch_execute("SET statement_timeout = '5s'")
        .await
        .map_err(map_query_error)?;

    // The simple query protocol, deliberately. Under the extended protocol the
    // driver prepares the statement, reads the `$1` placeholders inside the
    // statement being explained as parameters of its own, and refuses the call
    // for passing none. psql succeeds for the same reason: it sends this
    // simply. Values arrive as text, which is what the parser wants anyway.
    let messages = client.simple_query(sql).await.map_err(map_query_error)?;

    messages
        .into_iter()
        .find_map(|message| match message {
            SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
            _ => None,
        })
        .ok_or_else(|| CollectorError::Query("the server returned no plan".to_string()))
}

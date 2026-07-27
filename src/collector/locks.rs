/* collector/locks.rs
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

use std::collections::{HashMap, HashSet};

/// One backend involved in a lock conflict — either waiting, or blocking
/// somebody who is.
#[derive(Debug, Clone, PartialEq)]
pub struct LockParticipant {
    pub pid: i32,
    /// From `pg_blocking_pids`. Empty for a backend that blocks others
    /// without waiting itself, which is the usual shape of a chain's root.
    pub blocked_by: Vec<i32>,
    pub waiting: bool,
    pub wait_secs: Option<f64>,
    pub lock_mode: Option<String>,
    pub relation: Option<String>,
    pub user_name: Option<String>,
    pub database: Option<String>,
    pub state: Option<String>,
    /// `None` when the connected role lacks `pg_monitor` and the backend
    /// belongs to another user, exactly as `Session::query` is.
    pub query: Option<String>,
}

/// A node in a blocked chain. `children` are the backends this one blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct LockNode {
    pub participant: LockParticipant,
    pub children: Vec<LockNode>,
    /// Set when this node's chain closes on itself. The server resolves real
    /// deadlocks; a sample can still catch one mid-flight.
    pub in_cycle: bool,
    /// Set when the blocker was named by `pg_blocking_pids` but had gone by
    /// the time the rest of the row was read.
    pub is_stub: bool,
}

/// A blocker that vanished between the two halves of the query. Only the pid
/// is known, and the page must say so rather than invent fields.
pub fn stub_participant(pid: i32) -> LockParticipant {
    LockParticipant {
        pid,
        blocked_by: Vec::new(),
        waiting: false,
        wait_secs: None,
        lock_mode: None,
        relation: None,
        user_name: None,
        database: None,
        state: None,
        query: None,
    }
}

pub fn build_forest(rows: &[LockParticipant]) -> Vec<LockNode> {
    let by_pid: HashMap<i32, &LockParticipant> = rows.iter().map(|row| (row.pid, row)).collect();

    // Roots are the backends nobody in this sample blocks: either they are not
    // waiting at all, or every pid they wait on has since gone. A vanished
    // blocker becomes a stub so its waiters keep their context.
    let mut roots: Vec<LockParticipant> = Vec::new();
    let mut seen_stub: HashSet<i32> = HashSet::new();

    for row in rows {
        if row.blocked_by.is_empty() {
            roots.push(row.clone());
            continue;
        }
        for blocker in &row.blocked_by {
            if !by_pid.contains_key(blocker) && seen_stub.insert(*blocker) {
                roots.push(stub_participant(*blocker));
            }
        }
    }

    // A cycle has no root at all: every member waits on another member. Break
    // it by promoting its lowest pid, so the chain is shown rather than lost.
    if roots.is_empty() && !rows.is_empty() {
        if let Some(lowest) = rows.iter().min_by_key(|row| row.pid) {
            roots.push(lowest.clone());
        }
    }

    roots
        .into_iter()
        .map(|participant| {
            let is_stub = !by_pid.contains_key(&participant.pid);
            let pid = participant.pid;
            let mut path = HashSet::new();
            path.insert(pid);
            LockNode {
                participant,
                children: children_of(pid, rows, &mut path),
                in_cycle: false,
                is_stub,
            }
        })
        .collect()
}

/// Everything blocked directly by `pid`, recursively. `path` carries the pids
/// already on this branch so a cycle stops instead of recursing forever.
fn children_of(pid: i32, rows: &[LockParticipant], path: &mut HashSet<i32>) -> Vec<LockNode> {
    rows.iter()
        .filter(|row| row.blocked_by.contains(&pid))
        .map(|row| {
            if !path.insert(row.pid) {
                return LockNode {
                    participant: row.clone(),
                    children: Vec::new(),
                    in_cycle: true,
                    is_stub: false,
                };
            }
            let children = children_of(row.pid, rows, path);
            path.remove(&row.pid);
            LockNode {
                participant: row.clone(),
                children,
                in_cycle: false,
                is_stub: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiter(pid: i32, blocked_by: &[i32]) -> LockParticipant {
        LockParticipant {
            pid,
            blocked_by: blocked_by.to_vec(),
            waiting: true,
            wait_secs: Some(1.0),
            lock_mode: Some("RowExclusiveLock".to_string()),
            relation: Some("public.app_orders".to_string()),
            user_name: Some("app".to_string()),
            database: Some("postgres".to_string()),
            state: Some("active".to_string()),
            query: Some("UPDATE app_orders SET note = 'x'".to_string()),
        }
    }

    fn root(pid: i32) -> LockParticipant {
        LockParticipant {
            waiting: false,
            wait_secs: None,
            lock_mode: None,
            state: Some("idle in transaction".to_string()),
            ..waiter(pid, &[])
        }
    }

    #[test]
    fn nothing_blocked_is_an_empty_forest() {
        assert!(build_forest(&[]).is_empty());
    }

    #[test]
    fn a_single_chain_nests_the_waiter_under_its_blocker() {
        let forest = build_forest(&[root(100), waiter(200, &[100])]);

        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].participant.pid, 100);
        assert_eq!(forest[0].children.len(), 1);
        assert_eq!(forest[0].children[0].participant.pid, 200);
    }

    #[test]
    fn one_blocker_with_several_waiters_is_one_tree_not_several() {
        let forest = build_forest(&[root(100), waiter(200, &[100]), waiter(300, &[100])]);

        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].children.len(), 2);
    }

    #[test]
    fn a_three_deep_chain_nests_all_the_way_down() {
        let forest = build_forest(&[root(100), waiter(200, &[100]), waiter(300, &[200])]);

        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].children[0].participant.pid, 200);
        assert_eq!(forest[0].children[0].children[0].participant.pid, 300);
    }

    #[test]
    fn a_blocker_that_has_gone_becomes_a_stub_rather_than_dropping_the_waiter() {
        let forest = build_forest(&[waiter(200, &[999])]);

        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].participant.pid, 999);
        assert!(forest[0].is_stub);
        assert_eq!(forest[0].children[0].participant.pid, 200);
    }

    /// A cycle is flagged wherever the chain closes, which is not necessarily
    /// near the root, so the whole tree is searched.
    fn any_in_cycle(nodes: &[LockNode]) -> bool {
        nodes
            .iter()
            .any(|node| node.in_cycle || any_in_cycle(&node.children))
    }

    #[test]
    fn a_cycle_terminates_and_is_flagged() {
        let forest = build_forest(&[waiter(100, &[200]), waiter(200, &[100])]);

        assert_eq!(forest.len(), 1);
        assert!(any_in_cycle(&forest), "the closing edge must be flagged");
    }

    #[test]
    fn a_backend_blocked_by_two_others_appears_under_both() {
        let forest = build_forest(&[root(100), root(200), waiter(300, &[100, 200])]);

        assert_eq!(forest.len(), 2);
        assert_eq!(forest[0].children[0].participant.pid, 300);
        assert_eq!(forest[1].children[0].participant.pid, 300);
    }
}

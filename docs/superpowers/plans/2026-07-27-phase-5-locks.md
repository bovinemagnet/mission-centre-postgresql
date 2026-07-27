# Phase 5 — Locks Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Locks page — a live blocked-session tree, a full lock inventory that samples only while it is on screen, and cancel/terminate on the selected backend.

**Architecture:** One SQL query returns a flat row per backend involved in contention — waiters *and* the backends blocking them. A pure Rust function turns that flat list into a forest of blocked chains, which keeps the awkward cases (cycles, missing blockers) unit-testable without a database. The tree samples on the fast tier; the inventory is gated on view visibility by a single atomic flag. Actions reuse Phase 4 wholesale.

**Tech Stack:** Rust, GTK4 + libadwaita via gtk-rs, Blueprint (`.blp`) for layout, `tokio-postgres`, Meson + Ninja, `testcontainers` for portability tests.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-27-phase-5-replication-and-locks-design.md`. This plan implements §4, §6 and the lock-related parts of §7–§10. Replication (§5) is a separate plan.
- **Author:** Paul Snow. **Version:** 0.0.0. Every new file carries the GPL-3.0-or-later header block copied from `src/pages/sessions.rs:1-19`.
- **British spelling** in all comments, documentation and user-visible strings.
- **PostgreSQL 14 is the version floor.** Every query must run on 14 and 18.
- **Prefer files under ~800 lines**, but this is a guideline rather than a hard limit: Paul confirmed on 2026-07-27 that a file may exceed it where splitting would mean restructuring unrelated code. `src/collector/worker.rs` sits at 813 lines after this phase. If `src/collector/locks.rs` grows unwieldy, the tree builder splitting into `src/collector/locks_tree.rs` is the intended seam.
- **User-visible strings** go through `crate::i18n::i18n`.
- **TDD throughout:** the failing test comes first, and is run and seen to fail before any implementation.
- **Commands:** `cargo test --lib`, `cargo test --bin mission-centre-pg`, `ninja -C build`. Portability tests need `export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"` and run with `cargo test --test portability`.
- **Three failure states are distinct** (spec §8): unsupported names the version, not-permitted names the privilege, failed carries the PostgreSQL message verbatim.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/collector/locks.rs` | Create — lock SQL, flat row types, row mapping, the pure tree builder, unit tests |
| `src/pages/locks.rs` | Create — two-view page, selection, action bar wiring |
| `resources/ui/locks_page.blp` | Create — layout for both views |
| `src/collector/snapshot.rs` | Modify — two new `Snapshot` fields |
| `src/collector/worker.rs` | Modify — fast-tier tree sample, gated inventory sample |
| `src/collector/mod.rs` | Modify — declare the module |
| `src/pages/mod.rs` | Modify — declare the page |
| `src/window.rs` | Modify — construct, update and gate the page |
| `resources/ui/window.blp` | Modify — the `Adw.ViewStackPage` entry |
| `resources/meson.build` | Modify — the new `.blp` in the blueprints list |
| `resources/mission-centre-pg.gresource.xml` | Modify — the compiled `.ui` |
| `data/io.github.paulsnow.MissionCentrePg.gschema.xml` | Modify — the `locks-limit` key |
| `tests/portability.rs` | Modify — both queries on 14 and 18, superuser and plain role |

---

## Task 1: The lock row types and the tree builder

The pure heart of the page. No database, no GTK — just data in and a forest out.

**Files:**
- Create: `src/collector/locks.rs`
- Modify: `src/collector/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `LockParticipant { pid: i32, blocked_by: Vec<i32>, waiting: bool, wait_secs: Option<f64>, lock_mode: Option<String>, relation: Option<String>, user_name: Option<String>, database: Option<String>, state: Option<String>, query: Option<String> }`; `LockNode { participant: LockParticipant, children: Vec<LockNode>, in_cycle: bool, is_stub: bool }`; `pub fn build_forest(rows: &[LockParticipant]) -> Vec<LockNode>`; `pub fn stub_participant(pid: i32) -> LockParticipant`.

- [ ] **Step 1: Create the module with the types and a stubbed builder**

Create `src/collector/locks.rs` with the GPL header from `src/pages/sessions.rs:1-19` (change the path comment to `collector/locks.rs`), then:

```rust
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

/// A blocker that vanished between the two halves of the query. Only the
/// pid is known, and the page must say so rather than invent fields.
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

pub fn build_forest(_rows: &[LockParticipant]) -> Vec<LockNode> {
    Vec::new()
}
```

Declare it in `src/collector/mod.rs` alongside the existing modules:

```rust
pub mod locks;
```

- [ ] **Step 2: Write the failing tests**

Append to `src/collector/locks.rs`:

```rust
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

    #[test]
    fn a_cycle_terminates_and_is_flagged() {
        let forest = build_forest(&[waiter(100, &[200]), waiter(200, &[100])]);

        assert_eq!(forest.len(), 1);
        assert!(forest.iter().any(|node| node.in_cycle
            || node.children.iter().any(|child| child.in_cycle)));
    }

    #[test]
    fn a_backend_blocked_by_two_others_appears_under_both() {
        let forest = build_forest(&[root(100), root(200), waiter(300, &[100, 200])]);

        assert_eq!(forest.len(), 2);
        assert_eq!(forest[0].children[0].participant.pid, 300);
        assert_eq!(forest[1].children[0].participant.pid, 300);
    }
}
```

- [ ] **Step 3: Run the tests and watch them fail**

```bash
cargo test --lib collector::locks
```

Expected: `nothing_blocked_is_an_empty_forest` passes (the stub returns an empty vector); every other test fails on an empty forest.

- [ ] **Step 4: Implement the builder**

Replace the stubbed `build_forest`:

```rust
use std::collections::{HashMap, HashSet};

pub fn build_forest(rows: &[LockParticipant]) -> Vec<LockNode> {
    let by_pid: HashMap<i32, &LockParticipant> =
        rows.iter().map(|row| (row.pid, row)).collect();

    // Roots are the backends nobody in this sample blocks: either they are
    // not waiting at all, or every pid they wait on has since gone. A
    // vanished blocker becomes a stub so its waiters keep their context.
    let mut roots: Vec<LockParticipant> = Vec::new();
    let mut seen_stub: HashSet<i32> = HashSet::new();

    for row in rows {
        if row.blocked_by.is_empty() {
            roots.push((*row).clone());
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
            roots.push((*lowest).clone());
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
```

- [ ] **Step 5: Run the tests and watch them pass**

```bash
cargo test --lib collector::locks
```

Expected: all seven pass.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt
cargo test --lib
git add src/collector/locks.rs src/collector/mod.rs
git commit -m "feat: blocked-lock tree model and builder"
```

---

## Task 2: The blocked-tree query

**Files:**
- Modify: `src/collector/locks.rs`
- Modify: `tests/portability.rs`

**Interfaces:**
- Consumes: `LockParticipant` from Task 1.
- Produces: `pub const BLOCKED_SQL: &str`; `pub fn map_participant(row: &tokio_postgres::Row) -> LockParticipant`; `pub struct LocksSample { pub participants: Vec<LockParticipant> }`.

- [ ] **Step 1: Write the SQL and the mapper**

Add to `src/collector/locks.rs`:

```rust
use tokio_postgres::Row;

#[derive(Debug, Clone)]
pub struct LocksSample {
    pub participants: Vec<LockParticipant>,
}

/// Every backend in a lock conflict, waiters and blockers alike.
///
/// `pg_blocking_pids` inspects lock manager state on every call, so it is
/// evaluated only for backends actually waiting on a lock — on a healthy
/// server that is no rows at all. The blockers are then unioned back in:
/// a chain's root is typically `idle in transaction` and so waits on
/// nobody, which would leave the top of the tree with no user, database or
/// query — exactly the row the operator needs in order to decide whether
/// terminating it is safe.
pub const BLOCKED_SQL: &str = "\
WITH waiters AS (
    SELECT pid, pg_blocking_pids(pid) AS blocked_by
    FROM pg_stat_activity
    WHERE wait_event_type = 'Lock'
),
participants AS (
    SELECT pid, blocked_by FROM waiters
    UNION
    SELECT unnest(blocked_by), ARRAY[]::int[] FROM waiters
)
SELECT p.pid,
       (SELECT coalesce(w.blocked_by, ARRAY[]::int[])
          FROM waiters w WHERE w.pid = p.pid)         AS blocked_by,
       EXISTS (SELECT 1 FROM waiters w WHERE w.pid = p.pid) AS waiting,
       EXTRACT(EPOCH FROM (now() - a.query_start))::float8  AS wait_secs,
       lk.mode                                        AS lock_mode,
       lk.relation                                    AS relation,
       a.usename::text                                AS user_name,
       a.datname::text                                AS database,
       a.state                                        AS state,
       a.query                                        AS query
FROM participants p
JOIN pg_stat_activity a ON a.pid = p.pid
LEFT JOIN LATERAL (
    SELECT l.mode, l.relation::regclass::text AS relation
    FROM pg_locks l
    WHERE l.pid = p.pid AND NOT l.granted
    LIMIT 1
) lk ON true";

pub fn map_participant(row: &Row) -> LockParticipant {
    LockParticipant {
        pid: row.get("pid"),
        blocked_by: row.get("blocked_by"),
        waiting: row.get("waiting"),
        wait_secs: row.get("wait_secs"),
        lock_mode: row.get("lock_mode"),
        relation: row.get("relation"),
        user_name: row.get("user_name"),
        database: row.get("database"),
        state: row.get("state"),
        query: row.get("query"),
    }
}
```

- [ ] **Step 2: Write the failing portability test**

Add to `tests/portability.rs`, following the shape of the existing `relations_sql_runs_on_postgres_14`:

```rust
#[tokio::test]
async fn blocked_sql_runs_on_postgres_14() {
    let (client, _container) = connect(14).await;
    let rows = client
        .query(mission_centre_pg::collector::locks::BLOCKED_SQL, &[])
        .await
        .expect("the blocked-lock query must run on PostgreSQL 14");
    assert!(rows.is_empty(), "an idle server has no lock contention");
}

#[tokio::test]
async fn blocked_sql_runs_on_postgres_18() {
    let (client, _container) = connect(18).await;
    let rows = client
        .query(mission_centre_pg::collector::locks::BLOCKED_SQL, &[])
        .await
        .expect("the blocked-lock query must run on PostgreSQL 18");
    assert!(rows.is_empty(), "an idle server has no lock contention");
}

#[tokio::test]
async fn blocked_sql_finds_a_real_conflict_on_postgres_18() {
    let (client, container) = connect(18).await;
    client
        .batch_execute("CREATE TABLE conflict (id int PRIMARY KEY, note text); INSERT INTO conflict VALUES (1, 'a')")
        .await
        .expect("setup");

    // A second connection holds a row lock inside an open transaction; a
    // third waits on it. Both must outlive the assertion, so neither is
    // dropped until the end of the test.
    let holder = connect_to(&container).await;
    holder
        .batch_execute("BEGIN; UPDATE conflict SET note = 'held' WHERE id = 1")
        .await
        .expect("holder takes the row lock");

    let waiter = connect_to(&container).await;
    let waiting = tokio::spawn(async move {
        waiter
            .batch_execute("UPDATE conflict SET note = 'waiting' WHERE id = 1")
            .await
    });

    // Give the waiter time to reach the lock manager and register as waiting.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let rows = client
        .query(mission_centre_pg::collector::locks::BLOCKED_SQL, &[])
        .await
        .expect("query runs");

    let participants: Vec<_> = rows
        .iter()
        .map(mission_centre_pg::collector::locks::map_participant)
        .collect();
    let forest = mission_centre_pg::collector::locks::build_forest(&participants);

    assert_eq!(forest.len(), 1, "one chain: {participants:?}");
    assert_eq!(forest[0].children.len(), 1, "with one waiter under it");
    assert!(
        forest[0].participant.state.as_deref() == Some("idle in transaction"),
        "the root is the transaction holding the lock"
    );

    holder.batch_execute("ROLLBACK").await.expect("release");
    let _ = waiting.await;
}
```

If `connect_to(&container)` does not already exist in `tests/portability.rs`, add it next to `connect`, returning a second `tokio_postgres::Client` against the same running container.

- [ ] **Step 3: Run the portability tests and watch them fail**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability blocked_sql
```

Expected: compilation fails until `map_participant` and `BLOCKED_SQL` are public and the module is exported from `src/lib.rs`.

- [ ] **Step 4: Make them pass**

Ensure `src/lib.rs` re-exports the collector module so the integration test can reach it, matching how `relations` is already reached. Re-run:

```bash
cargo test --test portability blocked_sql
```

Expected: three passes. The conflict test is the one that matters — it proves the union of waiters and blockers produces a root with the holder's state on it.

- [ ] **Step 5: Add the plain-role visibility test**

```rust
#[tokio::test]
async fn blocked_sql_runs_for_a_role_without_pg_monitor_on_postgres_18() {
    let (client, container) = connect(18).await;
    client
        .batch_execute("CREATE ROLE plain LOGIN PASSWORD 'plain'")
        .await
        .expect("setup");

    let plain = connect_as(&container, "plain", "plain").await;
    let rows = plain
        .query(mission_centre_pg::collector::locks::BLOCKED_SQL, &[])
        .await
        .expect("a plain role may still run the query");
    assert!(rows.is_empty());
}
```

This is what settles spec §3.4 by observation rather than assertion: the query must not error for an unprivileged role, whatever it does or does not mask.

- [ ] **Step 6: Run everything and commit**

```bash
cargo test --test portability
cargo fmt
git add src/collector/locks.rs tests/portability.rs src/lib.rs
git commit -m "feat: blocked-lock query, verified against PostgreSQL 14 and 18"
```

---

## Task 3: Carry locks in the snapshot

**Files:**
- Modify: `src/collector/snapshot.rs`
- Modify: `src/collector/worker.rs`
- Modify: `src/collector/worker_tests.rs`

**Interfaces:**
- Consumes: `LocksSample`, `BLOCKED_SQL`, `map_participant` from Task 2.
- Produces: `Snapshot.locks: Option<Result<LocksSample, CollectorError>>`, populated on every tick.

- [ ] **Step 1: Add the field**

In `src/collector/snapshot.rs`, inside `pub struct Snapshot`, after `relations`:

```rust
    /// Fast tier, so `Some` on every tick. `Err` carries the reason the page
    /// renders in place of its tree.
    pub locks: Option<Result<LocksSample, CollectorError>>,
```

Import `LocksSample` at the top of the file alongside the other sample types.

- [ ] **Step 2: Run the build and watch it fail**

```bash
cargo build
```

Expected: every construction site of `Snapshot` fails with "missing field `locks`". That list is the exact set of places Step 3 must touch.

- [ ] **Step 3: Sample it on the fast tier**

In `src/collector/worker.rs`, inside `async fn sample`, after the sessions are mapped:

```rust
    let locks = classify_slow(
        client
            .query(BLOCKED_SQL, &[])
            .await
            .map_err(map_query_error)
            .map(|rows| LocksSample {
                participants: rows.iter().map(map_participant).collect(),
            }),
    )?;
```

Add `locks: Some(locks)` to the `Snapshot` it returns, and import the three names from `crate::collector::locks`.

`classify_slow` is reused deliberately: a permission or query error becomes a per-page message, while a timeout or lost connection still fails the whole tick, exactly as it does for statements and relations.

- [ ] **Step 4: Fix the remaining construction sites**

Any test helper in `src/collector/worker_tests.rs` that builds a `Snapshot` gets `locks: None`. `None` means "no data this tick", which is what a fixture that predates the field should say.

- [ ] **Step 5: Build and test**

```bash
cargo build
cargo test --lib
```

Expected: clean build, all existing tests still pass.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/collector/snapshot.rs src/collector/worker.rs src/collector/worker_tests.rs
git commit -m "feat: sample blocked locks on the fast tier"
```

---

## Task 4: The Locks page — blocked tree view

**Files:**
- Create: `src/pages/locks.rs`
- Create: `resources/ui/locks_page.blp`
- Modify: `src/pages/mod.rs`, `resources/meson.build`, `resources/mission-centre-pg.gresource.xml`

**Interfaces:**
- Consumes: `LockNode`, `build_forest`, `LocksSample`.
- Produces: `McpgLocksPage` with `pub fn update(&self, locks: Option<&Result<LocksSample, CollectorError>>)`, `pub fn set_capabilities(&self, capabilities: &Capabilities)`, `pub fn selected_pid(&self) -> Option<i32>`.

- [ ] **Step 1: Write the layout**

Create `resources/ui/locks_page.blp`, modelled on `resources/ui/sessions_page.blp`:

```blueprint
using Gtk 4.0;
using Adw 1;

template $McpgLocksPage: Adw.Bin {
  child: Gtk.Box {
    orientation: vertical;

    Adw.ViewSwitcher {
      stack: view_stack;
      policy: wide;
    }

    Adw.ViewStack view_stack {
      vexpand: true;

      Adw.ViewStackPage {
        name: "tree";
        title: _("Blocked Sessions");
        child: Gtk.Stack tree_stack {
          Gtk.StackPage {
            name: "empty";
            child: Adw.StatusPage {
              icon-name: "emblem-ok-symbolic";
              title: _("No blocked sessions");
              description: _("Nothing is waiting on a lock.");
            };
          }

          Gtk.StackPage {
            name: "tree";
            child: Gtk.ScrolledWindow {
              Gtk.ColumnView tree_view {
                show-row-separators: true;
              }
            };
          }

          Gtk.StackPage {
            name: "unavailable";
            child: Adw.StatusPage unavailable_page {
              icon-name: "dialog-information-symbolic";
              title: _("Locks unavailable");
            };
          }
        };
      }
    }

    Gtk.ActionBar action_bar {
      revealed: false;

      [start]
      Gtk.Button cancel_button {
        label: _("Cancel Query");
        action-name: "win.cancel-backend";
      }

      [start]
      Gtk.Button terminate_button {
        label: _("Terminate");
        action-name: "win.terminate-backend";

        styles ["destructive-action"]
      }

      [end]
      Gtk.Label reason_label {
        ellipsize: end;

        styles ["dim-label"]
      }
    }
  };
}
```

- [ ] **Step 2: Register the layout with the build**

In `resources/meson.build`, add `'ui/locks_page.blp',` to the `blueprints` input list. In `resources/mission-centre-pg.gresource.xml`, add:

```xml
    <file preprocess="xml-stripblanks">ui/locks_page.ui</file>
```

- [ ] **Step 3: Write the failing tests for the flattening helper**

The tree is rendered in a flat `ColumnView` with an indent per depth, because `ColumnView` has no native tree mode in this codebase's usage. That flattening is pure and gets tested first. Create `src/pages/locks.rs` with the GPL header, then:

```rust
use crate::collector::locks::{LockNode, LockParticipant};

/// One rendered line: a node plus how deep it sits, so the first column can
/// indent it.
#[derive(Debug, Clone, PartialEq)]
pub struct LockRow {
    pub depth: usize,
    pub participant: LockParticipant,
    pub in_cycle: bool,
    pub is_stub: bool,
}

pub fn flatten(forest: &[LockNode]) -> Vec<LockRow> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::locks::{build_forest, stub_participant};

    fn participant(pid: i32, blocked_by: &[i32]) -> LockParticipant {
        LockParticipant {
            blocked_by: blocked_by.to_vec(),
            ..stub_participant(pid)
        }
    }

    #[test]
    fn an_empty_forest_flattens_to_nothing() {
        assert!(flatten(&[]).is_empty());
    }

    #[test]
    fn a_chain_flattens_depth_first_with_increasing_depth() {
        let forest = build_forest(&[participant(100, &[]), participant(200, &[100])]);
        let rows = flatten(&forest);

        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].participant.pid, rows[0].depth), (100, 0));
        assert_eq!((rows[1].participant.pid, rows[1].depth), (200, 1));
    }

    #[test]
    fn siblings_share_a_depth_and_follow_their_parent() {
        let forest = build_forest(&[
            participant(100, &[]),
            participant(200, &[100]),
            participant(300, &[100]),
        ]);
        let rows = flatten(&forest);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 1);
    }
}
```

- [ ] **Step 4: Run the tests and watch them fail**

```bash
cargo test --lib pages::locks
```

Expected: the empty case passes, the two structural cases fail with a length of 0.

- [ ] **Step 5: Implement the flattening**

```rust
pub fn flatten(forest: &[LockNode]) -> Vec<LockRow> {
    let mut rows = Vec::new();
    for node in forest {
        push_node(node, 0, &mut rows);
    }
    rows
}

fn push_node(node: &LockNode, depth: usize, rows: &mut Vec<LockRow>) {
    rows.push(LockRow {
        depth,
        participant: node.participant.clone(),
        in_cycle: node.in_cycle,
        is_stub: node.is_stub,
    });
    for child in &node.children {
        push_node(child, depth + 1, rows);
    }
}
```

- [ ] **Step 6: Run the tests and watch them pass**

```bash
cargo test --lib pages::locks
```

Expected: three passes.

- [ ] **Step 7: Build the widget**

Add the `McpgLocksPage` subclass below the helper, following `src/pages/sessions.rs` exactly for the template, `imp` struct and `Table` wiring. The columns:

| Title | Render | Sort key |
|---|---|---|
| Blocked | `"  ".repeat(depth) + &pid.to_string()`, suffixed `" (cycle)"` when `in_cycle` and `" (gone)"` when `is_stub` | `pid` |
| User | `user_name` or empty | none |
| Database | `database` or empty | none |
| State | `state` or empty | none |
| Waiting | `wait_secs` formatted by `crate::pages::format` as a duration, or `—` | `wait_secs` |
| Lock | `lock_mode` or `—` | none |
| Object | `relation` or `—` | none |
| Query | `query` collapsed onto one line, or the not-permitted message when `None` | none |

The row key for selection is the pid as a string, matching how `Session` rows key themselves:

```rust
Table::attach(&view, COLUMNS, |_| true, |row| row.participant.pid.to_string())
```

`update` builds the forest, flattens it, and switches `tree_stack`: `"empty"` when the forest is empty, `"tree"` when it is not, `"unavailable"` when the result is `Err`, setting `unavailable_page`'s description to the error's message so the PostgreSQL text reaches the user (spec §8).

- [ ] **Step 8: Declare the page and build**

Add `pub mod locks;` to `src/pages/mod.rs`.

```bash
ninja -C build
cargo test --lib
```

Expected: builds clean, all tests pass.

- [ ] **Step 9: Commit**

```bash
cargo fmt
git add src/pages/locks.rs src/pages/mod.rs resources/ui/locks_page.blp resources/meson.build resources/mission-centre-pg.gresource.xml
git commit -m "feat: locks page with the blocked-session tree"
```

---

## Task 5: Wire the page into the window

**Files:**
- Modify: `resources/ui/window.blp`, `src/window.rs`

**Interfaces:**
- Consumes: `McpgLocksPage` from Task 4.
- Produces: a Locks entry in the view stack, updated on every snapshot.

- [ ] **Step 1: Add the stack page**

In `resources/ui/window.blp`, after the `relations` `Adw.ViewStackPage` — the slot Phase 2 deliberately left free:

```blueprint
          Adw.ViewStackPage {
            name: "locks";
            title: _("Locks");
            icon-name: "changes-prevent-symbolic";
            child: $McpgLocksPage locks_page {};
          }
```

- [ ] **Step 2: Hold and update it**

In `src/window.rs`, add `locks_page: TemplateChild<McpgLocksPage>` to the `imp` struct beside the existing pages, and in the snapshot handler — next to where `relations_page.update(...)` is called:

```rust
imp.locks_page.update(snapshot.locks.as_ref());
```

Wherever `set_capabilities` is called for the sessions page, call it for the locks page too, so the action bar gates identically.

- [ ] **Step 3: Build and run**

```bash
ninja -C build
MCPG_RESOURCE_DIR=$PWD/build/resources GSETTINGS_SCHEMA_DIR=$PWD/data ./build/src/mission-centre-pg
```

Expected: a Locks entry appears in the switcher, showing "No blocked sessions" against an idle server.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add resources/ui/window.blp src/window.rs
git commit -m "feat: add the locks page to the window"
```

---

## Task 6: The inventory view and visibility gating

**Files:**
- Modify: `src/collector/locks.rs`, `src/collector/snapshot.rs`, `src/collector/worker.rs`, `src/pages/locks.rs`, `resources/ui/locks_page.blp`, `data/io.github.paulsnow.MissionCentrePg.gschema.xml`, `src/window.rs`, `tests/portability.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: `pub const INVENTORY_SQL: &str`; `pub struct LockInventorySample { pub locks: Vec<LockEntry>, pub total: i64 }`; `Snapshot.lock_inventory`; a `locks-limit` setting; an `Arc<AtomicBool>` gate on the sample config.

- [ ] **Step 1: Add the setting**

In `data/io.github.paulsnow.MissionCentrePg.gschema.xml`, beside `relations-limit`:

```xml
    <key name="locks-limit" type="i">
      <default>500</default>
      <range min="50" max="5000"/>
      <summary>Maximum lock rows shown in the inventory</summary>
      <description>
        How many rows the full lock inventory fetches. The page reports the
        total when the limit truncates the list.
      </description>
    </key>
```

- [ ] **Step 2: Write the inventory query with its total**

In `src/collector/locks.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct LockEntry {
    pub pid: i32,
    pub lock_type: Option<String>,
    pub mode: Option<String>,
    pub granted: bool,
    pub relation: Option<String>,
    pub user_name: Option<String>,
    pub database: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LockInventorySample {
    pub locks: Vec<LockEntry>,
    /// Every lock the server holds, so the page can say "showing 500 of
    /// 4,312" rather than silently truncating.
    pub total: i64,
}

pub const INVENTORY_SQL: &str = "\
SELECT l.pid,
       l.locktype::text                     AS lock_type,
       l.mode                               AS mode,
       l.granted                            AS granted,
       l.relation::regclass::text           AS relation,
       a.usename::text                      AS user_name,
       a.datname::text                      AS database,
       count(*) OVER ()                     AS total
FROM pg_locks l
LEFT JOIN pg_stat_activity a ON a.pid = l.pid
ORDER BY l.granted, l.pid
LIMIT $1";
```

`count(*) OVER ()` gives the untruncated total in the same pass, so no second query is needed.

- [ ] **Step 3: Write the failing portability tests**

```rust
#[tokio::test]
async fn inventory_sql_runs_on_postgres_14() {
    let (client, _container) = connect(14).await;
    let rows = client
        .query(mission_centre_pg::collector::locks::INVENTORY_SQL, &[&500i64])
        .await
        .expect("the inventory query must run on PostgreSQL 14");
    assert!(!rows.is_empty(), "the querying backend holds locks of its own");
}

#[tokio::test]
async fn inventory_sql_runs_on_postgres_18() {
    let (client, _container) = connect(18).await;
    let rows = client
        .query(mission_centre_pg::collector::locks::INVENTORY_SQL, &[&500i64])
        .await
        .expect("the inventory query must run on PostgreSQL 18");
    assert!(!rows.is_empty());
}

#[tokio::test]
async fn the_inventory_total_exceeds_the_limit_when_truncated_on_postgres_18() {
    let (client, _container) = connect(18).await;
    let rows = client
        .query(mission_centre_pg::collector::locks::INVENTORY_SQL, &[&1i64])
        .await
        .expect("query runs");
    let total: i64 = rows[0].get("total");
    assert_eq!(rows.len(), 1, "the limit is honoured");
    assert!(total >= 1, "the total counts past the limit: {total}");
}
```

- [ ] **Step 4: Run them and watch them fail, then pass**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability inventory_sql
cargo test --test portability the_inventory_total
```

Expected: compile failure first, then three passes once `INVENTORY_SQL` and the types exist.

- [ ] **Step 5: Add the gate**

In `src/collector/worker.rs`, add to the config struct that already carries `relations_limit`:

```rust
    pub locks_limit: i64,
    /// Set by the window when the lock inventory view is on screen. The
    /// query is expensive and rarely watched, so it runs only while it is
    /// visible — and fails closed, because an inventory one refresh stale is
    /// a far better failure than an expensive query running forever.
    pub inventory_visible: Arc<AtomicBool>,
```

In `sample`, after the blocked-tree sample:

```rust
    let lock_inventory = if config.inventory_visible.load(Ordering::Relaxed) {
        Some(classify_slow(
            client
                .query(INVENTORY_SQL, &[&config.locks_limit])
                .await
                .map_err(map_query_error)
                .map(|rows| LockInventorySample {
                    total: rows.first().map(|row| row.get("total")).unwrap_or(0),
                    locks: rows.iter().map(map_lock_entry).collect(),
                }),
        )?)
    } else {
        None
    };
```

Add `pub lock_inventory: Option<Result<LockInventorySample, CollectorError>>` to `Snapshot` and populate it. `None` here means "not sampled", which the page renders as its resting state rather than as an error.

- [ ] **Step 6: Add the view and flip the flag**

Add a second `Adw.ViewStackPage` named `inventory` to `resources/ui/locks_page.blp`, holding a `ColumnView` with columns PID, Type, Mode, Granted, Object, User, Database, plus a `Gtk.Label truncation_label` below it.

In `src/pages/locks.rs`, connect to the view stack's `visible-child-name` notification and store the result through a callback the window installs:

```rust
pub fn connect_inventory_visibility(&self, f: impl Fn(bool) + 'static) {
    let stack = self.imp().view_stack.clone();
    stack.connect_visible_child_name_notify(move |stack| {
        f(stack.visible_child_name().as_deref() == Some("inventory"));
    });
}
```

In `src/window.rs`, install a callback that sets the `AtomicBool` the collector holds. Set it to `false` when the window is destroyed and when a server is deselected, so the flag cannot leak into a collector nobody is watching.

- [ ] **Step 7: Report truncation**

In the page's update for the inventory:

```rust
if sample.total > sample.locks.len() as i64 {
    imp.truncation_label.set_text(&i18n(&format!(
        "Showing {} of {} locks",
        sample.locks.len(),
        sample.total
    )));
    imp.truncation_label.set_visible(true);
} else {
    imp.truncation_label.set_visible(false);
}
```

- [ ] **Step 8: Build, test and commit**

```bash
ninja -C build
cargo test --lib
cargo test --test portability
cargo fmt
git add -A
git commit -m "feat: lock inventory view, sampled only while visible"
```

---

## Task 7: Cancel and terminate from the Locks page

**Files:**
- Modify: `src/pages/locks.rs`, `src/window_actions.rs`

**Interfaces:**
- Consumes: `Action::CancelBackend`, `Action::TerminateBackend`, `Capabilities.signal_backend`, and the existing confirmation and toast machinery.
- Produces: no new types. This task adds a second source of an existing action.

- [ ] **Step 1: Gate the action bar**

`set_capabilities` mirrors the sessions page exactly:

```rust
pub fn set_capabilities(&self, capabilities: &Capabilities) {
    let imp = self.imp();
    imp.signal_allowed.set(capabilities.signal_backend);
    if !capabilities.signal_backend {
        imp.reason_label
            .set_text(&i18n("Requires the pg_signal_backend role"));
    }
    imp.reason_label.set_visible(!capabilities.signal_backend);
    self.update_action_sensitivity();
}
```

Buttons are sensitive only when a row is selected *and* `signal_allowed` is set, matching `src/pages/sessions.rs`.

- [ ] **Step 2: Refuse the actions on a stub row**

A stub row carries a pid and nothing else — the backend was already gone when the sample was taken. Cancelling it can only ever report "no longer running", so the buttons stay insensitive for a stub, with the reason stated:

```rust
if row.is_stub {
    imp.reason_label
        .set_text(&i18n("That backend has already exited"));
    imp.reason_label.set_visible(true);
    return false;
}
```

- [ ] **Step 3: Write the failing test**

In `src/pages/locks.rs` tests:

```rust
#[test]
fn a_stub_row_offers_no_actions_because_its_backend_has_gone() {
    let forest = build_forest(&[participant(200, &[999])]);
    let rows = flatten(&forest);

    let stub = rows.iter().find(|row| row.participant.pid == 999).unwrap();
    assert!(!actions_available(stub, true));
    let waiter = rows.iter().find(|row| row.participant.pid == 200).unwrap();
    assert!(actions_available(waiter, true));
}

#[test]
fn no_row_offers_actions_without_the_signal_capability() {
    let forest = build_forest(&[participant(100, &[]), participant(200, &[100])]);
    let rows = flatten(&forest);
    assert!(rows.iter().all(|row| !actions_available(row, false)));
}
```

with the pure predicate the widget calls:

```rust
pub fn actions_available(row: &LockRow, signal_allowed: bool) -> bool {
    signal_allowed && !row.is_stub
}
```

- [ ] **Step 4: Run, watch fail, implement, watch pass**

```bash
cargo test --lib pages::locks
```

- [ ] **Step 5: Resolve the action target**

In `src/window_actions.rs`, `action_for` currently resolves the selected session. Extend it so that when the visible page is `locks`, the selected pid comes from `locks_page.selected_pid()`. The confirmation dialog, capability check, submission and toasts are all unchanged — this is the same `Action` from a different source.

- [ ] **Step 6: Build, test, commit**

```bash
ninja -C build
cargo test --lib
cargo test --bin mission-centre-pg
cargo fmt
git add src/pages/locks.rs src/window_actions.rs
git commit -m "feat: cancel and terminate a blocking backend from the locks page"
```

---

## Task 8: Full verification

**Files:** none modified unless a check fails.

- [ ] **Step 1: Run every automated check**

```bash
cargo fmt --check
cargo test --lib
cargo test --bin mission-centre-pg
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
ninja -C build
```

Expected: no diff from `fmt`; all unit tests pass; all container tests pass on 14 and 18; the build completes.

- [ ] **Step 2: Confirm no file has grown past the limit**

```bash
find src -name '*.rs' -exec wc -l {} + | sort -rn | head -5
```

Expected: the largest file is under ~800 lines. If `locks.rs` is not, split the tree builder into `src/collector/locks_tree.rs` before continuing.

- [ ] **Step 3: Walk the success criteria against a live server**

```bash
podman run --rm -d --name mcpg-p5 -e POSTGRES_PASSWORD=postgres -p 55432:5432 docker.io/library/postgres:18
podman exec mcpg-p5 bash -c 'until pg_isready -U postgres -q; do sleep 1; done'
podman exec -i mcpg-p5 psql -U postgres -c "CREATE TABLE conflict (id int PRIMARY KEY, note text)"
podman exec -i mcpg-p5 psql -U postgres -c "INSERT INTO conflict VALUES (1, 'a'), (2, 'b')"
```

Build a three-deep chain from three terminals, each `podman exec -it mcpg-p5 psql -U postgres`:

```sql
-- terminal 1: the root, then stop typing
BEGIN; UPDATE conflict SET note = 'held' WHERE id = 1;
-- terminal 2: waits on terminal 1
UPDATE conflict SET note = 'second' WHERE id = 1;
-- terminal 3: waits on terminal 2
UPDATE conflict SET note = 'third' WHERE id = 1;
```

Then tick each, which are the spec's §10 criteria 1–6 and 11:

- [ ] A two-node chain appears within one refresh of the block starting, naming both backends' pid, user, database and query.
- [ ] The three-deep chain renders as one tree, with the `idle in transaction` root at the top.
- [ ] Terminating the root from the Locks page clears the tree on the next sample.
- [ ] With no contention, the page reads "No blocked sessions".
- [ ] The inventory view reports truncation when `locks-limit` is lowered below the lock count.
- [ ] The inventory query does not run while its view is not selected — confirm from the server, not the UI: `podman exec -i mcpg-p5 psql -U postgres -c "SELECT query FROM pg_stat_activity WHERE application_name='mission-centre-pg'"` shows no `pg_locks` query while the tree view is on screen.
- [ ] As a role without `pg_monitor`, the page either shows data or states the privilege required — never a silently empty table.

- [ ] **Step 4: Tear down**

```bash
podman rm -f mcpg-p5
```

- [ ] **Step 5: Commit any fixes and open the pull request**

```bash
git status --short
git push -u origin phase-5-locks
gh pr create --title "Phase 5: locks page" \
  --body "Implements the Locks half of docs/superpowers/specs/2026-07-27-phase-5-replication-and-locks-design.md — a blocked-session tree on the fast tier, a full lock inventory sampled only while its view is visible, and cancel/terminate on the selected backend reusing the Phase 4 action machinery."
```

---

## Self-Review Notes

**Spec coverage.** §4.1 blocked-tree query → Task 2, including the union of waiters and blockers and the `wait_event_type = 'Lock'` filter. §4.2 tree builder → Task 1, with a test per named case: cycle, missing blocker, several waiters on one blocker. §4.3 inventory and visibility gating → Task 6, including the `locks-limit` key and the truncation message. §4.4 selection and actions → Tasks 4 and 7. §4.5 empty state → Task 4 Step 7. §6.1 fast tier → Task 3. §6.2 fail-closed gate → Task 6 Steps 5 and 6. §7 module layout → the File Structure table. §8 three failure states → Task 4 Step 7 renders the PostgreSQL message; Task 7 Step 2 states the not-permitted reason. §9.1 unit tests → Tasks 1, 4, 7. §9.2 portability on both versions with both roles → Tasks 2 and 6. §10 criteria 1–6 and 11 → Task 8 Step 3, one tick each.

**Deliberately not covered.** §10 criteria 7–10 are replication criteria and belong to the Replication plan. §3.2's version matrix has no lock-related branch — `pg_locks` and `pg_blocking_pids` are identical on 14 and 18, which Task 2's tests confirm rather than assume.

**Type consistency.** `LockParticipant` is defined once in Task 1 and used unchanged in Tasks 2, 4 and 7. `LockNode` gains no fields after Task 1. `LockRow` (the page's flattened line) is distinct from `LockEntry` (an inventory row) and the two never mix: the tree view renders `LockRow`, the inventory renders `LockEntry`. `build_forest` and `flatten` keep their names throughout. `stub_participant` is used by both the builder and the page tests.

**One trap worth knowing.** `Snapshot.locks` is `Option<Result<...>>` where `None` cannot occur in practice, because the tree samples on every tick — but the field stays `Option` so existing test fixtures compile with `locks: None` and so the type matches `statements` and `relations`. `Snapshot.lock_inventory` uses `None` meaningfully: it is the resting state when the view is not visible, and the page must render that as "not sampled", never as an error.

# Mission Centre PostgreSQL — Phase 4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user cancel a query, terminate a backend, `VACUUM` or `ANALYZE` a table, reset the query statistics and reload the server configuration — each offered only when the connected role can actually perform it, each naming its exact target before it runs.

**Architecture:** A new GTK-free `src/actions/` module turns an `Action` into a `Plan` (the SQL, whether it binds a PID, which protocol, which timeouts) as pure functions. The collector thread gains a command channel; an accepted action spawns a task that opens its **own** connection, so a multi-minute `VACUUM` never blocks the two-second sample loop. The connect probe gains four capability columns, and the tables query gains a per-row `can_maintain`. On the UI side the shared `Table` grows a single selection that survives the two-second refresh, pages grow action bars, and the window owns the `GAction`s, the confirmation dialogs and the result toasts.

**Tech Stack:** Rust, gtk4-rs 0.11, libadwaita 0.9 (v1_5), tokio-postgres 0.7, async-channel 2.5, testcontainers 0.27.

**Spec:** `docs/superpowers/specs/2026-07-25-phase-4-actions-design.md`
**Parent spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Repository:** `/home/paul/gitHUB/mission-centre-postgresql`. Branch: `phase-4-actions`.
- **Licence:** GPL-3.0-or-later. Every new source file carries the same GPL header block as its neighbours (copy from `src/pages/format.rs`, changing only the first line), naming **Paul Snow** as author, ending `SPDX-License-Identifier: GPL-3.0-or-later`.
- **Version:** `0.0.0`.
- **Phase 4 is the first phase that changes state the DBA cares about.** No action may run without the user having asked for it by clicking a control. Nothing is retried automatically. Nothing is inferred.
- **An action failure must never fail a sample or disconnect the collector.** The action path and the sampling path share a thread and a runtime, and nothing else.
- **Out of scope, do not add:** `VACUUM FULL`, `REINDEX`, `ALTER SYSTEM`, an action log, multi-select or bulk actions.
- **Never log or display a password**, nor a full connection string.
- **PostgreSQL floor 14.** Every statement must run on 14 through 18. Where it cannot, the version branch is explicit and tested.
- **Never touch GTK widgets off the main thread.** `src/actions/` and `src/collector/` stay GTK-free.
- **Spelling:** British English in all user-facing strings, comments and documentation (`behaviour`, `initialise`, `colour`).
- **Cargo renames the GTK crates:** code says `gtk::` and `adw::`.
- **`glib::wrapper!` blocks for `CompositeTemplate` widgets must list** `gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget`, plus `gtk::Orientable` for `gtk::Box` subclasses.
- **Every `.blp` change is compiled by `ninja -C build`**, not by `cargo` — build before committing UI changes.
- **`cargo fmt` must produce no diff** before any commit.
- **File size:** no source file over ~800 lines. `src/collector/worker.rs` is at 689 lines and `src/window.rs` at 492; Tasks 4 and 8 put their new code in new sibling files for exactly this reason.
- **All user-facing strings go through `crate::i18n::{i18n, i18n_f}`.**

### Conventions from earlier phases (follow them)

- **Pure functions carry the logic; GTK wiring stays thin.** Anything decidable is a free function with unit tests in the same file, as in `probe.rs` and `table/mod.rs`.
- **`None` means "no honest figure exists", never zero.**
- **A collector error that is a property of the schema or role, not the connection, degrades one feature** rather than failing the sample — the `classify_slow` pattern in `worker.rs`.
- **Unit tests live in a `#[cfg(test)] mod tests` at the foot of the file they test**, except `worker.rs`, which uses `#[path = "worker_tests.rs"]`.

### Commands

| Purpose | Command |
|---------|---------|
| Unit tests | `cargo test --lib` |
| One module's unit tests | `cargo test --lib actions::` |
| Container tests | `export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"; cargo test --test portability` |
| One container test | `export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"; cargo test --test portability capability_probe -- --nocapture` |
| Format check | `cargo fmt --check` |
| Compile check | `cargo check --all-targets` |
| Full build (compiles `.blp`) | `ninja -C build` |
| Run | `MCPG_RESOURCE_DIR=$PWD/build/resources GSETTINGS_SCHEMA_DIR=$PWD/data ./build/src/mission-centre-pg` |

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/actions/mod.rs` | `Action`, `MaintenanceKind`, `ActionOutcome`, `requires_confirmation`, `signal_outcome` | 1 |
| `src/actions/sql.rs` | `quote_ident`, `qualified_name`, `Plan`, `plan_for` | 1 |
| `src/lib.rs` | declare `pub mod actions;` | 1 |
| `src/connection/probe.rs` | four capability columns, `Capabilities` on `ServerInfo` | 2 |
| `tests/portability.rs` | the capability probe and the maintenance statements run on 14 and 18 | 2, 3, 4 |
| `src/collector/relations.rs` | `tables_sql(version_num)`, `TableStats::can_maintain` | 3 |
| `src/collector/action_runner.rs` | opens the action connection, runs the plan, returns an outcome | 4 |
| `src/collector/worker.rs` | command channel, `submit`, spawn on receipt, two new events | 4 |
| `src/collector/mod.rs` | declare `pub mod action_runner;` | 4 |
| `src/table/mod.rs` | `SingleSelection`, row keys, `reselect_index`, `selected` | 5 |
| `src/pages/sessions.rs` | row key, `selected_session`, `set_capabilities`, selection hook | 6 |
| `resources/ui/sessions_page.blp` | the sessions action bar | 6 |
| `src/pages/relations.rs` | row keys, `selected_table`, `set_capabilities`, selection hook | 7 |
| `resources/ui/relations_page.blp` | the tables action bar | 7 |
| `src/pages/queries.rs` | row key only (no row action this phase) | 5 |
| `src/window_actions.rs` | `GAction`s, enablement, confirmation dialogs, toasts | 8 |
| `src/window.rs` | toast overlay child, install the actions, forward the two new events | 8 |
| `resources/ui/window.blp` | `Adw.ToastOverlay`, header-bar menu | 8 |
| `src/main.rs` | declare `mod window_actions;` | 8 |

---

## Deviations from the spec

Recorded here so a reviewer can see they are deliberate.

1. **Spec §3.4 says the reason for a disabled button is a tooltip.** GTK does not deliver tooltips to insensitive widgets, so a tooltip alone would be invisible in exactly the case it exists for. Each action bar therefore carries a **dim label** to the left of its buttons showing the reason, and the tooltip is set as well for when the button is sensitive. Behaviour matches the spec's intent; the mechanism differs.

2. **The action connection reuses `worker::connect`**, which also runs `PROBE_SQL`. A dedicated probe-free connect would save one round trip on a user-initiated action — invisible — at the cost of duplicating the rustls and TLS-mode handling. Reuse wins.

---

## Task 1: The actions module — pure SQL planning

**Files:**
- Create: `src/actions/mod.rs`
- Create: `src/actions/sql.rs`
- Modify: `src/lib.rs:21-28`
- Test: unit tests inside both new files

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `crate::actions::{Action, MaintenanceKind, ActionOutcome}`
  - `Action::requires_confirmation(&self) -> bool`
  - `crate::actions::signal_outcome(returned: bool) -> ActionOutcome`
  - `crate::actions::sql::{quote_ident, qualified_name, Plan, plan_for}`
  - `Plan { setup: String, sql: String, pid: Option<i32>, batch: bool }`

- [ ] **Step 1: Write the failing tests for `src/actions/sql.rs`**

Create `src/actions/sql.rs` containing **only** the GPL header (copied from `src/pages/format.rs`, first line changed to `/* actions/sql.rs`), then this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{Action, MaintenanceKind};

    #[test]
    fn a_plain_identifier_is_still_quoted() {
        // Always quoting is what makes a reserved word or a capitalised name
        // safe; there is no case where leaving it bare is worth the branch.
        assert_eq!(quote_ident("orders"), "\"orders\"");
    }

    #[test]
    fn an_embedded_double_quote_is_doubled() {
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn a_name_carrying_sql_is_neutralised() {
        // Legal PostgreSQL: CREATE TABLE "x"; DROP TABLE bar --".
        assert_eq!(
            quote_ident("x\"; DROP TABLE bar --"),
            "\"x\"\"; DROP TABLE bar --\""
        );
    }

    #[test]
    fn case_is_preserved_because_quoting_makes_it_significant() {
        assert_eq!(quote_ident("MyTable"), "\"MyTable\"");
    }

    #[test]
    fn a_qualified_name_quotes_both_parts() {
        assert_eq!(qualified_name("public", "orders"), "\"public\".\"orders\"");
    }

    #[test]
    fn analyze_runs_on_the_simple_protocol_with_no_statement_timeout() {
        let plan = plan_for(&Action::Maintain {
            kind: MaintenanceKind::Analyze,
            schema: "public".to_string(),
            table: "orders".to_string(),
        });
        assert_eq!(plan.sql, "ANALYZE \"public\".\"orders\"");
        assert!(plan.batch, "maintenance must not use the extended protocol");
        assert_eq!(plan.pid, None);
        assert_eq!(
            plan.setup,
            "SET statement_timeout = 0; SET lock_timeout = '30s'"
        );
    }

    #[test]
    fn vacuum_and_vacuum_analyze_have_distinct_spellings() {
        let vacuum = plan_for(&Action::Maintain {
            kind: MaintenanceKind::Vacuum,
            schema: "public".to_string(),
            table: "orders".to_string(),
        });
        assert_eq!(vacuum.sql, "VACUUM \"public\".\"orders\"");

        let both = plan_for(&Action::Maintain {
            kind: MaintenanceKind::VacuumAnalyze,
            schema: "public".to_string(),
            table: "orders".to_string(),
        });
        assert_eq!(both.sql, "VACUUM (ANALYZE) \"public\".\"orders\"");
    }

    #[test]
    fn the_signal_actions_bind_the_pid_rather_than_interpolating_it() {
        let cancel = plan_for(&Action::CancelBackend { pid: 4821 });
        assert_eq!(cancel.sql, "SELECT pg_cancel_backend($1)");
        assert_eq!(cancel.pid, Some(4821));
        assert!(!cancel.batch);
        assert_eq!(cancel.setup, "SET statement_timeout = '5s'");

        let terminate = plan_for(&Action::TerminateBackend { pid: 4821 });
        assert_eq!(terminate.sql, "SELECT pg_terminate_backend($1)");
        assert_eq!(terminate.pid, Some(4821));
    }

    #[test]
    fn the_server_wide_actions_take_no_parameter() {
        let reset = plan_for(&Action::ResetStatements);
        assert_eq!(reset.sql, "SELECT pg_stat_statements_reset()");
        assert_eq!(reset.pid, None);
        assert!(!reset.batch);

        let reload = plan_for(&Action::ReloadConfig);
        assert_eq!(reload.sql, "SELECT pg_reload_conf()");
        assert_eq!(reload.pid, None);
        assert!(!reload.batch);
    }

    #[test]
    fn only_maintenance_lifts_the_statement_timeout() {
        for action in [
            Action::CancelBackend { pid: 1 },
            Action::TerminateBackend { pid: 1 },
            Action::ResetStatements,
            Action::ReloadConfig,
        ] {
            assert_eq!(
                plan_for(&action).setup,
                "SET statement_timeout = '5s'",
                "{action:?} must keep the sampler's timeout"
            );
        }
    }
}
```

- [ ] **Step 2: Write the failing tests for `src/actions/mod.rs`**

Create `src/actions/mod.rs` containing the GPL header (first line `/* actions/mod.rs`), then:

```rust
pub mod sql;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_affecting_other_users_or_destroying_data_are_confirmed() {
        assert!(Action::CancelBackend { pid: 1 }.requires_confirmation());
        assert!(Action::TerminateBackend { pid: 1 }.requires_confirmation());
        assert!(Action::ResetStatements.requires_confirmation());
    }

    #[test]
    fn idempotent_and_routine_actions_are_not_confirmed() {
        assert!(!Action::ReloadConfig.requires_confirmation());
        for kind in [
            MaintenanceKind::Analyze,
            MaintenanceKind::Vacuum,
            MaintenanceKind::VacuumAnalyze,
        ] {
            assert!(!Action::Maintain {
                kind,
                schema: "public".to_string(),
                table: "orders".to_string(),
            }
            .requires_confirmation());
        }
    }

    #[test]
    fn a_signal_that_found_its_backend_succeeded() {
        assert_eq!(signal_outcome(true), ActionOutcome::Succeeded);
    }

    #[test]
    fn a_signal_that_found_nothing_is_neither_success_nor_failure() {
        // pg_cancel_backend returns false when the PID has already gone.
        // Reporting that as success would claim work that never happened;
        // reporting it as an error would blame the user for a race.
        assert_eq!(signal_outcome(false), ActionOutcome::NoSuchBackend);
    }

    #[test]
    fn maintenance_reports_the_relation_it_targeted() {
        let action = Action::Maintain {
            kind: MaintenanceKind::Vacuum,
            schema: "public".to_string(),
            table: "orders".to_string(),
        };
        assert_eq!(action.target(), Some("public.orders".to_string()));
        assert_eq!(
            Action::CancelBackend { pid: 4821 }.target(),
            Some("4821".to_string())
        );
        assert_eq!(Action::ReloadConfig.target(), None);
    }

    #[test]
    fn only_maintenance_is_long_running() {
        assert!(Action::Maintain {
            kind: MaintenanceKind::Vacuum,
            schema: "public".to_string(),
            table: "orders".to_string(),
        }
        .is_long_running());
        assert!(!Action::CancelBackend { pid: 1 }.is_long_running());
        assert!(!Action::ResetStatements.is_long_running());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib actions::`
Expected: FAIL — `error[E0583]: file not found for module 'actions'` until Step 4 adds the declaration, then `cannot find type Action in this scope`.

- [ ] **Step 4: Declare the module**

In `src/lib.rs`, add `pub mod actions;` so the list stays alphabetical:

```rust
pub mod actions;
pub mod collector;
pub mod connection;
pub mod dialogs;
pub mod history;
pub mod i18n;
pub mod pages;
pub mod table;
pub mod widgets;
```

- [ ] **Step 5: Implement `src/actions/mod.rs`**

Insert this above the `#[cfg(test)] mod tests` block, below the `pub mod sql;` line:

```rust
/// Which maintenance command to run. Kept separate from `Action` so the three
/// variants share one target and one capability check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceKind {
    Analyze,
    Vacuum,
    VacuumAnalyze,
}

/// A single operation the user asked for. Never constructed by the collector
/// or by a timer — only by a control the user activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CancelBackend {
        pid: i32,
    },
    TerminateBackend {
        pid: i32,
    },
    Maintain {
        kind: MaintenanceKind,
        schema: String,
        table: String,
    },
    ResetStatements,
    ReloadConfig,
}

impl Action {
    /// True for anything that interrupts another user's work or destroys data
    /// that cannot be recovered. `pg_reload_conf` is idempotent and loses
    /// nothing; VACUUM and ANALYZE are what autovacuum does unprompted.
    pub fn requires_confirmation(&self) -> bool {
        matches!(
            self,
            Action::CancelBackend { .. } | Action::TerminateBackend { .. } | Action::ResetStatements
        )
    }

    /// What the action was aimed at, for the result message. `None` for the
    /// server-wide actions, whose message names no target.
    pub fn target(&self) -> Option<String> {
        match self {
            Action::CancelBackend { pid } | Action::TerminateBackend { pid } => Some(pid.to_string()),
            Action::Maintain { schema, table, .. } => Some(format!("{schema}.{table}")),
            Action::ResetStatements | Action::ReloadConfig => None,
        }
    }

    /// True when the action may take minutes rather than milliseconds, so the
    /// window knows to post a persistent in-flight notice rather than assume
    /// the result toast will follow immediately.
    pub fn is_long_running(&self) -> bool {
        matches!(self, Action::Maintain { .. })
    }
}

/// How an action ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Succeeded,
    /// The signal functions return false when the PID has already gone.
    NoSuchBackend,
    Failed(String),
}

/// Classifies the boolean `pg_cancel_backend` and `pg_terminate_backend`
/// return.
pub fn signal_outcome(returned: bool) -> ActionOutcome {
    if returned {
        ActionOutcome::Succeeded
    } else {
        ActionOutcome::NoSuchBackend
    }
}
```

- [ ] **Step 6: Implement `src/actions/sql.rs`**

Insert above its test module:

```rust
use crate::actions::{Action, MaintenanceKind};

/// Wraps a name in double quotes, doubling any it already contains.
///
/// `VACUUM` cannot be parameterised, so its identifiers are interpolated. The
/// names come from the catalogue rather than from the user, but
/// `CREATE TABLE "x""; DROP TABLE bar --"` is legal PostgreSQL, so the quoting
/// is required rather than decorative. Quoting unconditionally also preserves
/// case, which matters the moment a name is not all lower case.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn qualified_name(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

/// Everything the runner needs to execute one action: the session settings to
/// apply first, the statement, whether a PID binds as `$1`, and whether the
/// simple protocol is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub setup: String,
    pub sql: String,
    pub pid: Option<i32>,
    /// True when the statement must go through `batch_execute`. `VACUUM`
    /// cannot run inside a transaction block, and the extended protocol wraps
    /// its statement in an implicit one.
    pub batch: bool,
}

/// The sampler's own guard against a wedged server; short actions inherit it.
const QUICK_SETUP: &str = "SET statement_timeout = '5s'";

/// Maintenance runs without a statement timeout — a VACUUM may legitimately
/// take an hour, and a timeout firing part-way discards the work already done
/// — but keeps a lock timeout, so a VACUUM blocked behind conflicting DDL
/// reports rather than hanging invisibly.
const MAINTENANCE_SETUP: &str = "SET statement_timeout = 0; SET lock_timeout = '30s'";

pub fn plan_for(action: &Action) -> Plan {
    match action {
        Action::CancelBackend { pid } => Plan {
            setup: QUICK_SETUP.to_string(),
            sql: "SELECT pg_cancel_backend($1)".to_string(),
            pid: Some(*pid),
            batch: false,
        },
        Action::TerminateBackend { pid } => Plan {
            setup: QUICK_SETUP.to_string(),
            sql: "SELECT pg_terminate_backend($1)".to_string(),
            pid: Some(*pid),
            batch: false,
        },
        Action::ResetStatements => Plan {
            setup: QUICK_SETUP.to_string(),
            sql: "SELECT pg_stat_statements_reset()".to_string(),
            pid: None,
            batch: false,
        },
        Action::ReloadConfig => Plan {
            setup: QUICK_SETUP.to_string(),
            sql: "SELECT pg_reload_conf()".to_string(),
            pid: None,
            batch: false,
        },
        Action::Maintain {
            kind,
            schema,
            table,
        } => {
            let relation = qualified_name(schema, table);
            let sql = match kind {
                MaintenanceKind::Analyze => format!("ANALYZE {relation}"),
                MaintenanceKind::Vacuum => format!("VACUUM {relation}"),
                MaintenanceKind::VacuumAnalyze => format!("VACUUM (ANALYZE) {relation}"),
            };
            Plan {
                setup: MAINTENANCE_SETUP.to_string(),
                sql,
                pid: None,
                batch: true,
            }
        }
    }
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --lib actions::`
Expected: PASS — 13 tests.

- [ ] **Step 8: Format and commit**

```bash
cargo fmt
cargo fmt --check
git add src/actions/mod.rs src/actions/sql.rs src/lib.rs
git commit -m "feat: action model and SQL planning"
```

---

## Task 2: Capability probe

**Files:**
- Modify: `src/connection/probe.rs:27-32` (`PROBE_SQL`), `:105-120` (`ServerInfo`), `:127-138` (`map_server_info`)
- Test: unit tests in `src/connection/probe.rs`, integration test in `tests/portability.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `crate::connection::probe::Capabilities { signal_backend: bool, reload_conf: bool, reset_statements: bool, maintain: bool }`
  - `Capabilities::from_flags(signal_backend: Option<bool>, reload_conf: Option<bool>, reset_statements: Option<bool>, maintain: Option<bool>) -> Capabilities`
  - `ServerInfo::capabilities: Capabilities`

- [ ] **Step 1: Write the failing unit tests**

Append to the existing `#[cfg(test)] mod tests` block at the foot of `src/connection/probe.rs`:

```rust
    #[test]
    fn absent_objects_probe_as_no_capability() {
        // to_regprocedure returns NULL when pg_stat_statements is absent, and
        // the pg_roles subselect returns no row on 14-16 where pg_maintain
        // does not exist. Both reach us as None and must not be read as
        // permission.
        let caps = Capabilities::from_flags(Some(false), Some(false), None, None);
        assert!(!caps.reset_statements);
        assert!(!caps.maintain);
    }

    #[test]
    fn granted_capabilities_are_carried_through_independently() {
        let caps = Capabilities::from_flags(Some(true), Some(false), Some(true), Some(false));
        assert!(caps.signal_backend);
        assert!(!caps.reload_conf);
        assert!(caps.reset_statements);
        assert!(!caps.maintain);
    }

    #[test]
    fn a_monitor_role_has_no_action_capabilities_by_default() {
        // The parent spec says the privilege probe gates the action buttons.
        // It does not: pg_monitor grants no right to signal a backend. This
        // test is the guard against that conflation coming back.
        let caps = Capabilities::from_flags(Some(false), Some(false), Some(false), Some(false));
        assert!(!caps.signal_backend);
        assert!(!caps.reload_conf);
    }
```

Also extend the four existing `ServerInfo { .. }` literals in `recognises_a_server_below_the_supported_floor` with `capabilities: Capabilities::default(),` so the file still compiles.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib connection::probe`
Expected: FAIL with `cannot find struct, variant or union type 'Capabilities' in this scope`.

- [ ] **Step 3: Extend `PROBE_SQL`**

Replace the `PROBE_SQL` constant at `src/connection/probe.rs:27-32` with:

```rust
/// Every capability expression must be written so it cannot raise on a server
/// that lacks the object it names: this query runs on every connect, and a
/// probe that fails fails the connection. `to_regprocedure` yields NULL rather
/// than raising when pg_stat_statements is absent, and the `pg_roles`
/// subselect returns no row on 14-16, where `pg_maintain` does not exist — a
/// bare `pg_has_role(current_user, 'pg_maintain', 'member')` would raise there.
///
/// Superusers need no special case: `pg_has_role` and `has_function_privilege`
/// both return true for them.
pub const PROBE_SQL: &str = "\
SELECT current_setting('server_version_num')::int AS version_num,
       pg_has_role(current_user, 'pg_monitor', 'member') AS is_monitor,
       COALESCE((SELECT rolsuper FROM pg_roles WHERE rolname = current_user), false) AS is_superuser,
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_stat_statements')
         AS statements_version,
       pg_has_role(current_user, 'pg_signal_backend', 'member') AS can_signal,
       has_function_privilege(current_user, 'pg_reload_conf()', 'execute') AS can_reload,
       (SELECT has_function_privilege(current_user, p.oid, 'execute')
          FROM pg_proc p
         WHERE p.oid = to_regprocedure('pg_stat_statements_reset()'))
         AS can_reset_statements,
       (SELECT pg_has_role(current_user, oid, 'member')
          FROM pg_roles WHERE rolname = 'pg_maintain')
         AS can_maintain";
```

- [ ] **Step 4: Add `Capabilities` and put it on `ServerInfo`**

Insert immediately above `pub struct ServerInfo` at `src/connection/probe.rs:105`:

```rust
/// What the connected role may *do*, as distinct from what it may *see*.
///
/// `PrivilegeLevel` answers visibility and drives the window banner; it is the
/// wrong authority for actions in both directions. `pg_monitor` grants no
/// right to signal a backend, and a plain role granted `pg_signal_backend`, or
/// one that merely owns the table it wants to ANALYZE, may act without holding
/// either level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub signal_backend: bool,
    pub reload_conf: bool,
    pub reset_statements: bool,
    pub maintain: bool,
}

impl Capabilities {
    /// SQL NULL — an absent extension, an absent `pg_maintain` role — means
    /// the capability could not be established, which is never permission.
    pub fn from_flags(
        signal_backend: Option<bool>,
        reload_conf: Option<bool>,
        reset_statements: Option<bool>,
        maintain: Option<bool>,
    ) -> Self {
        Capabilities {
            signal_backend: signal_backend.unwrap_or(false),
            reload_conf: reload_conf.unwrap_or(false),
            reset_statements: reset_statements.unwrap_or(false),
            maintain: maintain.unwrap_or(false),
        }
    }
}
```

Add the field to `ServerInfo`:

```rust
pub struct ServerInfo {
    pub version_num: i32,
    pub version_display: String,
    pub privilege: PrivilegeLevel,
    pub statements: StatementsAvailability,
    pub capabilities: Capabilities,
}
```

And extend `map_server_info`:

```rust
pub fn map_server_info(row: &Row) -> ServerInfo {
    let version_num: i32 = row.get("version_num");
    let is_monitor: bool = row.get("is_monitor");
    let is_superuser: bool = row.get("is_superuser");
    let statements_version: Option<String> = row.get("statements_version");
    let can_signal: Option<bool> = row.get("can_signal");
    let can_reload: Option<bool> = row.get("can_reload");
    let can_reset_statements: Option<bool> = row.get("can_reset_statements");
    let can_maintain: Option<bool> = row.get("can_maintain");
    ServerInfo {
        version_num,
        version_display: format_version(version_num),
        privilege: PrivilegeLevel::classify(is_superuser, is_monitor),
        statements: StatementsAvailability::classify(statements_version.as_deref()),
        capabilities: Capabilities::from_flags(
            can_signal,
            can_reload,
            can_reset_statements,
            can_maintain,
        ),
    }
}
```

- [ ] **Step 5: Run the unit tests to verify they pass**

Run: `cargo test --lib connection::probe`
Expected: PASS.

- [ ] **Step 6: Add the integration test**

Append to `tests/portability.rs`:

```rust
/// The four capability columns must run on every supported major. The two
/// guarded expressions are the point: `to_regprocedure` on a server without
/// pg_stat_statements, and the `pg_roles` subselect on a server without
/// `pg_maintain` (14 through 16). Either written naively raises, and a raising
/// probe fails the connection outright.
async fn assert_capability_probe_runs(tag: &str) {
    let (client, container) = connect(tag).await;

    let row = client
        .query_one(PROBE_SQL, &[])
        .await
        .expect("the probe must run on a stock server with no extension");
    let info = map_server_info(&row);
    assert!(
        info.capabilities.signal_backend,
        "postgres is a superuser and must be able to signal"
    );
    assert!(info.capabilities.reload_conf);
    assert!(
        !info.capabilities.reset_statements,
        "the extension is absent, so the reset function does not exist"
    );

    client
        .batch_execute("CREATE ROLE plain LOGIN PASSWORD 'plain'")
        .await
        .expect("failed to create the unprivileged role");
    let plain = connect_as(&container, "plain", "plain").await;
    let row = plain
        .query_one(PROBE_SQL, &[])
        .await
        .expect("the probe must run for an unprivileged role too");
    let info = map_server_info(&row);
    assert!(
        !info.capabilities.signal_backend,
        "a plain role may not signal other backends"
    );
    assert!(
        !info.capabilities.maintain,
        "a plain role holds no server-wide maintenance privilege"
    );
}

#[tokio::test]
async fn capability_probe_runs_on_postgres_14() {
    assert_capability_probe_runs("14").await;
}

#[tokio::test]
async fn capability_probe_runs_on_postgres_18() {
    assert_capability_probe_runs("18").await;
}
```

- [ ] **Step 7: Run the integration tests**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability capability_probe
```
Expected: PASS — 2 tests. PostgreSQL 14 is the one that proves the `pg_maintain` guard.

- [ ] **Step 8: Format and commit**

```bash
cargo fmt && cargo fmt --check
git add src/connection/probe.rs tests/portability.rs
git commit -m "feat: probe per-action capabilities at connect"
```

---

## Task 3: Per-table maintenance capability

**Files:**
- Modify: `src/collector/relations.rs:30-47` (`TABLES_SQL`), `:67-83` (`TableStats`), `:139-155` (`map_table_stats`)
- Modify: `src/collector/worker.rs:563-568` (`SlowTier`), `:607-624` (`sample`), `:661-675` (`sample_relations`), `:250-260` (the connect arm in `run`), `:456-465` (`sample_loop` signature)
- Modify: `tests/portability.rs` (the `TABLES_SQL` import and the two relations tests)
- Test: unit tests in `src/collector/relations.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1–2.
- Produces:
  - `crate::collector::relations::tables_sql(version_num: i32) -> String` (replaces the `TABLES_SQL` constant)
  - `TableStats::can_maintain: bool`
  - `TableStats::may_maintain(&self, server_wide: bool) -> bool`

- [ ] **Step 1: Write the failing unit tests**

Append to the `#[cfg(test)] mod tests` block at the foot of `src/collector/relations.rs` (create the block if the file has none, using the same shape as `src/connection/probe.rs`):

```rust
    #[test]
    fn postgres_17_and_later_ask_for_the_maintain_privilege() {
        for version in [170000, 180004] {
            let sql = tables_sql(version);
            assert!(
                sql.contains("has_table_privilege(current_user, c.oid, 'MAINTAIN')"),
                "{version} should use the MAINTAIN privilege"
            );
        }
    }

    #[test]
    fn postgres_16_and_earlier_fall_back_to_ownership() {
        // has_table_privilege raises "unrecognized privilege type" for
        // MAINTAIN before 17, and a raising slow-tier query costs the page.
        for version in [140011, 160002] {
            let sql = tables_sql(version);
            assert!(
                sql.contains("pg_has_role(current_user, c.relowner, 'MEMBER')"),
                "{version} should fall back to an ownership check"
            );
            assert!(
                !sql.contains("'MAINTAIN'"),
                "{version} must never mention a privilege it cannot parse"
            );
        }
    }

    #[test]
    fn every_version_still_selects_the_same_columns() {
        for version in [140011, 180004] {
            let sql = tables_sql(version);
            assert!(sql.contains("AS can_maintain"));
            assert!(sql.contains("pg_total_relation_size"));
            assert!(sql.ends_with("LIMIT $1"));
        }
    }

    #[test]
    fn a_server_wide_privilege_covers_a_table_the_role_does_not_own() {
        let table = table_stats_with(false);
        assert!(table.may_maintain(true));
        assert!(!table.may_maintain(false));
    }

    #[test]
    fn an_owned_table_is_maintainable_without_any_server_privilege() {
        // The common case for an application role: it owns its own tables and
        // holds nothing else. Greying the button out here is the failure this
        // whole column exists to avoid.
        let table = table_stats_with(true);
        assert!(table.may_maintain(false));
    }

    fn table_stats_with(can_maintain: bool) -> TableStats {
        TableStats {
            schema_name: "public".to_string(),
            table_name: "orders".to_string(),
            seq_scan: 0,
            seq_tup_read: 0,
            idx_scan: 0,
            idx_tup_fetch: 0,
            n_tup_ins: 0,
            n_tup_upd: 0,
            n_tup_del: 0,
            n_live_tup: 0,
            n_dead_tup: 0,
            secs_since_vacuum: None,
            total_bytes: 0,
            can_maintain,
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib collector::relations`
Expected: FAIL with `cannot find function 'tables_sql' in this scope`.

- [ ] **Step 3: Replace `TABLES_SQL` with `tables_sql`**

Replace `src/collector/relations.rs:30-47` with:

```rust
/// Whether the connected role may maintain a given table is a property of the
/// row, not of the connection: a table owner may VACUUM their own tables
/// holding no server-wide privilege at all.
///
/// This is the first query in the project that genuinely branches on server
/// version, which is what the parent spec §5 deferred `sql_for(version)` for.
/// `has_table_privilege(..., 'MAINTAIN')` raises *unrecognized privilege type*
/// before PostgreSQL 17, so 14 through 16 fall back to an ownership check —
/// which also covers superusers, who are members of every role.
const MAINTAIN_PRIVILEGE_VERSION: i32 = 170000;

/// Table statistics for the connected database. `pg_stat_user_tables` is
/// per-database; there is no server-wide equivalent.
///
/// `idx_scan` is NULL for a table with no indexes, and COALESCE to zero is
/// correct: a table with no indexes has had no index scans. `GREATEST` over
/// the two vacuum timestamps is NULL only when neither route has ever
/// vacuumed the table, which is itself the interesting answer.
pub fn tables_sql(version_num: i32) -> String {
    let can_maintain = if version_num >= MAINTAIN_PRIVILEGE_VERSION {
        "has_table_privilege(current_user, c.oid, 'MAINTAIN')"
    } else {
        "pg_has_role(current_user, c.relowner, 'MEMBER')"
    };

    format!(
        "\
SELECT t.schemaname::text AS schema_name,
       t.relname::text    AS table_name,
       t.seq_scan,
       t.seq_tup_read,
       COALESCE(t.idx_scan, 0)      AS idx_scan,
       COALESCE(t.idx_tup_fetch, 0) AS idx_tup_fetch,
       t.n_tup_ins,
       t.n_tup_upd,
       t.n_tup_del,
       t.n_live_tup,
       t.n_dead_tup,
       EXTRACT(EPOCH FROM (now() - GREATEST(t.last_vacuum, t.last_autovacuum)))::float8
         AS secs_since_vacuum,
       pg_total_relation_size(t.relid)::int8 AS total_bytes,
       {can_maintain} AS can_maintain
  FROM pg_stat_user_tables t
  JOIN pg_class c ON c.oid = t.relid
 ORDER BY total_bytes DESC
 LIMIT $1"
    )
}
```

- [ ] **Step 4: Add the field and the helper**

Add to `TableStats` after `total_bytes`:

```rust
    /// True when the connected role may maintain this specific table —
    /// through ownership, a granted MAINTAIN, or superuser.
    pub can_maintain: bool,
```

Add to `impl TableStats`:

```rust
    /// Whether maintenance may run on this table, combining the row's own
    /// answer with the connection's server-wide `pg_maintain` membership.
    pub fn may_maintain(&self, server_wide: bool) -> bool {
        self.can_maintain || server_wide
    }
```

Add to `map_table_stats`, after `total_bytes: row.get("total_bytes"),`:

```rust
        can_maintain: row.get("can_maintain"),
```

- [ ] **Step 5: Thread the version through the collector**

In `src/collector/worker.rs`, add a field to `SlowTier` (line 563):

```rust
struct SlowTier<'a> {
    statements_available: bool,
    statements_limit: i64,
    relations_limit: i64,
    version_num: i32,
    previous_statements: Option<(&'a HashMap<StatementKey, StatementCounters>, Instant)>,
}
```

Change `sample_relations` (line 661) to take the version and call the function:

```rust
async fn sample_relations(
    client: &Client,
    limit: i64,
    version_num: i32,
) -> Result<RelationsSample, CollectorError> {
    let rows = client
        .query(&tables_sql(version_num), &[&limit])
        .await
        .map_err(map_query_error)?;
    let tables = rows.iter().map(map_table_stats).collect();
```

Update its call site inside `sample` (line 620):

```rust
            let relations = Some(classify_slow(
                sample_relations(client, slow.relations_limit, slow.version_num).await,
            )?);
```

Update the import at the head of `worker.rs` — replace `TABLES_SQL` with `tables_sql`:

```rust
use crate::collector::relations::{
    map_index_stats, map_table_stats, tables_sql, RelationsSample, INDEXES_SQL,
};
```

Add a `version_num` parameter to `sample_loop` (line 456), immediately after `statements_available`:

```rust
async fn sample_loop(
    client: &Client,
    config: &CollectorConfig,
    history: &mut HistoryBackend,
    statements_available: bool,
    version_num: i32,
    events: &async_channel::Sender<CollectorEvent>,
    stop: &async_channel::Receiver<()>,
) -> Exit {
```

and set it when building the `SlowTier` inside that function:

```rust
                SlowTier {
                    statements_available,
                    statements_limit: config.statements_limit,
                    relations_limit: config.relations_limit,
                    version_num,
                    previous_statements: previous_statements
                        .as_ref()
                        .map(|(counters, at)| (counters, *at)),
                }
```

In `run`, capture the version before `info` is moved into the `Connected` event (line ~251):

```rust
            Ok((client, info)) => {
                let statements_available = info.statements.is_available();
                let version_num = info.version_num;
                if !emit(&events, &stop, CollectorEvent::Connected(info)).await {
                    return;
                }
```

and pass it at the `sample_loop` call (line ~318):

```rust
                match sample_loop(
                    &client,
                    &config,
                    &mut history,
                    statements_available,
                    version_num,
                    &events,
                    &stop,
                )
                .await
```

- [ ] **Step 6: Update the integration test**

In `tests/portability.rs`, change the import:

```rust
use mission_centre_pg::collector::relations::{
    map_index_stats, map_table_stats, tables_sql, INDEXES_SQL,
};
```

In `assert_relations_sql_runs`, replace the `TABLES_SQL` query with a version-aware one and assert the new column. Add this just after `let (client, _container) = connect(tag).await;`:

```rust
    let version_num: i32 = client
        .query_one("SELECT current_setting('server_version_num')::int", &[])
        .await
        .expect("failed to read the server version")
        .get(0);
    let tables_query = tables_sql(version_num);
```

then replace both `client.query(TABLES_SQL, &[&200i64])` occurrences with `client.query(&tables_query, &[&200i64])`, and add after the `total_bytes` assertion:

```rust
    assert!(
        orders.can_maintain,
        "postgres owns the table it created and must be able to maintain it"
    );
```

- [ ] **Step 7: Run everything**

```bash
cargo test --lib collector::relations
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability relations_sql
```
Expected: PASS. The 14 case proves the ownership fallback parses; the 18 case proves `MAINTAIN` does.

- [ ] **Step 8: Format and commit**

```bash
cargo fmt && cargo fmt --check
git add src/collector/relations.rs src/collector/worker.rs tests/portability.rs
git commit -m "feat: report per-table maintenance capability"
```

---

## Task 4: Action execution on its own connection

**Files:**
- Create: `src/collector/action_runner.rs`
- Modify: `src/collector/mod.rs`, `src/collector/worker.rs` (handle, events, `spawn`, `run`, `sample_loop`, `connect` visibility)
- Test: unit tests in `src/collector/worker_tests.rs`, integration test in `tests/portability.rs`

**Interfaces:**
- Consumes: `Action`, `ActionOutcome`, `signal_outcome`, `plan_for` from Task 1; `worker::connect` and `worker::map_query_error`.
- Produces:
  - `crate::collector::action_runner::run_action(params: &ConnectionParams, password: &str, action: &Action) -> ActionOutcome`
  - `CollectorHandle::submit(&self, action: Action) -> bool`
  - `CollectorEvent::ActionStarted(Action)`
  - `CollectorEvent::ActionFinished { action: Action, outcome: ActionOutcome }`

- [ ] **Step 1: Write the failing unit test**

Append to `src/collector/worker_tests.rs`:

```rust
#[test]
fn a_full_command_channel_refuses_rather_than_queues() {
    // Destructive actions must never pile up behind a wedged collector: a
    // terminate the user gave up on and re-clicked five times should not
    // arrive five times a minute later.
    let (tx, rx) = async_channel::bounded::<Action>(2);
    assert!(offer_command(&tx, Action::ReloadConfig));
    assert!(offer_command(&tx, Action::ReloadConfig));
    assert!(!offer_command(&tx, Action::ReloadConfig));
    drop(rx);
    assert!(!offer_command(&tx, Action::ReloadConfig));
}
```

Add to that file's imports at the top:

```rust
use mission_centre_pg::actions::Action;
```

(match the existing import style in `worker_tests.rs`; if it uses `use super::*;` then `Action` comes in through `worker.rs`'s own imports and no extra line is needed).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib collector::worker`
Expected: FAIL with `cannot find function 'offer_command' in this scope`.

- [ ] **Step 3: Create `src/collector/action_runner.rs`**

GPL header (first line `/* collector/action_runner.rs`), then:

```rust
use crate::actions::sql::plan_for;
use crate::actions::{signal_outcome, Action, ActionOutcome};
use crate::collector::worker::{connect, map_query_error};
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
```

Declare it in `src/collector/mod.rs`, keeping the list alphabetical:

```rust
pub mod action_runner;
```

- [ ] **Step 4: Widen two `worker.rs` items and add the events**

In `src/collector/worker.rs`, change `async fn connect(` (line ~365) to `pub(crate) async fn connect(`, and `pub(super) fn map_query_error(` (line 677) to `pub(crate) fn map_query_error(`.

Add to the imports at the head of the file:

```rust
use crate::actions::{Action, ActionOutcome};
use crate::collector::action_runner::run_action;
```

Extend `CollectorEvent`:

```rust
#[derive(Debug, Clone)]
pub enum CollectorEvent {
    Connecting,
    Connected(ServerInfo),
    Sample(Box<Snapshot>),
    Error(CollectorError),
    Disconnected,
    History(Box<HistoryPreload>),
    /// Emitted as the action's task starts, so a long-running maintenance
    /// action can post an in-flight notice rather than appearing to do
    /// nothing for minutes.
    ActionStarted(Action),
    ActionFinished {
        action: Action,
        outcome: ActionOutcome,
    },
}
```

- [ ] **Step 5: Add the command channel to the handle**

Replace `CollectorHandle` and its `impl` block (lines ~78-92):

```rust
pub struct CollectorHandle {
    pub events: async_channel::Receiver<CollectorEvent>,
    stop: async_channel::Sender<()>,
    commands: async_channel::Sender<Action>,
}

impl CollectorHandle {
    pub fn stop(&self) {
        let _ = self.stop.try_send(());
    }

    /// Offers an action to the collector. False means it was not accepted —
    /// the collector has gone, or too many actions are already queued — and
    /// the caller must say so rather than let the user believe it ran.
    pub fn submit(&self, action: Action) -> bool {
        offer_command(&self.commands, action)
    }
}

/// Non-blocking offer of one command. A full channel refuses rather than
/// queues: these are destructive actions, and a terminate the user gave up on
/// must not arrive a minute later.
fn offer_command(commands: &async_channel::Sender<Action>, action: Action) -> bool {
    commands.try_send(action).is_ok()
}
```

- [ ] **Step 6: Wire the channel through `spawn` and `run`**

In `spawn` (line ~240), add the channel and pass the receiver into `run`:

```rust
    let (event_tx, event_rx) = async_channel::bounded(32);
    let (stop_tx, stop_rx) = async_channel::bounded(1);
    // Eight is far more than a human clicking buttons can produce; a full
    // channel means something is wrong, and refusing is the right answer.
    let (command_tx, command_rx) = async_channel::bounded(8);

    std::thread::Builder::new()
        .name("mcpg-collector".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build the collector runtime");
            runtime.block_on(run(params, password, config, event_tx, stop_rx, command_rx));
        })
        .expect("failed to spawn the collector thread");

    CollectorHandle {
        events: event_rx,
        stop: stop_tx,
        commands: command_tx,
    }
```

Add the parameter to `run`:

```rust
async fn run(
    params: ConnectionParams,
    password: String,
    config: CollectorConfig,
    events: async_channel::Sender<CollectorEvent>,
    stop: async_channel::Receiver<()>,
    commands: async_channel::Receiver<Action>,
) {
```

and pass it, with the params and password, into `sample_loop`:

```rust
                match sample_loop(
                    &client,
                    &config,
                    &mut history,
                    statements_available,
                    version_num,
                    &params,
                    &password,
                    &events,
                    &stop,
                    &commands,
                )
                .await
```

- [ ] **Step 7: Drain commands during the inter-sample wait**

Extend `sample_loop`'s signature to match the call above:

```rust
#[allow(clippy::too_many_arguments)]
async fn sample_loop(
    client: &Client,
    config: &CollectorConfig,
    history: &mut HistoryBackend,
    statements_available: bool,
    version_num: i32,
    params: &ConnectionParams,
    password: &str,
    events: &async_channel::Sender<CollectorEvent>,
    stop: &async_channel::Receiver<()>,
    commands: &async_channel::Receiver<Action>,
) -> Exit {
```

Replace the `tokio::select!` at the foot of its loop:

```rust
        // The wait is a deadline rather than a sleep so that receiving an
        // action does not shorten the sampling interval: an action arriving
        // 100ms into a 2s wait must not trigger the next sample 100ms early.
        let deadline = tokio::time::Instant::now() + config.interval;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                _ = stop.recv() => return Exit::Stopped,
                command = commands.recv() => match command {
                    Ok(action) => spawn_action(params, password, action, events),
                    // The sender lives on `CollectorHandle`, so a closed
                    // channel means the handle has gone — the same condition
                    // `stop` reports.
                    Err(_) => return Exit::Stopped,
                },
            }
        }
```

Add the spawner beside `sample_loop`:

```rust
/// Runs one action off the sampling path entirely. The task owns clones of
/// everything it needs, so a VACUUM lasting minutes never touches the sample
/// loop; the loop keeps driving the runtime, so the task still progresses.
///
/// The task is bound to the runtime, which is bound to the collector: dropping
/// the handle cancels a running action. That is documented behaviour, and the
/// in-flight notice in the window is what makes it visible.
fn spawn_action(
    params: &ConnectionParams,
    password: &str,
    action: Action,
    events: &async_channel::Sender<CollectorEvent>,
) {
    let params = params.clone();
    let password = password.to_string();
    let events = events.clone();

    tokio::spawn(async move {
        let _ = events
            .send(CollectorEvent::ActionStarted(action.clone()))
            .await;
        let outcome = run_action(&params, &password, &action).await;
        let _ = events
            .send(CollectorEvent::ActionFinished { action, outcome })
            .await;
    });
}
```

- [ ] **Step 8: Run the unit tests**

Run: `cargo test --lib collector::`
Expected: PASS, including `a_full_command_channel_refuses_rather_than_queues`.

- [ ] **Step 9: Add the integration test**

Append to `tests/portability.rs`, with `use mission_centre_pg::actions::sql::plan_for;` and `use mission_centre_pg::actions::{Action, MaintenanceKind};` added to its imports:

```rust
/// Every action statement must actually run on both supported extremes. The
/// two that could plausibly break are the maintenance ones: `batch_execute`
/// carries them on the simple protocol because VACUUM cannot run inside the
/// implicit transaction the extended protocol opens.
async fn assert_action_statements_run(tag: &str) {
    let (client, _container) = connect(tag).await;
    client
        .batch_execute("CREATE TABLE orders (id bigserial PRIMARY KEY, note text)")
        .await
        .expect("failed to create the sample table");

    for kind in [
        MaintenanceKind::Analyze,
        MaintenanceKind::Vacuum,
        MaintenanceKind::VacuumAnalyze,
    ] {
        let plan = plan_for(&Action::Maintain {
            kind,
            schema: "public".to_string(),
            table: "orders".to_string(),
        });
        client
            .batch_execute(&plan.setup)
            .await
            .expect("the maintenance session settings must apply");
        client
            .batch_execute(&plan.sql)
            .await
            .unwrap_or_else(|e| panic!("{:?} failed: {e}", plan.sql));
    }

    let reload = plan_for(&Action::ReloadConfig);
    client
        .batch_execute(&reload.setup)
        .await
        .expect("the quick session settings must apply");
    client
        .execute(reload.sql.as_str(), &[])
        .await
        .expect("pg_reload_conf must run as superuser");

    // A backend that has already gone: the signal returns false rather than
    // raising, which is the NoSuchBackend case the UI reports honestly.
    let cancel = plan_for(&Action::CancelBackend { pid: 1 });
    let row = client
        .query_one(cancel.sql.as_str(), &[&cancel.pid.expect("a pid")])
        .await
        .expect("pg_cancel_backend must run");
    let signalled: bool = row.get(0);
    assert!(!signalled, "PID 1 is not a backend of this server");
}

#[tokio::test]
async fn action_statements_run_on_postgres_14() {
    assert_action_statements_run("14").await;
}

#[tokio::test]
async fn action_statements_run_on_postgres_18() {
    assert_action_statements_run("18").await;
}
```

- [ ] **Step 10: Run the integration tests**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability action_statements
```
Expected: PASS — 2 tests. A failure here reading `VACUUM cannot run inside a transaction block` means a maintenance plan lost its `batch: true`.

- [ ] **Step 11: Check the file size, format and commit**

```bash
wc -l src/collector/worker.rs   # expect well under 800
cargo fmt && cargo fmt --check
git add src/collector/action_runner.rs src/collector/mod.rs src/collector/worker.rs src/collector/worker_tests.rs tests/portability.rs
git commit -m "feat: run actions on a dedicated connection off the sampling path"
```

---

## Task 5: Table selection that survives the refresh

**Files:**
- Modify: `src/table/mod.rs:106-164` (`Table`, `attach`, `update`)
- Modify: `src/pages/sessions.rs:143-147`, `src/pages/relations.rs:258-268`, `src/pages/queries.rs:275-278` (the `attach` call sites)
- Test: unit tests in `src/table/mod.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1–4.
- Produces:
  - `crate::table::RowKey<T> = fn(&T) -> String`
  - `crate::table::reselect_index(keys: impl Iterator<Item = String>, previous: Option<&str>) -> Option<u32>`
  - `Table::attach(view, columns, matches, key)` — fourth parameter is new
  - `Table::selected(&self) -> Option<Rc<T>>`
  - `Table::connect_selection_changed(&self, f: impl Fn() + 'static)`

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block at the foot of `src/table/mod.rs`:

```rust
    #[test]
    fn a_row_still_present_is_reselected_at_its_new_index() {
        // The table re-sorts under the user every two seconds. Reselecting by
        // position would silently move the selection to a different backend.
        let keys = ["4822".to_string(), "4821".to_string(), "4823".to_string()];
        assert_eq!(reselect_index(keys.into_iter(), Some("4821")), Some(1));
    }

    #[test]
    fn a_row_that_has_gone_clears_the_selection() {
        let keys = ["4822".to_string(), "4823".to_string()];
        assert_eq!(reselect_index(keys.into_iter(), Some("4821")), None);
    }

    #[test]
    fn nothing_previously_selected_stays_nothing() {
        let keys = ["4821".to_string()];
        assert_eq!(reselect_index(keys.into_iter(), None), None);
    }

    #[test]
    fn an_empty_table_clears_the_selection() {
        assert_eq!(reselect_index(std::iter::empty(), Some("4821")), None);
    }

    #[test]
    fn the_first_match_wins() {
        let keys = ["a".to_string(), "b".to_string(), "b".to_string()];
        assert_eq!(reselect_index(keys.into_iter(), Some("b")), Some(1));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib table::`
Expected: FAIL with `cannot find function 'reselect_index' in this scope`.

- [ ] **Step 3: Add the key type and the pure function**

Insert after the `NumericKey` type alias at `src/table/mod.rs:37`:

```rust
/// A row's stable identity, used to re-establish the selection after the
/// two-second refresh replaces every row object in the store.
pub type RowKey<T> = fn(&T) -> String;

/// Where `previous` sits in the current view order, if it is still there.
///
/// The keys must come from the *view* — filtered and sorted — not from the
/// store: a store index is not a view index, and `SingleSelection` indexes
/// into the view.
pub fn reselect_index(keys: impl Iterator<Item = String>, previous: Option<&str>) -> Option<u32> {
    let previous = previous?;
    keys.enumerate()
        .find(|(_, key)| key == previous)
        .map(|(index, _)| index as u32)
}
```

- [ ] **Step 4: Give `Table` a selection**

Replace the `Table` struct and its `attach`/`update` (lines 106–163) with:

```rust
/// The store, filter, sorter and selection behind one `ColumnView`. The type
/// parameter keeps the API typed even though the underlying row object erases
/// it.
pub struct Table<T> {
    store: gio::ListStore,
    filter: gtk::CustomFilter,
    selection: gtk::SingleSelection,
    key: RowKey<T>,
    marker: PhantomData<T>,
}

impl<T: Clone + 'static> Table<T> {
    /// Builds the model, installs it on `view`, and appends one column per
    /// entry in `columns`. `matches` decides which rows the filter admits;
    /// it is re-evaluated on every `refilter()`. `key` identifies a row across
    /// refreshes so the user's selection survives them.
    pub fn attach(
        view: &gtk::ColumnView,
        columns: &[Column<T>],
        matches: impl Fn(&T) -> bool + 'static,
        key: RowKey<T>,
    ) -> Self {
        let store = gio::ListStore::new::<McpgRowObject>();

        let filter = gtk::CustomFilter::new(move |object| {
            let row = object
                .downcast_ref::<McpgRowObject>()
                .expect("the model only holds McpgRowObject")
                .row::<T>();
            matches(&row)
        });

        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        // Incremental filtering plus rapid items-changed is the combination
        // implicated in the upstream GTK sort/filter crash; keep it off.
        filtered.set_incremental(false);

        let sorted = gtk::SortListModel::new(Some(filtered), view.sorter());
        sorted.set_incremental(false);

        let selection = gtk::SingleSelection::new(Some(sorted));
        // Both default the wrong way for us. `autoselect` would force a row to
        // be selected at all times, so "nothing selected" — the state the
        // action buttons need on a fresh connection, or once the selected
        // backend exits — could not be represented at all.
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        view.set_model(Some(&selection));

        for column in columns {
            append_column(view, column);
        }

        Table {
            store,
            filter,
            selection,
            key,
            marker: PhantomData,
        }
    }

    /// Replaces the contents in one splice, keeping items-changed to a single
    /// emission per sample rather than one per row.
    ///
    /// The splice destroys the selection, which on a two-second sample cadence
    /// would mean the user could never keep a row selected long enough to act
    /// on it. The selected row's key is therefore captured first and looked up
    /// again afterwards.
    pub fn update(&self, rows: &[T]) {
        let previous = self.selected_key();
        let objects: Vec<McpgRowObject> = rows.iter().cloned().map(McpgRowObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objects);
        self.restore_selection(previous.as_deref());
    }

    pub fn refilter(&self) {
        self.filter.changed(gtk::FilterChange::Different);
    }

    /// The selected row, or `None` when nothing is selected or the previously
    /// selected row has gone.
    pub fn selected(&self) -> Option<Rc<T>> {
        self.selection
            .selected_item()
            .and_downcast::<McpgRowObject>()
            .map(|object| object.row::<T>())
    }

    /// Runs `f` whenever the selection changes, including when a refresh
    /// clears it because the row disappeared.
    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) {
        self.selection.connect_selected_item_notify(move |_| f());
    }

    fn selected_key(&self) -> Option<String> {
        self.selected().map(|row| (self.key)(row.as_ref()))
    }

    /// Keys in view order. Read from the selection model rather than the store
    /// because the view is filtered and sorted.
    fn view_keys(&self) -> Vec<String> {
        (0..self.selection.n_items())
            .filter_map(|index| self.selection.item(index))
            .filter_map(|object| object.downcast::<McpgRowObject>().ok())
            .map(|object| (self.key)(object.row::<T>().as_ref()))
            .collect()
    }

    fn restore_selection(&self, previous: Option<&str>) {
        match reselect_index(self.view_keys().into_iter(), previous) {
            Some(index) => self.selection.set_selected(index),
            None => self.selection.set_selected(gtk::INVALID_LIST_POSITION),
        }
    }
}
```

- [ ] **Step 5: Update the three call sites**

In `src/pages/sessions.rs`, add above the `mod imp` block:

```rust
fn session_key(session: &Session) -> String {
    session.pid.to_string()
}
```

and pass it in `constructed`:

```rust
            let table = Table::attach(
                &self.column_view.get(),
                COLUMNS,
                move |session| page.matches(session),
                session_key,
            );
```

In `src/pages/relations.rs`, add above `mod imp`:

```rust
fn table_key(table: &TableStats) -> String {
    format!("{}.{}", table.schema_name, table.table_name)
}

fn index_key(index: &IndexStats) -> String {
    format!("{}.{}.{}", index.schema_name, index.table_name, index.index_name)
}
```

and pass them at both `attach` calls:

```rust
            let tables = Table::attach(
                &self.tables_view.get(),
                TABLE_COLUMNS,
                move |table| page.table_matches(table),
                table_key,
            );
```

```rust
            let indexes = Table::attach(
                &self.indexes_view.get(),
                INDEX_COLUMNS,
                move |index| page.index_matches(index),
                index_key,
            );
```

In `src/pages/queries.rs`, add above `mod imp`:

```rust
/// Queries has no row action this phase, but the key is part of the shared
/// signature and selection costs nothing — a clicked row simply stays
/// highlighted through a refresh instead of flickering.
fn query_row_key(row: &QueryRow) -> String {
    format!("{:?}", row.statement.key)
}
```

and pass it:

```rust
            let table = Table::attach(
                &self.column_view.get(),
                COLUMNS,
                move |row| page.matches(row),
                query_row_key,
            );
```

- [ ] **Step 6: Run the tests and the compile check**

```bash
cargo test --lib table::
cargo check --all-targets
```
Expected: PASS, and a clean check with no unused-import warnings.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt && cargo fmt --check
git add src/table/mod.rs src/pages/sessions.rs src/pages/relations.rs src/pages/queries.rs
git commit -m "feat: single row selection that survives the sample refresh"
```

---

## Task 6: The sessions action bar

**Files:**
- Modify: `resources/ui/sessions_page.blp`
- Modify: `src/pages/sessions.rs` (imp fields, `constructed`, public methods)
- Test: manual — the page's logic is one pure predicate, tested in Task 8's window unit tests

**Interfaces:**
- Consumes: `Table::selected`, `Table::connect_selection_changed` (Task 5); `Capabilities` (Task 2).
- Produces:
  - `McpgSessionsPage::selected_session(&self) -> Option<Session>`
  - `McpgSessionsPage::connect_selection_changed(&self, f: impl Fn() + 'static)`
  - `McpgSessionsPage::set_capabilities(&self, capabilities: &Capabilities)`
  - Buttons bound to `win.cancel-backend` and `win.terminate-backend`

- [ ] **Step 1: Add the action bar to the Blueprint**

In `resources/ui/sessions_page.blp`, append inside the template, after the closing `}` of the `Gtk.ScrolledWindow` block:

```blueprint
  Gtk.Box {
    spacing: 6;
    margin-start: 12;
    margin-end: 12;
    margin-top: 6;
    margin-bottom: 12;

    Gtk.Label signal_reason {
      hexpand: true;
      xalign: 0;
      visible: false;
      wrap: true;
      styles ["dim-label", "caption"]
    }

    Gtk.Button cancel_backend_button {
      label: _("Cancel query");
      tooltip-text: _("Ask the selected backend to abandon its current query");
      action-name: "win.cancel-backend";
    }

    Gtk.Button terminate_backend_button {
      label: _("Terminate");
      tooltip-text: _("Close the selected backend's connection");
      action-name: "win.terminate-backend";
      styles ["destructive-action"]
    }
  }
```

- [ ] **Step 2: Build to compile the Blueprint**

Run: `ninja -C build`
Expected: builds with no Blueprint error. A typo here reports as `sessions_page.blp:NN: error:`.

- [ ] **Step 3: Add the template children and the public methods**

In `src/pages/sessions.rs`, add to the `imp::McpgSessionsPage` struct:

```rust
        #[template_child]
        pub signal_reason: TemplateChild<gtk::Label>,
        #[template_child]
        pub cancel_backend_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub terminate_backend_button: TemplateChild<gtk::Button>,
```

Add to the imports at the head of the file:

```rust
use crate::connection::probe::Capabilities;
```

Add these methods to `impl McpgSessionsPage`, after `set_privilege_limited`:

```rust
    /// The selected backend, or `None` when nothing is selected — including
    /// after a refresh in which the selected backend exited.
    pub fn selected_session(&self) -> Option<Session> {
        self.imp()
            .table
            .borrow()
            .as_ref()
            .and_then(|table| table.selected())
            .map(|row| (*row).clone())
    }

    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) {
        if let Some(table) = self.imp().table.borrow().as_ref() {
            table.connect_selection_changed(f);
        }
    }

    /// Shows why the buttons are unavailable when the role cannot signal.
    ///
    /// A label rather than only a tooltip: GTK does not deliver tooltips to
    /// insensitive widgets, so a tooltip alone would be invisible in exactly
    /// the case it exists for. The tooltip set in the Blueprint still serves
    /// the sensitive case.
    pub fn set_capabilities(&self, capabilities: &Capabilities) {
        let imp = self.imp();
        imp.signal_reason.set_visible(!capabilities.signal_backend);
        imp.signal_reason.set_text(&i18n(
            "Cancelling and terminating backends requires membership of pg_signal_backend.",
        ));
    }
```

Add `i18n` to the file's imports if it is not already there:

```rust
use crate::i18n::i18n;
```

- [ ] **Step 4: Compile and format**

```bash
cargo check --all-targets
cargo fmt && cargo fmt --check
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add resources/ui/sessions_page.blp src/pages/sessions.rs
git commit -m "feat: sessions action bar for cancel and terminate"
```

---

## Task 7: The tables action bar

**Files:**
- Modify: `resources/ui/relations_page.blp`
- Modify: `src/pages/relations.rs` (imp fields, public methods)
- Test: manual, as Task 6

**Interfaces:**
- Consumes: `Table::selected`, `Table::connect_selection_changed` (Task 5); `Capabilities` (Task 2); `TableStats::may_maintain` (Task 3).
- Produces:
  - `McpgRelationsPage::selected_table(&self) -> Option<TableStats>`
  - `McpgRelationsPage::connect_tables_selection_changed(&self, f: impl Fn() + 'static)`
  - `McpgRelationsPage::set_capabilities(&self, capabilities: &Capabilities)`
  - Buttons bound to `win.analyze-table`, `win.vacuum-table`, `win.vacuum-analyze-table`

- [ ] **Step 1: Add the action bar to the Blueprint**

In `resources/ui/relations_page.blp`, inside the `"tables"` `Adw.ViewStackPage`'s `Gtk.Box`, after the closing `}` of the `Gtk.ScrolledWindow` block:

```blueprint
        Gtk.Box {
          spacing: 6;
          margin-start: 12;
          margin-end: 12;
          margin-top: 6;
          margin-bottom: 12;

          Gtk.Label maintain_reason {
            hexpand: true;
            xalign: 0;
            visible: false;
            wrap: true;
            styles ["dim-label", "caption"]
          }

          Gtk.Button analyze_button {
            label: _("Analyse");
            tooltip-text: _("Refresh the planner's statistics for the selected table");
            action-name: "win.analyze-table";
          }

          Gtk.Button vacuum_button {
            label: _("Vacuum");
            tooltip-text: _("Reclaim space from dead tuples in the selected table");
            action-name: "win.vacuum-table";
          }

          Gtk.Button vacuum_analyze_button {
            label: _("Vacuum & analyse");
            tooltip-text: _("Reclaim space and refresh statistics in one pass");
            action-name: "win.vacuum-analyze-table";
          }
        }
```

- [ ] **Step 2: Build to compile the Blueprint**

Run: `ninja -C build`
Expected: builds cleanly.

- [ ] **Step 3: Add the template children and the public methods**

In `src/pages/relations.rs`, add to the `imp::McpgRelationsPage` struct:

```rust
        #[template_child]
        pub maintain_reason: TemplateChild<gtk::Label>,
        #[template_child]
        pub analyze_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub vacuum_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub vacuum_analyze_button: TemplateChild<gtk::Button>,
```

Add to the imports:

```rust
use crate::connection::probe::Capabilities;
use crate::i18n::i18n;
```

(`i18n_f` is already imported; add `i18n` to the same `use` line rather than duplicating it.)

Add to `impl McpgRelationsPage`, after `set_database`:

```rust
    /// The selected table, or `None` when nothing is selected — including
    /// after a refresh in which the table was dropped.
    pub fn selected_table(&self) -> Option<TableStats> {
        self.imp()
            .tables
            .borrow()
            .as_ref()
            .and_then(|table| table.selected())
            .map(|row| (*row).clone())
    }

    pub fn connect_tables_selection_changed(&self, f: impl Fn() + 'static) {
        if let Some(table) = self.imp().tables.borrow().as_ref() {
            table.connect_selection_changed(f);
        }
    }

    /// Shows why maintenance is unavailable when neither the connection nor
    /// the selected table grants it. See the note on
    /// `McpgSessionsPage::set_capabilities` for why this is a label and not
    /// only a tooltip.
    pub fn set_capabilities(&self, capabilities: &Capabilities) {
        let selected_allows = self
            .selected_table()
            .map(|table| table.may_maintain(capabilities.maintain))
            .unwrap_or(false);
        let imp = self.imp();
        imp.maintain_reason.set_visible(!selected_allows);
        imp.maintain_reason.set_text(&i18n(
            "Maintaining a table requires owning it, or membership of pg_maintain.",
        ));
    }
```

- [ ] **Step 4: Compile and format**

```bash
cargo check --all-targets
cargo fmt && cargo fmt --check
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add resources/ui/relations_page.blp src/pages/relations.rs
git commit -m "feat: tables action bar for vacuum and analyse"
```

---

## Task 8: Window actions, confirmation and toasts

**Files:**
- Create: `src/window_actions.rs`
- Modify: `resources/ui/window.blp`, `src/window.rs`, `src/main.rs:21-22`
- Test: unit tests in `src/window_actions.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–7.
- Produces:
  - `MissionCentrePgWindow::install_actions(&self)`
  - `MissionCentrePgWindow::update_action_enablement(&self)`
  - `MissionCentrePgWindow::handle_action_event(&self, event: CollectorEvent)` — called from `handle_event`'s two new arms
  - `crate::window_actions::{confirmation_body, outcome_message}` — pure, tested

- [ ] **Step 1: Add the toast overlay and the header menu**

In `resources/ui/window.blp`, wrap the split view:

```blueprint
  content: Adw.ToastOverlay toast_overlay {
    child: Adw.NavigationSplitView split_view {
```

and close it after the split view's closing brace, so the file ends:

```blueprint
    };
  };
}

menu server_actions_menu {
  section {
    item {
      label: _("Reload configuration");
      action: "win.reload-configuration";
    }

    item {
      label: _("Reset query statistics");
      action: "win.reset-statements";
    }
  }
}
```

Add the menu button to the content header bar, so that block reads:

```blueprint
        Adw.HeaderBar {
          title-widget: Adw.ViewSwitcher {
            stack: view_stack;
            policy: wide;
          };

          [end]
          Gtk.MenuButton {
            icon-name: "view-more-symbolic";
            tooltip-text: _("Server actions");
            menu-model: server_actions_menu;
          }
        }
```

- [ ] **Step 2: Build to compile the Blueprint**

Run: `ninja -C build`
Expected: builds cleanly.

- [ ] **Step 3: Write the failing tests for the pure helpers**

Create `src/window_actions.rs` with the GPL header (first line `/* window_actions.rs`), then:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use mission_centre_pg::actions::{Action, MaintenanceKind};
    use mission_centre_pg::collector::snapshot::Session;

    fn session() -> Session {
        Session {
            pid: 4821,
            user_name: Some("alice".to_string()),
            application_name: Some("orders-api".to_string()),
            client_addr: None,
            database: Some("prod".to_string()),
            state: Some("idle in transaction".to_string()),
            wait_event_type: None,
            wait_event: None,
            backend_type: Some("client backend".to_string()),
            query_duration_secs: Some(842.0),
            query: Some("UPDATE orders SET status = 'sent'".to_string()),
        }
    }

    #[test]
    fn a_confirmation_names_every_field_needed_to_identify_the_backend() {
        // The table re-sorts under the pointer every two seconds. A dialog
        // that does not name its target is how the wrong backend gets killed.
        let body = confirmation_body(&session());
        assert!(body.contains("4821"));
        assert!(body.contains("alice"));
        assert!(body.contains("prod"));
        assert!(body.contains("idle in transaction"));
        assert!(body.contains("UPDATE orders SET status = 'sent'"));
    }

    #[test]
    fn a_confirmation_survives_a_session_with_nothing_but_a_pid() {
        let bare = Session {
            user_name: None,
            application_name: None,
            database: None,
            state: None,
            query: None,
            query_duration_secs: None,
            ..session()
        };
        let body = confirmation_body(&bare);
        assert!(body.contains("4821"));
    }

    #[test]
    fn a_missing_backend_is_reported_as_neither_success_nor_failure() {
        let message = outcome_message(
            &Action::CancelBackend { pid: 4821 },
            &ActionOutcome::NoSuchBackend,
        );
        assert!(message.contains("4821"));
        assert!(message.contains("no longer running"));
    }

    #[test]
    fn a_maintenance_success_names_the_relation() {
        let message = outcome_message(
            &Action::Maintain {
                kind: MaintenanceKind::Vacuum,
                schema: "public".to_string(),
                table: "orders".to_string(),
            },
            &ActionOutcome::Succeeded,
        );
        assert!(message.contains("public.orders"));
    }

    #[test]
    fn a_failure_carries_the_servers_own_words() {
        let message = outcome_message(
            &Action::ReloadConfig,
            &ActionOutcome::Failed("permission denied for function pg_reload_conf".to_string()),
        );
        assert!(message.contains("permission denied for function pg_reload_conf"));
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test --bin mission-centre-pg`
Expected: FAIL — the module is not declared, then `cannot find function 'confirmation_body'`.

- [ ] **Step 5: Declare the module**

In `src/main.rs`:

```rust
mod application;
mod window;
mod window_actions;
```

- [ ] **Step 6: Implement `src/window_actions.rs`**

Insert above the test module:

```rust
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::gio;

use mission_centre_pg::actions::{Action, ActionOutcome, MaintenanceKind};
use mission_centre_pg::collector::snapshot::Session;
use mission_centre_pg::collector::worker::CollectorEvent;
use mission_centre_pg::connection::probe::Capabilities;
use mission_centre_pg::i18n::{i18n, i18n_f};

use crate::window::MissionCentrePgWindow;

const ACTION_CANCEL: &str = "cancel-backend";
const ACTION_TERMINATE: &str = "terminate-backend";
const ACTION_ANALYZE: &str = "analyze-table";
const ACTION_VACUUM: &str = "vacuum-table";
const ACTION_VACUUM_ANALYZE: &str = "vacuum-analyze-table";
const ACTION_RESET_STATEMENTS: &str = "reset-statements";
const ACTION_RELOAD_CONF: &str = "reload-configuration";

/// Everything needed to tell one backend apart from another that looks like
/// it. Fields the server withheld are simply omitted rather than rendered as
/// blanks, which would read as "this backend has no user".
pub fn confirmation_body(session: &Session) -> String {
    let mut lines = vec![i18n_f("PID {}", &[&session.pid.to_string()])];

    if let Some(user) = session.user_name.as_deref() {
        lines.push(i18n_f("User {}", &[user]));
    }
    if let Some(database) = session.database.as_deref() {
        lines.push(i18n_f("Database {}", &[database]));
    }
    if let Some(application) = session.application_name.as_deref() {
        lines.push(i18n_f("Application {}", &[application]));
    }
    if let Some(state) = session.state.as_deref() {
        lines.push(i18n_f("State {}", &[state]));
    }
    if let Some(secs) = session.query_duration_secs {
        lines.push(i18n_f("Running for {} seconds", &[&format!("{secs:.0}")]));
    }
    if let Some(query) = session.query.as_deref() {
        lines.push(String::new());
        lines.push(query.to_string());
    }

    lines.join("\n")
}

/// The toast text for a finished action.
pub fn outcome_message(action: &Action, outcome: &ActionOutcome) -> String {
    match outcome {
        ActionOutcome::Succeeded => match action {
            Action::CancelBackend { pid } => {
                i18n_f("Cancelled the query on backend {}.", &[&pid.to_string()])
            }
            Action::TerminateBackend { pid } => {
                i18n_f("Terminated backend {}.", &[&pid.to_string()])
            }
            Action::Maintain { kind, .. } => {
                let relation = action.target().unwrap_or_default();
                match kind {
                    MaintenanceKind::Analyze => i18n_f("Analysed {}.", &[&relation]),
                    MaintenanceKind::Vacuum => i18n_f("Vacuumed {}.", &[&relation]),
                    MaintenanceKind::VacuumAnalyze => {
                        i18n_f("Vacuumed and analysed {}.", &[&relation])
                    }
                }
            }
            Action::ResetStatements => i18n("Query statistics reset."),
            Action::ReloadConfig => i18n("Configuration reloaded."),
        },
        // Neither a success nor an error: the backend exited between the
        // sample that listed it and the signal.
        ActionOutcome::NoSuchBackend => i18n_f(
            "Backend {} was no longer running.",
            &[&action.target().unwrap_or_default()],
        ),
        ActionOutcome::Failed(message) => i18n_f("Action failed: {}", &[message]),
    }
}

/// The in-flight notice for a long-running action.
fn in_flight_message(action: &Action) -> String {
    i18n_f(
        "Running {} on {}…",
        &[
            match action {
                Action::Maintain {
                    kind: MaintenanceKind::Analyze,
                    ..
                } => "ANALYZE",
                Action::Maintain {
                    kind: MaintenanceKind::Vacuum,
                    ..
                } => "VACUUM",
                _ => "VACUUM (ANALYZE)",
            },
            &action.target().unwrap_or_default(),
        ],
    )
}

impl MissionCentrePgWindow {
    /// Registers the seven actions and connects the two selection sources that
    /// change their enablement. Called once, from `constructed`.
    pub fn install_actions(&self) {
        for name in [
            ACTION_CANCEL,
            ACTION_TERMINATE,
            ACTION_ANALYZE,
            ACTION_VACUUM,
            ACTION_VACUUM_ANALYZE,
            ACTION_RESET_STATEMENTS,
            ACTION_RELOAD_CONF,
        ] {
            let action = gio::SimpleAction::new(name, None);
            action.set_enabled(false);
            let window = self.clone();
            let name = name.to_string();
            action.connect_activate(move |_, _| window.activate_action_named(&name));
            self.add_action(&action);
        }

        let window = self.clone();
        self.imp()
            .sessions_page
            .connect_selection_changed(move || window.update_action_enablement());

        let window = self.clone();
        self.imp()
            .relations_page
            .connect_tables_selection_changed(move || window.update_action_enablement());
    }

    /// Builds the `Action` for a named GAction and either confirms it or
    /// submits it. A control whose target has gone between the click and here
    /// simply does nothing — the enablement pass that follows will disable it.
    fn activate_action_named(&self, name: &str) {
        if let Some(action) = self.action_for(name) {
            if action.requires_confirmation() {
                self.confirm_then(action);
            } else {
                self.submit_action(action);
            }
        }
    }

    /// Separate from `activate_action_named` so the selected-row lookups can
    /// use `?`: the activation path itself returns nothing.
    fn action_for(&self, name: &str) -> Option<Action> {
        let imp = self.imp();
        match name {
            ACTION_CANCEL => Some(Action::CancelBackend {
                pid: imp.sessions_page.selected_session()?.pid,
            }),
            ACTION_TERMINATE => Some(Action::TerminateBackend {
                pid: imp.sessions_page.selected_session()?.pid,
            }),
            ACTION_ANALYZE => self.maintenance_action(MaintenanceKind::Analyze),
            ACTION_VACUUM => self.maintenance_action(MaintenanceKind::Vacuum),
            ACTION_VACUUM_ANALYZE => self.maintenance_action(MaintenanceKind::VacuumAnalyze),
            ACTION_RESET_STATEMENTS => Some(Action::ResetStatements),
            ACTION_RELOAD_CONF => Some(Action::ReloadConfig),
            _ => None,
        }
    }

    fn maintenance_action(&self, kind: MaintenanceKind) -> Option<Action> {
        let table = self.imp().relations_page.selected_table()?;
        Some(Action::Maintain {
            kind,
            schema: table.schema_name,
            table: table.table_name,
        })
    }

    /// Presents a dialog naming the exact target, and submits only on the
    /// affirmative response.
    fn confirm_then(&self, action: Action) {
        let (heading, body, verb, destructive) = match &action {
            Action::CancelBackend { .. } => (
                i18n("Cancel this query?"),
                self.imp()
                    .sessions_page
                    .selected_session()
                    .map(|session| confirmation_body(&session))
                    .unwrap_or_default(),
                i18n("Cancel query"),
                false,
            ),
            Action::TerminateBackend { .. } => (
                i18n("Terminate this backend?"),
                self.imp()
                    .sessions_page
                    .selected_session()
                    .map(|session| confirmation_body(&session))
                    .unwrap_or_default(),
                i18n("Terminate"),
                true,
            ),
            _ => (
                i18n("Reset query statistics?"),
                i18n(
                    "Every statistic pg_stat_statements has accumulated since the last reset is discarded. This cannot be undone.",
                ),
                i18n("Reset"),
                true,
            ),
        };

        let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
        dialog.add_response("dismiss", &i18n("Dismiss"));
        dialog.add_response("confirm", &verb);
        if destructive {
            dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
        }
        dialog.set_default_response(Some("dismiss"));
        dialog.set_close_response("dismiss");

        let window = self.clone();
        dialog.connect_response(None, move |dialog, response| {
            dialog.close();
            if response == "confirm" {
                window.submit_action(action.clone());
            }
        });
        dialog.present(Some(self));
    }

    fn submit_action(&self, action: Action) {
        let accepted = self
            .imp()
            .collector
            .borrow()
            .as_ref()
            .map(|handle| handle.submit(action))
            .unwrap_or(false);

        if !accepted {
            self.add_toast_text(&i18n(
                "The action could not be sent — the connection is busy or has gone.",
            ));
        }
    }

    /// Recomputes all seven actions. Called on connect, disconnect, and every
    /// selection change — including the ones a refresh causes when the
    /// selected row disappears.
    pub fn update_action_enablement(&self) {
        let imp = self.imp();
        let connected = imp.connected.get();
        let capabilities = imp.capabilities.borrow().unwrap_or_default();

        let has_session = imp.sessions_page.selected_session().is_some();
        let can_signal = connected && capabilities.signal_backend && has_session;
        self.set_action_enabled(ACTION_CANCEL, can_signal);
        self.set_action_enabled(ACTION_TERMINATE, can_signal);

        let can_maintain = connected
            && imp
                .relations_page
                .selected_table()
                .map(|table| table.may_maintain(capabilities.maintain))
                .unwrap_or(false);
        self.set_action_enabled(ACTION_ANALYZE, can_maintain);
        self.set_action_enabled(ACTION_VACUUM, can_maintain);
        self.set_action_enabled(ACTION_VACUUM_ANALYZE, can_maintain);

        self.set_action_enabled(
            ACTION_RESET_STATEMENTS,
            connected && capabilities.reset_statements,
        );
        self.set_action_enabled(ACTION_RELOAD_CONF, connected && capabilities.reload_conf);

        imp.sessions_page.set_capabilities(&capabilities);
        imp.relations_page.set_capabilities(&capabilities);
    }

    fn set_action_enabled(&self, name: &str, enabled: bool) {
        if let Some(action) = self
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(enabled);
        }
    }

    /// Records the connection's capabilities and re-runs enablement.
    pub fn set_capabilities(&self, capabilities: Option<Capabilities>) {
        let imp = self.imp();
        imp.connected.set(capabilities.is_some());
        imp.capabilities.replace(capabilities);
        self.update_action_enablement();
    }

    /// The two action events. A long-running action posts a persistent notice
    /// when it starts, which the result dismisses.
    pub fn handle_action_event(&self, event: CollectorEvent) {
        match event {
            CollectorEvent::ActionStarted(action) if action.is_long_running() => {
                let toast = adw::Toast::new(&in_flight_message(&action));
                toast.set_timeout(0);
                self.imp().toast_overlay.add_toast(&toast);
                self.imp().in_flight_toast.replace(Some(toast));
            }
            CollectorEvent::ActionStarted(_) => {}
            CollectorEvent::ActionFinished { action, outcome } => {
                if let Some(toast) = self.imp().in_flight_toast.take() {
                    toast.dismiss();
                }
                self.add_toast_text(&outcome_message(&action, &outcome));
            }
            _ => {}
        }
    }

    fn add_toast_text(&self, text: &str) {
        self.imp().toast_overlay.add_toast(&adw::Toast::new(text));
    }
}
```

- [ ] **Step 7: Wire the window**

In `src/window.rs`, add the template child to `imp::MissionCentrePgWindow`:

```rust
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
```

and the three state fields, beside `connected_database`:

```rust
        /// What the connected role may do. `None` when nothing is connected,
        /// which is itself what disables every action.
        pub capabilities: RefCell<Option<Capabilities>>,
        pub connected: Cell<bool>,
        /// The persistent notice for a maintenance action still running.
        pub in_flight_toast: RefCell<Option<adw::Toast>>,
```

Add to the imports:

```rust
use mission_centre_pg::connection::probe::Capabilities;
```

Call `install_actions` at the foot of `constructed`, before `reload_servers`:

```rust
            self.obj().install_actions();
            self.obj().reload_servers();
```

In `select_server`, clear the capabilities alongside the other per-server state, immediately after `imp.below_floor_warning.replace(None);`:

```rust
        // Capabilities belong to the connection being left; leaving them set
        // would offer actions on a server that has not yet connected.
        self.set_capabilities(None);
```

In `handle_event`, set them on `Connected`, after the `set_database` call:

```rust
                self.set_capabilities(Some(info.capabilities));
```

(read `info.capabilities` before `info` is consumed — it is `Copy`, so the existing borrows are unaffected).

Clear them on `Disconnected` and on `Error`, inside those arms:

```rust
                self.set_capabilities(None);
```

Add the two new arms at the end of the `match`:

```rust
            CollectorEvent::ActionStarted(_) | CollectorEvent::ActionFinished { .. } => {
                self.handle_action_event(event);
            }
```

This requires `event` not to have been moved by the match. Bind the two arms by reference instead:

```rust
            event @ (CollectorEvent::ActionStarted(_) | CollectorEvent::ActionFinished { .. }) => {
                self.handle_action_event(event);
            }
```

Also call `self.update_action_enablement();` at the end of the `Sample` arm, so a refresh that dropped the selected row disables the buttons immediately rather than at the next selection change.

- [ ] **Step 8: Build, test and run**

```bash
cargo test --bin mission-centre-pg
cargo check --all-targets
ninja -C build
```
Expected: 5 unit tests pass; the build completes.

- [ ] **Step 9: Check file sizes, format and commit**

```bash
wc -l src/window.rs src/window_actions.rs   # both under ~800
cargo fmt && cargo fmt --check
git add resources/ui/window.blp src/window.rs src/window_actions.rs src/main.rs
git commit -m "feat: window actions, confirmation dialogs and result toasts"
```

---

## Task 9: Full verification

**Files:** none modified unless a check fails.

**Interfaces:**
- Consumes: everything.
- Produces: a verified Phase 4.

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
Expected: the largest file is under ~800 lines. If one is not, split it before continuing.

- [ ] **Step 3: Walk the success criteria against a live server**

Start a container with a superuser and a plain role:

```bash
podman run --rm -d --name mcpg-phase4 -e POSTGRES_PASSWORD=postgres -p 55432:5432 docker.io/library/postgres:18 \
  -c shared_preload_libraries=pg_stat_statements
sleep 3
podman exec -i mcpg-phase4 psql -U postgres -c "CREATE EXTENSION pg_stat_statements"
podman exec -i mcpg-phase4 psql -U postgres -c "CREATE ROLE app LOGIN PASSWORD 'app'"
podman exec -i mcpg-phase4 psql -U postgres -c "CREATE ROLE watcher LOGIN PASSWORD 'watcher' IN ROLE pg_monitor"
podman exec -i mcpg-phase4 psql -U app -h 127.0.0.1 -c "CREATE TABLE app_orders (id bigserial PRIMARY KEY, note text)"
```

Run the application:

```bash
MCPG_RESOURCE_DIR=$PWD/build/resources GSETTINGS_SCHEMA_DIR=$PWD/data ./build/src/mission-centre-pg
```

Check each, ticking them off:

- [ ] As `postgres` on `127.0.0.1:55432`: all seven controls become sensitive once a row is selected.
- [ ] As `watcher` (pg_monitor, no signal privilege): every action stays insensitive, and the sessions bar shows the pg_signal_backend reason.
- [ ] As `app`: `app_orders` is selectable and maintainable; no other table is, and the tables bar shows the reason when one of those is selected.
- [ ] Select a session, wait through five refreshes: the selection and the enabled buttons persist.
- [ ] Terminate a session opened from a second `psql`: the dialog names its PID, user, database and query; the row disappears on the next sample.
- [ ] Cancel a backend that has already exited: the toast reads "no longer running", not success and not an error.
- [ ] `VACUUM` a table large enough to take over a minute (`INSERT INTO app_orders SELECT g, 'n'||g FROM generate_series(1,20000000) g;` then delete half): the Overview graphs keep updating for its whole duration, an in-flight toast is visible, and a result toast replaces it.
- [ ] Switch servers mid-`VACUUM`: the in-flight toast goes; no crash. This is the documented §4.4 limitation.

- [ ] **Step 4: Tear down**

```bash
podman rm -f mcpg-phase4
```

- [ ] **Step 5: Commit any fixes and open the pull request**

```bash
git status --short          # expect clean unless a check needed a fix
git push -u origin phase-4-actions
gh pr create --title "Phase 4: actions and the privilege model" \
  --body "Implements docs/superpowers/specs/2026-07-25-phase-4-actions-design.md — cancel, terminate, VACUUM/ANALYZE, statistics reset and configuration reload, each gated on a per-action capability probe and executed on a connection of its own so a long VACUUM never stalls the sampler."
```

---

## Self-Review Notes

**Spec coverage.** §2.1 in-scope list → Tasks 1, 6, 7, 8. §2.2 out-of-scope → stated in Global Constraints. §3.1–3.2 capability probe → Task 2. §3.3 per-table capability and the version branch → Task 3. §3.4 gating rule → Tasks 6, 7, 8 (with the deviation recorded above). §4.1–4.3 execution, timeouts, protocols, quoting → Tasks 1 and 4. §4.4 lifetime limitation → Task 4 Step 7 comment, verified in Task 9. §5 selection → Task 5. §6.1–6.4 UI, enablement, confirmation, feedback → Tasks 6, 7, 8. §7 module layout → the File Structure table. §8 error handling → Task 4's `run_action`, Task 8's `submit_action` refusal path. §9 testing → the unit tests in Tasks 1, 2, 3, 4, 5, 8 and the container tests in Tasks 2, 3, 4. §10 success criteria → Task 9 Step 3, one tick per criterion.

**Type consistency.** `Capabilities` is defined once (Task 2) and consumed by the same field names in Tasks 3, 6, 7 and 8. `Action`, `MaintenanceKind` and `ActionOutcome` are defined in Task 1 and used unchanged thereafter. `Table::attach`'s fourth parameter is added in Task 5 and all three call sites are updated in the same task, so no intermediate state fails to compile. `tables_sql` replaces `TABLES_SQL` in Task 3, and every reader — `worker.rs` and `tests/portability.rs` — is updated in that same task.

**Compile trap worth knowing.** The natural way to write `activate_action_named` — one `match` using `?` on the selected-row lookups — does not compile, because the function returns `()`. Task 8 Step 6 therefore splits it into `activate_action_named` and `action_for`, and only the working split is shown.

# Mission Centre PostgreSQL — Phase 4 Design

**Author:** Paul Snow
**Date:** 2026-07-25
**Version:** 0.0.0
**Status:** Approved — ready for implementation planning
**Licence:** GPL-3.0-or-later
**Parent spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`

---

## 1. Summary

Phase 4 gives Mission Centre **actions**: the user can cancel a query, terminate a backend, run
`VACUUM` or `ANALYZE` on a table, reset the query statistics, and reload the server configuration —
each gated on the privileges the connected role actually holds.

Phases 1 and 2 only ever read. Phase 3 wrote, but only append-only history rows into a schema the
user opted into. Phase 4 is the first phase that **changes state the DBA cares about**, so the design
is dominated by three concerns: proving the role may perform an action before offering it, naming the
exact target before performing it, and keeping a long-running `VACUUM` from stalling the sampler.

---

## 2. Scope

### 2.1 In scope

Seven operations across three target kinds — maintenance is one action with three variants:

| Action | Target | Statement | Confirmed |
|---|---|---|---|
| Cancel query | selected session | `SELECT pg_cancel_backend($1)` | yes |
| Terminate backend | selected session | `SELECT pg_terminate_backend($1)` | yes |
| `ANALYZE` | selected table | `ANALYZE "s"."t"` | no |
| `VACUUM` | selected table | `VACUUM "s"."t"` | no |
| `VACUUM ANALYZE` | selected table | `VACUUM (ANALYZE) "s"."t"` | no |
| Reset query statistics | server | `SELECT pg_stat_statements_reset()` | yes |
| Reload configuration | server | `SELECT pg_reload_conf()` | no |

Supporting work this requires:

- A per-action **capability probe** at connect (§3), distinct from the existing `PrivilegeLevel`.
- **Row selection** in the shared table widget, surviving the two-second refresh (§5).
- An **action channel** into the collector thread and a dedicated connection per action (§4).
- Action bars, a header-bar menu, confirmation dialogs and result toasts (§6).

### 2.2 Explicitly out of scope

Recorded so the decisions are not silently relitigated:

- **`VACUUM FULL` and `REINDEX`.** Both take an `ACCESS EXCLUSIVE` lock for their whole duration, and
  `VACUUM FULL` rewrites the table, needing free disk space equal to its size. A monitoring tool
  offering them one click from a table row is offering an outage. They belong behind a deliberate,
  separately designed maintenance workflow, if at all.
- **`ALTER SYSTEM` and any configuration editing.** Phase 4 reloads what is already on disk; it does
  not write `postgresql.auto.conf`.
- **An action log or audit trail.** Feedback is a transient toast. Persisting who ran what and when
  is a real feature with its own storage and retention questions, and PostgreSQL's own logs already
  record the effects. Not needed to make the actions useful.
- **Bulk actions.** One selected row, one action. No multi-select, no "terminate all idle in
  transaction".
- **Reconnecting an action that outlived its connection.** See the limitation in §4.4.

---

## 3. The privilege model

### 3.1 Why `PrivilegeLevel` is not the authority

`probe.rs` already classifies the connection as `Superuser`, `Monitor` or `Limited`, and the parent
spec §6 says "the same probe result gates the Phase 4 action buttons". Taken literally that is wrong
in both directions:

- `pg_monitor` grants **no** right to signal a backend. A `Monitor` connection offered a working
  Terminate button would fail at the server every time.
- A deliberately built, non-superuser operations role with `pg_signal_backend` granted, or a role
  that simply **owns** the table it wants to `ANALYZE`, would see every button greyed out.

`PrivilegeLevel` keeps its existing job — deciding what the user can *see*, and whether the
window-level banner appears. Actions get their own answer.

### 3.2 The capability probe

`PROBE_SQL` gains four columns, evaluated once per connect on the existing probe round trip:

```sql
pg_has_role(current_user, 'pg_signal_backend', 'member')            AS can_signal,
has_function_privilege(current_user, 'pg_reload_conf()', 'execute') AS can_reload,
(SELECT has_function_privilege(current_user, p.oid, 'execute')
   FROM pg_proc p
  WHERE p.oid = to_regprocedure('pg_stat_statements_reset()'))      AS can_reset_statements,
COALESCE((SELECT pg_has_role(current_user, oid, 'member')
            FROM pg_roles WHERE rolname = 'pg_maintain'), false)    AS can_maintain
```

Every expression is written so it **cannot raise** on a server lacking the object it names. The probe
runs on every connect, and a probe that fails fails the connection:

- `to_regprocedure('pg_stat_statements_reset()')` returns `NULL` rather than raising when
  `pg_stat_statements` is not installed, so the subselect yields no row. It also names the zero-argument
  overload specifically, which exists in every extension version the project supports.
- The `pg_roles` subselect returns no row on PostgreSQL 14–16, where `pg_maintain` does not exist. A
  bare `pg_has_role(current_user, 'pg_maintain', 'member')` would raise *role "pg_maintain" does not
  exist* on three of the five supported majors.

Both `NULL` cases map to `false`.

Superusers need no special case: `pg_has_role` and `has_function_privilege` both return true for a
superuser, so the same four columns produce an all-true `Capabilities` without a branch.

```rust
pub struct Capabilities {
    pub signal_backend: bool,
    pub reload_conf: bool,
    pub reset_statements: bool,
    pub maintain: bool,
}
```

`Capabilities` hangs off `ServerInfo` beside `privilege` and `statements`, and travels to the UI on
the existing `CollectorEvent::Connected`.

### 3.3 Per-table maintenance is a row property

Whether the role may `VACUUM` a given table is not a property of the connection. A table owner may
maintain their own tables with no server-level privilege whatsoever, which is the common case for an
application role. `TABLES_SQL` therefore gains a `can_maintain` column, and `TableStats` a matching
field.

This is the first query in the project that genuinely branches on server version — the point the
parent spec §5 anticipated when it deferred `sql_for(version)` — because `has_table_privilege` raises
*unrecognized privilege type: "MAINTAIN"* before PostgreSQL 17:

| Version | Expression | Covers |
|---|---|---|
| 17+ | `has_table_privilege(current_user, c.oid, 'MAINTAIN')` | owner, `GRANT MAINTAIN`, `pg_maintain`, superuser |
| 14–16 | `pg_has_role(current_user, c.relowner, 'MEMBER')` | owner, superuser |

The alternative — using the ownership expression on every version and accepting a false negative for
a PostgreSQL 17 role holding a granted `MAINTAIN` — was rejected. Honesty about granted privileges is
the whole point of §3.2, and the branch is a single selected string in one function.

The effective per-row answer is `row.can_maintain || capabilities.maintain`.

### 3.4 Gating rule

A button is **disabled, never hidden**, when the capability is false, and carries a tooltip naming
the missing privilege — "Requires membership of pg_signal_backend", "Requires ownership of the table
or membership of pg_maintain". A hidden button teaches the user nothing; a disabled one with a reason
tells them exactly what to ask their DBA for.

---

## 4. Execution

### 4.1 Why not the sampler's connection

The collector thread owns the only `Client`, samples serially every two seconds, and sets
`statement_timeout = 5s`. `VACUUM` on a large table breaks all three assumptions at once: it holds
the connection for minutes, the graphs flatline while it runs, and the 5s timeout would have to be
raised and restored around every action. Three consecutive timed-out samples reach
`FAILURES_BEFORE_DISCONNECT`, so one `VACUUM` would cost the connection.

A second connection held open for the session was also rejected: it doubles idle connections against
`max_connections` for a feature used a handful of times an hour, and needs its own reconnect and
backoff logic duplicating the sampler's.

### 4.2 A dedicated connection per action

`CollectorHandle` gains a `commands` sender bounded at 8 and a `submit(Action)` method. `sample_loop`'s
inter-sample wait becomes a `sleep_until(deadline)` inside a `select!` that also drains `commands`,
looping until the deadline elapses — so submitting an action does not shorten the sampling interval,
and the sampler is never blocked waiting for one.

An accepted request spawns a task on the collector's runtime that opens its **own** connection from
the same `ConnectionParams` and password, runs one statement, emits a result, and closes.

```
collector thread (tokio)
├─ sample_loop ─────▶ client A   every 2s, statement_timeout 5s
└─ on ActionRequest:
     spawn task ────▶ client B   fresh connect, own timeouts
                      VACUUM (ANALYZE) "public"."orders"
                    ─▶ CollectorEvent::ActionFinished ─▶ toast
```

The cost is one transient connection and a connect round trip per action. Against a `VACUUM` that
runs for minutes, and against freezing the UI, that is cheap.

### 4.3 Statement construction and timeouts

| Class | `statement_timeout` | `lock_timeout` | Protocol |
|---|---|---|---|
| Signal, reset, reload | `5s` | default | extended (`query_one` / `execute`) |
| Maintenance | `0` | `30s` | simple (`batch_execute`) |

Maintenance runs without a statement timeout because a `VACUUM` may legitimately run for an hour, and
a timeout that fires mid-`VACUUM` wastes the work already done. It does carry a `lock_timeout`, so a
`VACUUM` blocked behind conflicting DDL reports a lock timeout rather than hanging invisibly.

**`VACUUM` cannot run inside a transaction block, and `tokio-postgres`'s `execute()` uses the extended
protocol, which wraps its statement in an implicit transaction.** Maintenance must therefore go
through `batch_execute()`, which uses the simple query protocol. `ANALYZE` is transaction-safe but
takes the same path for consistency.

`VACUUM` also cannot be parameterised, so its identifiers are quoted in Rust: each name is wrapped in
double quotes with any embedded double quote doubled. The names come from the catalogue rather than
from user input, but `CREATE TABLE "foo""; DROP TABLE bar --"` is legal PostgreSQL, so the quoting is
required, not decorative. `quote_ident` is a pure function with its own tests.

### 4.4 Lifetime

An in-flight action is bound to its collector. Switching servers or closing the window drops the
handle, which returns from `run`, which drops the runtime and cancels the spawned task — closing the
action's connection and aborting a running `VACUUM`.

This is accepted for Phase 4 rather than worked around. The persistent in-flight toast (§6.4) means
the user can see an action is still running before they switch away, so the outcome is visible rather
than silent. Detaching an action from its connection would mean tracking server-side PIDs and
polling `pg_stat_activity` to report on it, which is a larger feature than the actions themselves.

---

## 5. Selection

### 5.1 The problem

`Table::attach` installs `gtk::NoSelection`, and `Table::update` replaces the entire store with a
single `splice()` on every sample. Swapping in `gtk::SingleSelection` naively means **the selection is
destroyed every two seconds**, long before the user can move from the row to the action bar. Every
action in this phase except the two server-wide ones needs a selected row, so this is load-bearing,
not a detail.

### 5.2 Identity-based reselection

`Table` gains a key function, supplied by each page at attach time, and `update` re-establishes the
selection by identity after the splice:

```rust
Table::attach(view, columns, matches, key)
// sessions: pid   ·   relations: (schema, table)   ·   queries: queryid
```

The key is part of the shared signature, so every caller supplies one. Queries has no row action in
this phase and passes `queryid` anyway: selection there costs nothing and keeps the three column
views behaving alike, so a clicked row stays highlighted through a refresh instead of flickering.

`reselect_index(rows, previous_key) -> Option<u32>` is a pure function over the post-splice rows and
is where the tests live: the row is still present at a new index after a re-sort, the row has gone,
nothing was selected. When the target disappears — the backend exited, the table was dropped —
selection clears and the action buttons disable, which is the correct outcome rather than a failure
to explain.

Selection is read through the selection model, not the store: the model the user sees is filtered and
sorted, so a store index is not a view index.

---

## 6. User interface

### 6.1 Layout

- `window.blp` wraps its content in an `Adw.ToastOverlay`, and its header bar gains a `⋮`
  `Gtk.MenuButton` with **Reload configuration** and **Reset query statistics**.
- Sessions, and Tables & Indexes, each gain a bottom action bar below the column view. Queries needs
  no bar — its only action is server-wide and lives in the header menu.
- Actions are `GAction`s on the window, so the header menu, the action bars and any future keyboard
  accelerators drive one implementation.

```
Sessions
┌──────┬───────┬────────┬────────────────────────┐
│ PID  │ User  │ State  │ Query                  │
├──────┼───────┼────────┼────────────────────────┤
│ 4821 │ alice │ idle…  │ UPDATE orders SET …    │ ◀ selected
│ 4822 │ bob   │ active │ SELECT count(*) FROM … │
└──────┴───────┴────────┴────────────────────────┘
  [ Cancel query ]  [ Terminate ]
```

### 6.2 Enablement

A button is sensitive only when **a row is selected**, **the connection is live**, and **the
capability holds** (§3.4). Selecting a different row re-evaluates all three. Server-wide menu items
need only the last two.

### 6.3 Confirmation

Cancel, terminate and reset-statistics confirm; reload-configuration and maintenance do not. The line
is *affects another user's work, or destroys data that cannot be recovered*:

- Cancelling interrupts someone's query; terminating drops their connection outright.
- `pg_stat_statements_reset()` discards every statistic the server has accumulated since the last
  reset — unrecoverable, and easy to hit by accident from a menu.
- `pg_reload_conf()` is idempotent and loses nothing; `VACUUM` and `ANALYZE` are routine maintenance
  that the autovacuum daemon performs unprompted.

The dialog is an `Adw.AlertDialog` naming the **exact** target — PID, user, database, application and
the query text for a session; the qualified relation name for a table — because the table re-sorts
under the pointer every two seconds and "are you sure?" over an unnamed target is how the wrong
backend gets killed. Terminate carries the `destructive-action` style class.

### 6.4 Feedback

Results are `Adw.Toast`s on the window overlay. Maintenance additionally posts a persistent
(`timeout 0`) "Running VACUUM on public.orders…" toast when it starts, dismissed when the result
arrives.

`pg_cancel_backend` and `pg_terminate_backend` return `false` when the PID is already gone. That is
neither a success nor an error and is reported as neither: "Backend 4821 was no longer running."

```rust
pub enum ActionOutcome {
    Succeeded,
    NoSuchBackend,
    Failed(String),
}
```

All user-facing strings go through the existing `i18n` wrappers.

---

## 7. Module layout

```
src/actions/
  mod.rs        Action, MaintenanceKind, ActionOutcome, requires_confirmation()
  sql.rs        quote_ident, sql_for(&Action), timeout policy

src/connection/probe.rs        + Capabilities, four probe columns
src/collector/worker.rs        + command channel, run_action task, ActionFinished event
src/collector/relations.rs     + can_maintain, version-selected expression
src/table/mod.rs               SingleSelection, key function, reselect_index
src/pages/sessions.rs          action bar, cancel and terminate
src/pages/relations.rs         action bar, maintenance split button
src/pages/queries.rs           capability plumbing for the reset menu item
src/window.rs                  GActions, confirmation dialogs, toast overlay
resources/ui/*.blp             toast overlay, header menu, action bars
```

`window.rs` is already 492 lines and will grow; if it passes roughly 800, the action wiring moves to
`src/window_actions.rs` rather than the file being allowed to sprawl.

---

## 8. Error handling

| Failure | Behaviour |
|---|---|
| Action connection cannot be opened | `Failed` toast carrying the connect error. The sampler is untouched. |
| Server rejects on privilege grounds | `Failed` toast with the server's own message. Should be rare given §3, but the probe can go stale — a role revoked mid-session — so the error path is never assumed unreachable. |
| Signal function returns `false` | `NoSuchBackend` toast. Not an error. |
| `lock_timeout` fires during maintenance | `Failed` toast naming the lock timeout, suggesting a retry. |
| Command channel full | Request dropped with a toast. The bound is generous relative to a human clicking buttons; a full channel means something is wrong, and silently queueing destructive actions would be worse. |
| Connection lost while an action is in flight | The task is cancelled with the runtime (§4.4); the in-flight toast is dismissed on disconnect. |

No action failure may ever fail a sample or disconnect the collector. The action path and the
sampling path share a thread and a runtime, and nothing else.

---

## 9. Testing

Following phases 1–3, everything decidable is a pure function tested in-module:

- `quote_ident` — plain names, embedded double quotes, mixed case, a name containing `;`.
- `sql_for(&Action)` — every variant, including the exact `VACUUM (ANALYZE)` spelling.
- `requires_confirmation` — the three confirmed actions and the three unconfirmed ones.
- `Capabilities` classification from probe flags, including `NULL` for an absent extension and an
  absent `pg_maintain`.
- The version-selected `can_maintain` expression — 14, 16, 17 and 18 boundaries.
- Effective per-row maintenance gating — owner without server privilege, `pg_maintain` without
  ownership, neither.
- `reselect_index` — present at a new index, absent, nothing previously selected, empty rows.
- `ActionOutcome` classification — `true`, `false`, and a server error.

**Not tested automatically:** the action bars, the confirmation dialogs and the toasts, as with every
phase's GTK layer. Verified by running the application against a live server.

---

## 10. Success criteria

1. Connecting as a superuser enables every action; connecting as a `pg_monitor`-only role leaves
   every action disabled with a tooltip naming the missing privilege.
2. Connecting as a non-superuser role granted `pg_signal_backend` enables cancel and terminate and
   nothing else.
3. Connecting as a plain role that owns a table enables maintenance on that table and leaves it
   disabled on tables it does not own.
4. Selecting a session and waiting through several sample refreshes leaves the selection and the
   enabled buttons intact; the selection clears when the backend exits.
5. Terminating a backend names that backend's PID, user, database and query in the dialog, and the
   session disappears from the table on the next sample.
6. Cancelling a backend that has already exited reports "no longer running", not success and not an
   error.
7. A `VACUUM` on a table large enough to take over a minute leaves the Overview graphs updating
   normally throughout, and reports its result when it completes.
8. Connecting to PostgreSQL 14 and to 18 both probe cleanly, with no error from the `pg_maintain` or
   `pg_stat_statements_reset` columns on either.
9. `cargo fmt` produces no diff; unit and integration tests pass on 14 and 18; no source file exceeds
   roughly 800 lines.

---

## 11. Open questions

None blocking. Deferred decisions are recorded in §2.2 and §4.4.

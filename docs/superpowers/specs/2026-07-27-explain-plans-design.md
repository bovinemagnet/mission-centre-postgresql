# Mission Centre PostgreSQL — Explain Plans Design

**Author:** Paul Snow
**Date:** 2026-07-27
**Version:** 0.0.0
**Status:** Approved — ready for implementation planning
**Licence:** GPL-3.0-or-later
**Parent spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`
**Issue:** #5 — Right-click analyse query

---

## 1. Summary

A **Plan** page, sitting after Tables & Indexes, showing the execution plan for a
statement chosen on the Queries page. The plan is presented twice over: as the
server's own text, and as a tree of nodes carrying each one's cost, row estimate
and width.

The feature is opened by right-clicking a query. It is a point-in-time snapshot,
not a live view: a plan describes the moment it was asked for, and refreshing it
on the sampling tick would imply a currency it does not have.

### 1.1 The constraint that shapes everything

`pg_stat_statements` stores **normalised** queries, with literals replaced by
`$1`, `$2` and so on. Such a query cannot be planned by asking the server to
explain it directly — there is no parameter to bind. Verified against containers
on 2026-07-27:

| Version | `EXPLAIN <normalised query>` | `EXPLAIN (GENERIC_PLAN) …` |
|---|---|---|
| 14 | `ERROR: there is no parameter $1` | `ERROR: there is no parameter $1` |
| 15 | `ERROR: there is no parameter $1` | `ERROR: there is no parameter $1` |
| 16 | `ERROR: there is no parameter $1` | plan returned |
| 18 | `ERROR: there is no parameter $1` | plan returned |

`GENERIC_PLAN` exists precisely for this case and arrived in PostgreSQL 16. The
project floor is 14, so the feature is unavailable on 14 and 15 — not through
any choice of ours, and the page says so rather than appearing broken.

---

## 2. Scope

### 2.1 In scope

- A **Plan** page after Tables & Indexes, with a text view and a node-tree view.
- A **right-click menu** on a Queries row, with an *Explain query* item.
- One `EXPLAIN (GENERIC_PLAN, FORMAT JSON)` per request, on a connection of its
  own, with a statement timeout.
- The node tree parsed from the JSON plan: node type, relation, startup and total
  cost, estimated rows, width, and nesting.
- The originating query text and the time the plan was taken, shown with it.

### 2.2 Explicitly out of scope

Recorded so the decisions are not silently relitigated:

- **`EXPLAIN ANALYZE`.** It *executes* the statement. These are arbitrary
  captured queries, which may be `DELETE`, `UPDATE` or a call into a function
  with side effects. A monitoring tool offering that behind a right-click is
  offering an outage — the same reasoning that kept `VACUUM FULL` out of Phase 4.
  Actual run-time figures need a deliberate workflow with a query the user has
  chosen and understood, if ever.
- **Editing the query before explaining it.** Substituting real literals for
  `$1` would make the feature work on 14 and 15, and would give better plans
  everywhere, since a generic plan is not necessarily the plan a real execution
  gets. It also introduces a user-supplied SQL path into a monitoring tool, and
  the question of what happens when the edited text is no longer the statement
  being investigated. Worth its own design, not a clause in this one.
- **Plan history.** No storing of plans, no comparison between two plans of the
  same statement over time. Genuinely useful; a subsystem of its own.
- **A drawn box-and-arrow diagram.** The node tree carries the structure and the
  numbers; a custom drawing widget with layout, hit-testing and scrolling is a
  large piece of work for a presentational gain. The tree view is the diagram for
  now.
- **Plans for sessions.** Only Queries rows are explicable. A running session's
  query text is not normalised and could in principle be explained directly, but
  it is a moving target — by the time the user right-clicks, the backend is
  usually doing something else.

---

## 3. Obtaining the plan

### 3.1 The statement

    EXPLAIN (GENERIC_PLAN, FORMAT JSON) <the statement text, verbatim>

The statement text is taken from the Queries row and interpolated **as SQL, not
as a parameter** — `EXPLAIN` takes a statement, not a string, so there is no
placeholder form available. This is the one place in the application where text
from the server is composed into a statement, and it is worth being explicit
about why that is acceptable here: the text originates from `pg_stat_statements`
on the same server, it is normalised so it carries no literals, and `EXPLAIN`
without `ANALYZE` plans without executing. The request is refused before it is
sent if the text contains a semicolon followed by anything other than trailing
whitespace, so a single request cannot become two statements.

### 3.2 Where it runs

On a **connection of its own**, opened for the request and closed after it, in
the manner of Phase 4 actions (§4.1 of the Phase 4 design). Two reasons: an
`EXPLAIN` against a large catalogue can take longer than a sampling interval, and
the sampler must not be behind it; and a failed or slow request must not count
against the connection's failure budget.

`SET statement_timeout = '5s'` applies, matching the sampler's guard.

### 3.3 How the result arrives

A new collector event carries the outcome back to the window, in the way
`ActionFinished` already does. The event carries either the JSON text or the
error, together with the key of the statement it belongs to, so a result arriving
after the user has selected a different query can be discarded rather than shown
against the wrong statement.

---

## 4. The plan model

`FORMAT JSON` returns a single row containing an array with one object, whose
`Plan` member is the root node. Each node carries `Node Type`, optional
`Relation Name`, `Startup Cost`, `Total Cost`, `Plan Rows`, `Plan Width`, and an
optional `Plans` array of children.

The parse produces:

    PlanNode {
        node_type: String,
        relation: Option<String>,
        startup_cost: f64,
        total_cost: f64,
        rows: i64,
        width: i64,
        children: Vec<PlanNode>,
    }

Parsing is a pure function over a JSON string, so every shape that matters —
a single node, nested children, a missing optional field, a malformed document —
is testable without a database.

The tree is flattened for display exactly as the Phase 5 blocked tree is: depth
first, with an indent per level. That machinery is proven and needs no second
implementation.

---

## 5. The page

Two views, switched as the Locks page switches between its tree and inventory:

- **Text** — the server's own rendering, obtained from the same JSON by walking
  the tree, so a second round trip is not needed. Monospaced, selectable, and
  scrollable.
- **Tree** — one row per node: node type (indented by depth), relation, total
  cost, estimated rows, width.

Above both: the statement being explained, collapsed onto one line, and when the
plan was taken.

The page has four states, and each says something different:

| State | Shown when |
|---|---|
| Empty | Nothing has been explained yet — names the right-click that starts it |
| Unsupported | The server is older than 16 — names the version required |
| Failed | The server refused the statement — carries its message verbatim |
| Plan | A plan was returned |

---

## 6. Triggering

A right-click anywhere on a Queries row opens a menu with a single item,
*Explain query*. The item is insensitive, with the reason stated, when the server
is older than 16.

Right-clicking also selects the row under the pointer, so the menu cannot act on
a different statement from the one the user pointed at.

The Plan page is not switched to automatically. The user asked for a plan, not
for their current page to be taken away; the page fills in and they navigate to
it when ready. A toast confirms the plan is ready, in the manner of Phase 4's
result toasts.

---

## 7. Error handling

The three failure states of the Phase 5 design apply unchanged: **unsupported**
names the version, **not permitted** names the privilege, **failed** carries the
server's own message. A plan is a diagnostic tool; a generic failure in a
diagnostic tool is worse than useless.

The most likely real failure is a statement referring to a table the connected
role cannot see, or one that has since been dropped. Both come back as ordinary
PostgreSQL errors and are shown as such.

---

## 8. Testing

**Unit, without a database:** the JSON parse against a single node, a nested
plan, a plan with a missing optional field, and a malformed document; the
flattening into rows with depths; the semicolon refusal; the version gate.

**Portability, on 14, 15, 16 and 18:** that `GENERIC_PLAN` is refused on 14 and
15 and accepted on 16 and 18 — the table in §1.1 asserted rather than trusted,
including 15, since that is the boundary. A real normalised statement is taken
from `pg_stat_statements` and explained, so the round trip is proven end to end
rather than against a hand-written query.

**Live:** the right-click path, and the plan of a genuine multi-node query.

---

## 9. Success criteria

1. Right-clicking a Queries row on PostgreSQL 16 or later offers *Explain query*.
2. Choosing it produces a plan on the Plan page within a few seconds, without the
   Overview graphs pausing.
3. The plan's node tree nests: a join shows its inputs as children.
4. Each node shows its total cost, estimated rows and width.
5. The text view shows the same plan in the server's own words.
6. The statement being explained is shown with the plan, and matches the row that
   was right-clicked.
7. On PostgreSQL 14 or 15 the menu item is insensitive and states that
   PostgreSQL 16 or later is required.
8. A statement referring to a dropped table reports the server's error, not a
   generic failure.
9. Explaining a second statement replaces the first, and a result arriving for a
   statement no longer selected is discarded rather than shown.

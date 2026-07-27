# Explain Plans Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Right-click a query on the Queries page and see its execution plan, as the server's own text and as a tree of costed nodes, on a new Plan page.

**Architecture:** One `EXPLAIN (GENERIC_PLAN, FORMAT JSON)` runs on a connection of its own and returns JSON. A pure function parses that into a node tree, which is flattened for display exactly as the Phase 5 blocked tree is. The result travels back to the window as a collector event carrying the statement key it belongs to, so a late result cannot be shown against the wrong query.

**Tech Stack:** Rust, GTK4 + libadwaita via gtk-rs, Blueprint (`.blp`), `tokio-postgres`, `serde_json` (already a dependency), Meson + Ninja, `testcontainers`.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-27-explain-plans-design.md`. Issue #5.
- **Author:** Paul Snow. **Version:** 0.0.0. GPL-3.0-or-later header on every new file, copied from `src/pages/locks.rs:1-19`.
- **British spelling** in comments, documentation and user-visible strings.
- **PostgreSQL 14 is the floor**, but this feature requires **16**. Verified: `GENERIC_PLAN` is refused on 14 and 15, accepted on 16 and 18.
- **`EXPLAIN ANALYZE` is never emitted.** Not as an option, not behind a flag. It executes the statement.
- **User-visible strings** go through `crate::i18n::i18n`.
- **TDD:** the failing test first, run and seen to fail, before implementation.
- **Commands:** `cargo test --lib`, `cargo test --bin mission-centre-pg`, `ninja -C build`; portability needs `export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"`.
- **Verify the UI with `tools/uicheck.py digest`** rather than asking a human what is on screen.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/explain/mod.rs` | Create — plan model, JSON parse, text rendering, the safety refusal, unit tests |
| `src/pages/plan.rs` | Create — the Plan page, two views, four states |
| `resources/ui/plan_page.blp` | Create — layout |
| `src/collector/worker.rs` | Modify — an explain request channel and its event |
| `src/pages/queries.rs` | Modify — right-click menu on a row |
| `src/window.rs`, `src/window_actions.rs` | Modify — the action, routing the result |
| `resources/ui/window.blp`, `resources/ui/queries_page.blp` | Modify — the page and the menu |
| `resources/meson.build`, `resources/mission-centre-pg.gresource.xml` | Modify — register the layout |
| `src/lib.rs` | Modify — declare the module |
| `tests/portability.rs` | Modify — the version boundary on 14, 15, 16, 18, and a real round trip |

---

## Task 1: The plan model and its parser

**Files:**
- Create: `src/explain/mod.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `PlanNode { node_type: String, relation: Option<String>, startup_cost: f64, total_cost: f64, rows: i64, width: i64, children: Vec<PlanNode> }`; `pub fn parse_plan(json: &str) -> Result<PlanNode, ExplainError>`; `pub enum ExplainError { Malformed(String) }`.

- [ ] **Step 1: Create the module with the type and a stubbed parser**

Create `src/explain/mod.rs` with the GPL header, then:

```rust
use serde_json::Value;

/// One node of an execution plan, with the estimates the planner attached.
/// Actual figures are deliberately absent: obtaining them needs EXPLAIN
/// ANALYZE, which executes the statement (spec §2.2).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanNode {
    pub node_type: String,
    pub relation: Option<String>,
    pub startup_cost: f64,
    pub total_cost: f64,
    pub rows: i64,
    pub width: i64,
    pub children: Vec<PlanNode>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExplainError {
    #[error("The server returned a plan this version cannot read: {0}")]
    Malformed(String),
}

pub fn parse_plan(_json: &str) -> Result<PlanNode, ExplainError> {
    Err(ExplainError::Malformed("not implemented".into()))
}
```

Declare `pub mod explain;` in `src/lib.rs` alongside the other modules.

- [ ] **Step 2: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE: &str = r#"[{"Plan":{"Node Type":"Seq Scan","Relation Name":"orders",
        "Startup Cost":0.00,"Total Cost":18.10,"Plan Rows":810,"Plan Width":36}}]"#;

    const NESTED: &str = r#"[{"Plan":{"Node Type":"Nested Loop",
        "Startup Cost":0.29,"Total Cost":24.36,"Plan Rows":5,"Plan Width":72,
        "Plans":[
            {"Node Type":"Seq Scan","Relation Name":"orders",
             "Startup Cost":0.00,"Total Cost":18.10,"Plan Rows":5,"Plan Width":36},
            {"Node Type":"Index Scan","Relation Name":"customers",
             "Startup Cost":0.29,"Total Cost":1.25,"Plan Rows":1,"Plan Width":36}]}}]"#;

    #[test]
    fn a_single_node_plan_parses() {
        let plan = parse_plan(SINGLE).expect("a well formed plan parses");
        assert_eq!(plan.node_type, "Seq Scan");
        assert_eq!(plan.relation.as_deref(), Some("orders"));
        assert_eq!(plan.total_cost, 18.10);
        assert_eq!(plan.rows, 810);
        assert_eq!(plan.width, 36);
        assert!(plan.children.is_empty());
    }

    #[test]
    fn children_are_nested_under_their_parent() {
        let plan = parse_plan(NESTED).expect("a nested plan parses");
        assert_eq!(plan.node_type, "Nested Loop");
        assert_eq!(plan.children.len(), 2);
        assert_eq!(plan.children[0].relation.as_deref(), Some("orders"));
        assert_eq!(plan.children[1].node_type, "Index Scan");
    }

    #[test]
    fn a_node_without_a_relation_is_not_an_error() {
        // Aggregates, sorts and gathers have no relation of their own.
        let json = r#"[{"Plan":{"Node Type":"Aggregate","Startup Cost":1.0,
            "Total Cost":2.0,"Plan Rows":1,"Plan Width":8}}]"#;
        let plan = parse_plan(json).expect("a relationless node parses");
        assert_eq!(plan.relation, None);
    }

    #[test]
    fn malformed_json_is_reported_rather_than_panicking() {
        assert!(matches!(parse_plan("not json"), Err(ExplainError::Malformed(_))));
        assert!(matches!(parse_plan("[]"), Err(ExplainError::Malformed(_))));
        assert!(matches!(parse_plan(r#"[{"NoPlan":1}]"#), Err(ExplainError::Malformed(_))));
    }
}
```

- [ ] **Step 3: Run them and watch them fail**

```bash
cargo test --lib explain
```

Expected: the four tests fail, the stub returning `Malformed("not implemented")`.

- [ ] **Step 4: Implement the parser**

```rust
pub fn parse_plan(json: &str) -> Result<PlanNode, ExplainError> {
    let document: Value =
        serde_json::from_str(json).map_err(|e| ExplainError::Malformed(e.to_string()))?;

    let root = document
        .get(0)
        .and_then(|entry| entry.get("Plan"))
        .ok_or_else(|| ExplainError::Malformed("no Plan member".into()))?;

    Ok(node_from(root))
}

fn node_from(value: &Value) -> PlanNode {
    PlanNode {
        node_type: value
            .get("Node Type")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string(),
        relation: value
            .get("Relation Name")
            .and_then(Value::as_str)
            .map(str::to_string),
        startup_cost: value.get("Startup Cost").and_then(Value::as_f64).unwrap_or(0.0),
        total_cost: value.get("Total Cost").and_then(Value::as_f64).unwrap_or(0.0),
        rows: value.get("Plan Rows").and_then(Value::as_i64).unwrap_or(0),
        width: value.get("Plan Width").and_then(Value::as_i64).unwrap_or(0),
        children: value
            .get("Plans")
            .and_then(Value::as_array)
            .map(|kids| kids.iter().map(node_from).collect())
            .unwrap_or_default(),
    }
}
```

Missing numeric fields default rather than failing: a plan whose shape gains a
field in a later release should still display, and a zero cost is visibly odd
where a refusal to render tells the user nothing.

- [ ] **Step 5: Run them and watch them pass**

```bash
cargo test --lib explain
```

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo test --lib
git add src/explain/mod.rs src/lib.rs
git commit -m "feat: parse an execution plan from EXPLAIN FORMAT JSON"
```

---

## Task 2: Flattening, text rendering and the safety refusal

**Files:**
- Modify: `src/explain/mod.rs`

**Interfaces:**
- Consumes: `PlanNode`.
- Produces: `PlanRow { depth: usize, node: PlanNode }`; `pub fn flatten(root: &PlanNode) -> Vec<PlanRow>`; `pub fn render_text(root: &PlanNode) -> String`; `pub fn explain_sql(statement: &str) -> Option<String>`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn flattening_is_depth_first_with_increasing_depth() {
        let plan = parse_plan(NESTED).unwrap();
        let rows = flatten(&plan);

        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].depth, rows[0].node.node_type.as_str()), (0, "Nested Loop"));
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 1);
    }

    #[test]
    fn the_text_rendering_indents_children_and_names_costs() {
        let text = render_text(&parse_plan(NESTED).unwrap());
        let lines: Vec<_> = text.lines().collect();

        assert!(lines[0].starts_with("Nested Loop"));
        assert!(lines[0].contains("cost=0.29..24.36"));
        assert!(lines[1].starts_with("  ->"), "children are indented: {:?}", lines[1]);
        assert!(lines[1].contains("on orders"), "a scan names its relation");
    }

    #[test]
    fn a_single_statement_is_wrapped_for_explaining() {
        let sql = explain_sql("SELECT * FROM t WHERE id = $1").expect("accepted");
        assert!(sql.starts_with("EXPLAIN (GENERIC_PLAN, FORMAT JSON) "));
        assert!(sql.ends_with("SELECT * FROM t WHERE id = $1"));
        assert!(!sql.contains("ANALYZE"), "ANALYZE must never be emitted");
    }

    #[test]
    fn a_trailing_semicolon_is_accepted_but_a_second_statement_is_not() {
        assert!(explain_sql("SELECT 1;").is_some());
        assert!(explain_sql("SELECT 1;   \n").is_some());
        // Anything after the semicolon would make this two statements.
        assert!(explain_sql("SELECT 1; DROP TABLE t").is_none());
        assert!(explain_sql("SELECT 1;SELECT 2").is_none());
    }
```

- [ ] **Step 2: Run and watch them fail**

```bash
cargo test --lib explain
```

- [ ] **Step 3: Implement**

```rust
/// One rendered line: a node and how deep it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRow {
    pub depth: usize,
    pub node: PlanNode,
}

pub fn flatten(root: &PlanNode) -> Vec<PlanRow> {
    let mut rows = Vec::new();
    push(root, 0, &mut rows);
    rows
}

fn push(node: &PlanNode, depth: usize, rows: &mut Vec<PlanRow>) {
    rows.push(PlanRow { depth, node: node.clone() });
    for child in &node.children {
        push(child, depth + 1, rows);
    }
}

/// The plan in the server's own idiom, rebuilt from the JSON so a second
/// round trip in FORMAT TEXT is not needed.
pub fn render_text(root: &PlanNode) -> String {
    flatten(root)
        .iter()
        .map(|row| {
            let indent = "  ".repeat(row.depth);
            let arrow = if row.depth == 0 { "" } else { "->  " };
            let relation = match row.node.relation.as_deref() {
                Some(name) => format!(" on {name}"),
                None => String::new(),
            };
            format!(
                "{indent}{arrow}{}{relation}  (cost={:.2}..{:.2} rows={} width={})",
                row.node.node_type,
                row.node.startup_cost,
                row.node.total_cost,
                row.node.rows,
                row.node.width
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wraps a statement for explaining, or refuses it.
///
/// `EXPLAIN` takes a statement rather than a string, so the text cannot be
/// passed as a parameter and is composed into SQL. The refusal below is what
/// keeps that from becoming a second statement: everything after the first
/// semicolon must be whitespace. The text itself comes from
/// pg_stat_statements on the same server and is normalised, so it carries no
/// literals — but a refusal costs nothing and the alternative is trusting
/// that permanently.
pub fn explain_sql(statement: &str) -> Option<String> {
    let trimmed = statement.trim();
    if let Some(index) = trimmed.find(';') {
        if !trimmed[index + 1..].trim().is_empty() {
            return None;
        }
    }
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("EXPLAIN (GENERIC_PLAN, FORMAT JSON) {trimmed}"))
}
```

- [ ] **Step 4: Run and watch them pass, then commit**

```bash
cargo test --lib explain
cargo fmt
git add src/explain/mod.rs
git commit -m "feat: flatten, render and safely wrap a plan request"
```

---

## Task 3: Running the explain on its own connection

**Files:**
- Modify: `src/collector/worker.rs`

**Interfaces:**
- Consumes: `explain_sql`, `StatementKey`.
- Produces: `CollectorHandle::explain(key: StatementKey, statement: String) -> bool`; `CollectorEvent::ExplainFinished { key: StatementKey, result: Result<String, CollectorError> }`.

- [ ] **Step 1: Add the event and the request channel**

In `src/collector/worker.rs`, extend `CollectorEvent`:

```rust
    /// A plan, or the reason there is not one. The key identifies the
    /// statement it belongs to, so a result arriving after the user moved on
    /// can be discarded rather than shown against the wrong query.
    ExplainFinished {
        key: StatementKey,
        result: Result<String, CollectorError>,
    },
```

Add a second command channel beside the existing action one, carrying
`(StatementKey, String)`, and a `explain` method on `CollectorHandle` mirroring
`submit`: a non-blocking `try_send` that returns false when the collector has
gone or the channel is full, so the caller can say so rather than let the user
believe a plan is coming.

- [ ] **Step 2: Run the request**

Beside `run_action`, add to `src/collector/action_runner.rs`:

```rust
/// Runs one EXPLAIN on a connection of its own and returns the JSON.
///
/// Its own connection for the same reason actions have one: an EXPLAIN
/// against a large catalogue can outlast a sampling interval, and the
/// sampler must not be behind it.
pub async fn run_explain(
    params: &ConnectionParams,
    password: &str,
    sql: &str,
) -> Result<String, CollectorError> {
    let (client, connection) = connect_once(params, password).await?;
    let handle = tokio::spawn(async move { let _ = connection.await; });

    let result = async {
        client
            .batch_execute("SET statement_timeout = '5s'")
            .await
            .map_err(map_query_error)?;
        let row = client.query_one(sql, &[]).await.map_err(map_query_error)?;
        let json: serde_json::Value = row.get(0);
        Ok(json.to_string())
    }
    .await;

    drop(client);
    handle.abort();
    result
}
```

`FORMAT JSON` returns a single `json` column, which `tokio-postgres` maps to
`serde_json::Value`; converting straight back to text keeps the parse in one
place, `src/explain`.

- [ ] **Step 3: Serve the channel in the sample loop**

In the same select that already drains the action channel, drain the explain
channel: run `run_explain`, then emit `ExplainFinished`. Failures are carried in
the event rather than counted against the connection's failure budget — a
statement that cannot be explained says nothing about the connection's health.

- [ ] **Step 4: Build and test**

```bash
cargo build
cargo test --lib
cargo test --bin mission-centre-pg
```

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/collector/worker.rs src/collector/action_runner.rs
git commit -m "feat: run an explain request off the sampling path"
```

---

## Task 4: The Plan page

**Files:**
- Create: `src/pages/plan.rs`, `resources/ui/plan_page.blp`
- Modify: `src/pages/mod.rs`, `resources/meson.build`, `resources/mission-centre-pg.gresource.xml`

**Interfaces:**
- Consumes: `PlanNode`, `flatten`, `render_text`, `PlanRow`.
- Produces: `McpgPlanPage` with `set_version(i32)`, `show_pending(&str)`, `show_plan(&str, &PlanNode)`, `show_error(&str)`.

- [ ] **Step 1: Write the layout**

Create `resources/ui/plan_page.blp`: a vertical box holding a `Gtk.Label statement_label` (single line, ellipsised) and a `Gtk.Label taken_at_label`, then a `Gtk.Stack state_stack` with four pages — `empty`, `unsupported`, `failed`, `plan` — the last containing an `Adw.ViewStack` of a `Gtk.TextView text_view` in a scroller and a `Gtk.ColumnView tree_view`.

Register it in `resources/meson.build` and `resources/mission-centre-pg.gresource.xml` beside `locks_page`.

- [ ] **Step 2: Write the failing tests for the pure helpers**

In `src/pages/plan.rs`:

```rust
    #[test]
    fn the_state_follows_what_is_known() {
        assert_eq!(state_for(None, 160000), "empty");
        assert_eq!(state_for(None, 150000), "unsupported");
        assert_eq!(state_for(Some(Err("boom".into())), 160000), "failed");
        assert!(matches!(state_for(Some(Ok(())), 160000), "plan"));
    }

    #[test]
    fn a_node_is_labelled_with_its_relation_when_it_has_one() {
        assert_eq!(node_label(&node("Seq Scan", Some("orders")), 0), "Seq Scan on orders");
        assert_eq!(node_label(&node("Aggregate", None), 0), "Aggregate");
        assert!(node_label(&node("Seq Scan", Some("orders")), 2).starts_with("        "));
    }
```

with the helpers under test:

```rust
/// Which stack page to show. The version gate outranks emptiness: a server
/// that cannot explain at all should say so rather than invite a right-click
/// that will not work.
pub fn state_for(result: Option<Result<(), String>>, version_num: i32) -> &'static str {
    if version_num < GENERIC_PLAN_VERSION {
        return "unsupported";
    }
    match result {
        None => "empty",
        Some(Err(_)) => "failed",
        Some(Ok(())) => "plan",
    }
}

pub fn node_label(node: &PlanNode, depth: usize) -> String {
    let indent = "    ".repeat(depth);
    match node.relation.as_deref() {
        Some(relation) => format!("{indent}{} on {relation}", node.node_type),
        None => format!("{indent}{}", node.node_type),
    }
}
```

- [ ] **Step 3: Run, watch fail, implement, watch pass**

```bash
cargo test --lib pages::plan
```

- [ ] **Step 4: Build the widget**

`McpgPlanPage` follows `src/pages/locks.rs`: template children, a `Table<PlanRow>`
attached to `tree_view` with columns Node (via `node_label`), Relation, Total
cost, Rows, Width, and a key of `format!("{depth}:{node_type}:{index}")`.
`show_plan` sets the text view's buffer from `render_text`, fills the table from
`flatten`, and switches `state_stack` to `plan`. `show_error` puts the server's
message on the `failed` page verbatim.

- [ ] **Step 5: Build and commit**

```bash
ninja -C build
cargo test --lib
cargo fmt
git add src/pages/plan.rs src/pages/mod.rs resources/ui/plan_page.blp resources/meson.build resources/mission-centre-pg.gresource.xml
git commit -m "feat: plan page with a text and a tree view"
```

---

## Task 5: The right-click menu and wiring

**Files:**
- Modify: `resources/ui/window.blp`, `resources/ui/queries_page.blp`, `src/pages/queries.rs`, `src/window.rs`, `src/window_actions.rs`

- [ ] **Step 1: Add the page to the window**

In `resources/ui/window.blp`, after the `relations` entry and before `locks`, per
the issue's "to the right of tables & indexes":

```blueprint
          Adw.ViewStackPage {
            name: "plan";
            title: _("Plan");
            icon-name: "view-list-bullet-symbolic";
            child: $McpgPlanPage plan_page {};
          }
```

Add the template child, `ensure_type()` and the `set_version` call beside the
replication page's in `src/window.rs`.

- [ ] **Step 2: Add the menu**

In `resources/ui/queries_page.blp`, add a menu and a right-click gesture:

```blueprint
menu query_context_menu {
  item {
    label: _("Explain query");
    action: "win.explain-query";
  }
}
```

In `src/pages/queries.rs`, attach a `Gtk.GestureClick` with `button: 3` to the
column view. On press: translate the coordinates to the row under the pointer,
**select it**, then pop up the menu at the pointer. Selecting first is what stops
the menu acting on a different statement from the one pointed at (spec §6).

- [ ] **Step 3: Add the action**

In `src/window_actions.rs`, register `explain-query` alongside the seven existing
actions. Its enablement is `connected && version_num >= 160000`. Activating it:

```rust
fn explain_selected_query(&self) {
    let imp = self.imp();
    let Some(statement) = imp.queries_page.selected_statement() else { return };
    let Some(sql) = explain_sql(&statement.query) else {
        self.toast(&i18n("That statement cannot be explained safely."));
        return;
    };
    imp.plan_page.show_pending(&statement.query);
    if !collector.explain(statement.key, sql) {
        self.toast(&i18n("The server is busy; the plan was not requested."));
    }
}
```

`selected_statement` is a new accessor on the Queries page returning the selected
`Statement`, mirroring `selected_session` on the Sessions page.

- [ ] **Step 4: Route the result**

In the collector event handler, add:

```rust
CollectorEvent::ExplainFinished { key, result } => {
    // Discard a plan for a statement the user has since moved away from.
    if imp.plan_page.pending_key() != Some(key) {
        return;
    }
    match result {
        Ok(json) => match parse_plan(&json) {
            Ok(plan) => {
                imp.plan_page.show_plan(&json, &plan);
                self.toast(&i18n("The plan is ready on the Plan page."));
            }
            Err(error) => imp.plan_page.show_error(&error.to_string()),
        },
        Err(error) => imp.plan_page.show_error(&error.to_string()),
    }
}
```

- [ ] **Step 5: Build, run and commit**

```bash
ninja -C build
cargo test --lib
cargo test --bin mission-centre-pg
cargo fmt
git add -A
git commit -m "feat: explain a query from the queries page"
```

---

## Task 6: Portability tests

**Files:**
- Modify: `tests/portability.rs`

- [ ] **Step 1: Assert the version boundary, including 15**

```rust
/// The boundary the whole feature turns on. 15 is tested as well as 14,
/// because the boundary is between 15 and 16 and an off-by-one there would
/// silently disable the feature on a supported server.
async fn assert_generic_plan_is_refused(tag: &str) {
    let (client, _container) = connect(tag).await;
    client
        .batch_execute("CREATE TABLE t (id int PRIMARY KEY, note text)")
        .await
        .expect("setup");

    let sql = explain_sql("SELECT * FROM t WHERE id = $1").expect("accepted");
    let error = client.query_one(sql.as_str(), &[]).await.expect_err("must be refused");
    assert!(
        error.to_string().contains("parameter"),
        "unexpected refusal: {error}"
    );
}

#[tokio::test]
async fn generic_plan_is_refused_on_postgres_14() {
    assert_generic_plan_is_refused("14").await;
}

#[tokio::test]
async fn generic_plan_is_refused_on_postgres_15() {
    assert_generic_plan_is_refused("15").await;
}

async fn assert_generic_plan_is_accepted(tag: &str) {
    let (client, _container) = connect(tag).await;
    client
        .batch_execute("CREATE TABLE t (id int PRIMARY KEY, note text)")
        .await
        .expect("setup");

    let sql = explain_sql("SELECT * FROM t WHERE id = $1").expect("accepted");
    let row = client.query_one(sql.as_str(), &[]).await.expect("must be accepted");
    let json: serde_json::Value = row.get(0);
    let plan = parse_plan(&json.to_string()).expect("the plan parses");
    assert!(plan.node_type.contains("Scan"), "unexpected root: {plan:?}");
}

#[tokio::test]
async fn generic_plan_is_accepted_on_postgres_16() {
    assert_generic_plan_is_accepted("16").await;
}

#[tokio::test]
async fn generic_plan_is_accepted_on_postgres_18() {
    assert_generic_plan_is_accepted("18").await;
}
```

- [ ] **Step 2: Explain a statement taken from pg_stat_statements**

The round trip that matters: the text the page would actually send comes from
the extension, not from a hand-written literal.

```rust
#[tokio::test]
async fn a_statement_from_pg_stat_statements_can_be_explained_on_postgres_18() {
    let (client, _container) = connect_with_statements("18").await;
    client
        .batch_execute("CREATE TABLE t (id int PRIMARY KEY, note text)")
        .await
        .expect("setup");
    client
        .execute("SELECT * FROM t WHERE id = $1", &[&1i32])
        .await
        .expect("run a parameterised statement so it is recorded");

    let row = wait_for(|| async {
        client
            .query_opt(
                "SELECT query FROM pg_stat_statements WHERE query LIKE 'SELECT * FROM t%' LIMIT 1",
                &[],
            )
            .await
            .ok()
            .flatten()
    })
    .await
    .expect("the statement must be recorded");

    let normalised: String = row.get("query");
    assert!(normalised.contains('$'), "expected a normalised query: {normalised}");

    let sql = explain_sql(&normalised).expect("accepted");
    let plan_row = client.query_one(sql.as_str(), &[]).await.expect("explains");
    let json: serde_json::Value = plan_row.get(0);
    assert!(parse_plan(&json.to_string()).is_ok());
}
```

- [ ] **Step 3: Run and commit**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability generic_plan
cargo test --test portability pg_stat_statements_can_be_explained
cargo fmt
git add tests/portability.rs
git commit -m "feat: prove the explain version boundary on 14, 15, 16 and 18"
```

---

## Task 7: Verification

**Files:** none modified unless a check fails.

- [ ] **Step 1: Every automated check**

```bash
cargo fmt --check
cargo test --lib
cargo test --bin mission-centre-pg
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
ninja -C build
```

- [ ] **Step 2: A server with real statements**

```bash
podman run --rm -d --name mcpg-plan -e POSTGRES_PASSWORD=postgres -p 55436:5432 \
  docker.io/library/postgres:18 -c shared_preload_libraries=pg_stat_statements
podman exec mcpg-plan bash -c 'until pg_isready -U postgres -q; do sleep 1; done'
podman exec -i mcpg-plan psql -U postgres -c "CREATE EXTENSION pg_stat_statements"
podman exec -i mcpg-plan psql -U postgres -c "CREATE TABLE orders (id bigserial PRIMARY KEY, customer int, note text)"
podman exec -i mcpg-plan psql -U postgres -c "CREATE TABLE customers (id int PRIMARY KEY, name text)"
podman exec -i mcpg-plan psql -U postgres -c "INSERT INTO customers SELECT g, 'c'||g FROM generate_series(1,1000) g"
podman exec -i mcpg-plan psql -U postgres -c "INSERT INTO orders (customer, note) SELECT (random()*999)::int+1, 'n' FROM generate_series(1,50000)"
podman exec -i mcpg-plan psql -U postgres -c "ANALYZE"
# A join, so the plan has more than one node
podman exec -i mcpg-plan psql -U postgres -c "SELECT o.*, c.name FROM orders o JOIN customers c ON c.id = o.customer WHERE o.id = 42"
```

- [ ] **Step 3: Walk the success criteria**

Run the application against `127.0.0.1:55436`, then tick spec §9:

- [ ] Right-clicking a Queries row offers *Explain query*.
- [ ] Choosing it fills the Plan page within a few seconds; the Overview graphs keep updating.
- [ ] The join's plan nests — its inputs appear as children.
- [ ] Each node shows total cost, rows and width.
- [ ] The text view shows the same plan in the server's own words.
- [ ] The statement shown matches the row that was right-clicked.
- [ ] Against a PostgreSQL 15 server, the menu item is insensitive and states the requirement.
- [ ] A statement whose table has been dropped reports the server's error verbatim.
- [ ] Explaining a second statement replaces the first.

Use `tools/uicheck.py digest` to read the page state rather than describing it
from memory.

- [ ] **Step 4: Tear down and open the pull request**

```bash
podman rm -f mcpg-plan
git push -u origin feature/explain-plans
gh pr create --title "Explain plans for captured statements (#5)" \
  --body "Implements docs/superpowers/specs/2026-07-27-explain-plans-design.md — a Plan page showing EXPLAIN (GENERIC_PLAN, FORMAT JSON) for a statement chosen by right-clicking on the Queries page, as the server's own text and as a tree of costed nodes. Closes #5."
```

---

## Self-Review Notes

**Spec coverage.** §1.1 version constraint → Task 6's four boundary tests, 15 included. §2.2 no `EXPLAIN ANALYZE` → Task 2 asserts the emitted SQL never contains it. §3.1 statement composition and the semicolon refusal → Task 2's `explain_sql` and its tests. §3.2 own connection and timeout → Task 3's `run_explain`. §3.3 keyed result → Task 3's event and Task 5's discard-on-mismatch. §4 model and parse → Task 1. §5 the two views and four states → Task 4. §6 right-click that selects first → Task 5 Step 2. §7 error states verbatim → Task 4's `show_error`. §8 testing → Tasks 1, 2, 6. §9 criteria → Task 7 Step 3.

**Type consistency.** `PlanNode` is defined once in Task 1 and unchanged after. `PlanRow` (page display) is distinct from `PlanNode` (model) and only Task 2 converts between them. `explain_sql` returns `Option<String>` — `None` is a refusal, not an error, because refusing is a normal outcome for a statement the tool will not send. `ExplainFinished` carries `Result<String, CollectorError>` where the `String` is JSON text, parsed in the window rather than the collector, so the collector never depends on `src/explain`.

**One trap worth knowing.** `EXPLAIN (… FORMAT JSON)` returns a single column of type `json`, not `text`. `row.get::<_, String>(0)` panics on a type mismatch; the plan takes `serde_json::Value` and converts, which is why `run_explain` returns `String` rather than the row.

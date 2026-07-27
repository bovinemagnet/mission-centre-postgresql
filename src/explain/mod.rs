/* explain/mod.rs
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

//! Execution plans for statements captured by `pg_stat_statements`.
//!
//! Those statements are normalised — literals replaced by `$1`, `$2` — so they
//! cannot be planned by asking the server to explain them directly; there is no
//! parameter to bind. `EXPLAIN (GENERIC_PLAN)` exists for this case and arrived
//! in PostgreSQL 16, which is why the whole feature is version gated.
//!
//! `ANALYZE` is never emitted. It executes the statement, and these are
//! arbitrary captured queries that may be a `DELETE`.

use serde_json::Value;

/// The first version that can plan a parameterised statement without values.
pub const GENERIC_PLAN_VERSION: i32 = 160000;

/// One node of an execution plan, with the estimates the planner attached.
/// Actual figures are deliberately absent: obtaining them needs `EXPLAIN
/// ANALYZE`, which executes the statement.
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
    #[error("The server returned a plan that could not be read: {0}")]
    Malformed(String),
}

pub fn parse_plan(json: &str) -> Result<PlanNode, ExplainError> {
    let document: Value =
        serde_json::from_str(json).map_err(|e| ExplainError::Malformed(e.to_string()))?;

    let root = document
        .get(0)
        .and_then(|entry| entry.get("Plan"))
        .ok_or_else(|| ExplainError::Malformed("no Plan member".into()))?;

    Ok(node_from(root))
}

/// Missing numeric fields default rather than failing the parse. A plan whose
/// shape gains a member in a later release should still be readable, and a
/// visibly odd zero tells the user more than a refusal to render anything.
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
        startup_cost: value
            .get("Startup Cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        total_cost: value
            .get("Total Cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        rows: value.get("Plan Rows").and_then(Value::as_i64).unwrap_or(0),
        width: value.get("Plan Width").and_then(Value::as_i64).unwrap_or(0),
        children: value
            .get("Plans")
            .and_then(Value::as_array)
            .map(|kids| kids.iter().map(node_from).collect())
            .unwrap_or_default(),
    }
}

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
    rows.push(PlanRow {
        depth,
        node: node.clone(),
    });
    for child in &node.children {
        push(child, depth + 1, rows);
    }
}

/// The plan in the server's own idiom, rebuilt from the JSON so a second round
/// trip in `FORMAT TEXT` is not needed.
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
/// passed as a parameter and has to be composed into SQL. The refusal below is
/// what stops that becoming a second statement: everything after the first
/// semicolon must be whitespace. The text itself comes from
/// `pg_stat_statements` on the same server and is normalised, so it carries no
/// literals — but refusing costs nothing, and the alternative is trusting that
/// permanently.
pub fn explain_sql(statement: &str) -> Option<String> {
    let trimmed = statement.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(index) = trimmed.find(';') {
        if !trimmed[index + 1..].trim().is_empty() {
            return None;
        }
    }
    Some(format!("EXPLAIN (GENERIC_PLAN, FORMAT JSON) {trimmed}"))
}

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

        assert_eq!(parse_plan(json).expect("parses").relation, None);
    }

    #[test]
    fn flattening_is_depth_first_with_increasing_depth() {
        let plan = parse_plan(NESTED).unwrap();
        let rows = flatten(&plan);

        assert_eq!(rows.len(), 3);
        assert_eq!(
            (rows[0].depth, rows[0].node.node_type.as_str()),
            (0, "Nested Loop")
        );
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 1);
    }

    #[test]
    fn the_text_rendering_indents_children_and_names_costs() {
        let text = render_text(&parse_plan(NESTED).unwrap());
        let lines: Vec<_> = text.lines().collect();

        assert!(lines[0].starts_with("Nested Loop"));
        assert!(lines[0].contains("cost=0.29..24.36"));
        assert!(
            lines[1].starts_with("  ->"),
            "children are indented: {:?}",
            lines[1]
        );
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
        assert!(explain_sql("   ").is_none());
    }

    #[test]
    fn malformed_json_is_reported_rather_than_panicking() {
        assert!(matches!(
            parse_plan("not json"),
            Err(ExplainError::Malformed(_))
        ));
        assert!(matches!(parse_plan("[]"), Err(ExplainError::Malformed(_))));
        assert!(matches!(
            parse_plan(r#"[{"NoPlan":1}]"#),
            Err(ExplainError::Malformed(_))
        ));
    }
}

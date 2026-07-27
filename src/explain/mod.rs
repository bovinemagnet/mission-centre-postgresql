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

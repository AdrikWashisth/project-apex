//! Execution plans: the unit of work for multi-agent tasks.
//!
//! A plan is an ordered set of steps, each bound to an agent, with explicit
//! dependencies. Plans are validated before anything runs: dangling
//! dependencies, duplicate ids and cycles are rejected up front rather than
//! discovered halfway through a task.
//!
//! Validation against the agent catalog lives in `apex-orchestrator`, which
//! depends on the agent registry. This module stays free of that dependency so
//! plans can appear on the wire.

use apex_core::error::{ApexError, Result};
use serde::{Deserialize, Serialize};

/// A glob that matches the entire workspace.
pub const MATCH_ALL: &str = "**";

/// One unit of delegated work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlannedStep {
    /// Unique identifier within the plan.
    pub id: String,
    /// The agent that will perform this step.
    pub agent_id: String,
    /// What this step must accomplish.
    pub objective: String,
    /// Steps that must complete before this one may start.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Paths or globs this step may modify. Used for conflict detection.
    /// Empty means "may modify anything", which is treated conservatively.
    #[serde(default)]
    pub writes: Vec<String>,
    /// Model override for this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// True when this step cannot modify the workspace (for example a review).
    /// Read-only steps never conflict with each other and may run in parallel.
    #[serde(default)]
    pub read_only: bool,
}

impl PlannedStep {
    /// Create a step with no declared write scope.
    pub fn new(
        id: impl Into<String>,
        agent_id: impl Into<String>,
        objective: impl Into<String>,
    ) -> PlannedStep {
        PlannedStep {
            id: id.into(),
            agent_id: agent_id.into(),
            objective: objective.into(),
            depends_on: Vec::new(),
            writes: Vec::new(),
            model: None,
            read_only: false,
        }
    }

    /// Declare a dependency on another step.
    pub fn depending_on(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.depends_on = ids.into_iter().map(Into::into).collect();
        self
    }

    /// Declare which paths this step may modify.
    pub fn writes(mut self, paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.writes = paths.into_iter().map(Into::into).collect();
        self
    }

    /// Set whether this step can modify the workspace.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// The effective write scope. An undeclared step may touch the whole
    /// workspace, which is why undeclared steps are serialised against each
    /// other by the scheduler. A read-only step has no write scope at all.
    pub fn effective_writes(&self) -> Vec<&str> {
        if self.read_only {
            Vec::new()
        } else if self.writes.is_empty() {
            vec![MATCH_ALL]
        } else {
            self.writes.iter().map(|s| s.as_str()).collect()
        }
    }
}

/// A validated set of steps to execute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Plan {
    pub steps: Vec<PlannedStep>,
}

impl Plan {
    /// Build a plan from steps.
    pub fn new(steps: Vec<PlannedStep>) -> Plan {
        Plan { steps }
    }

    /// Number of steps.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the plan has no steps.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Look up a step by id.
    pub fn step(&self, id: &str) -> Option<&PlannedStep> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// Validate the plan's structure.
    ///
    /// Checks that ids are unique and non-empty, dependencies point at real
    /// steps, no step depends on itself, and the graph is acyclic.
    pub fn validate(&self) -> Result<()> {
        if self.steps.is_empty() {
            return Err(ApexError::config("plan contains no steps"));
        }

        let mut seen = std::collections::HashSet::new();
        for step in &self.steps {
            if step.id.trim().is_empty() {
                return Err(ApexError::config("plan step requires a non-empty id"));
            }
            if step.agent_id.trim().is_empty() {
                return Err(ApexError::config(format!(
                    "plan step '{}' requires an agent id",
                    step.id
                )));
            }
            if !seen.insert(step.id.as_str()) {
                return Err(ApexError::config(format!(
                    "duplicate plan step id '{}'",
                    step.id
                )));
            }
        }

        for step in &self.steps {
            for dep in &step.depends_on {
                if dep == &step.id {
                    return Err(ApexError::config(format!(
                        "plan step '{}' depends on itself",
                        step.id
                    )));
                }
                if !seen.contains(dep.as_str()) {
                    return Err(ApexError::config(format!(
                        "plan step '{}' depends on unknown step '{}'",
                        step.id, dep
                    )));
                }
            }
        }

        if let Some(cycle) = self.find_cycle() {
            return Err(ApexError::config(format!(
                "plan contains a dependency cycle: {}",
                cycle.join(" -> ")
            )));
        }
        Ok(())
    }

    /// Return one cycle if the dependency graph contains one.
    fn find_cycle(&self) -> Option<Vec<String>> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Unvisited,
            InProgress,
            Done,
        }

        let mut marks = std::collections::HashMap::new();
        for step in &self.steps {
            marks.insert(step.id.as_str(), Mark::Unvisited);
        }

        for start in &self.steps {
            if marks.get(start.id.as_str()) != Some(&Mark::Unvisited) {
                continue;
            }
            let mut path = vec![start.id.clone()];
            let mut stack = vec![(start, 0usize)];
            marks.insert(start.id.as_str(), Mark::InProgress);

            while let Some((node, index)) = stack.pop() {
                if index < node.depends_on.len() {
                    stack.push((node, index + 1));
                    let dep = &node.depends_on[index];
                    match marks.get(dep.as_str()) {
                        Some(Mark::InProgress) => {
                            // Back edge: reconstruct the cycle for the message.
                            let mut cycle = path.clone();
                            if let Some(pos) = cycle.iter().position(|s| s == dep) {
                                cycle = cycle[pos..].to_vec();
                            }
                            cycle.push(dep.clone());
                            return Some(cycle);
                        }
                        Some(Mark::Done) => {}
                        _ => {
                            if let Some(next) = self.step(dep) {
                                marks.insert(dep.as_str(), Mark::InProgress);
                                path.push(dep.clone());
                                stack.push((next, 0));
                            }
                        }
                    }
                } else {
                    marks.insert(node.id.as_str(), Mark::Done);
                    if path.last().map(|s| s == &node.id).unwrap_or(false) {
                        path.pop();
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_linear_plan() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "apex-default", "first"),
            PlannedStep::new("b", "apex-default", "second").depending_on(["a"]),
        ]);
        plan.validate().unwrap();
    }

    #[test]
    fn rejects_duplicate_ids() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "apex-default", "one"),
            PlannedStep::new("a", "apex-default", "two"),
        ]);
        let err = plan.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate"), "got {err}");
    }

    #[test]
    fn rejects_dangling_dependency() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "apex-default", "one").depending_on(["ghost"])
        ]);
        let err = plan.validate().unwrap_err().to_string();
        assert!(err.contains("unknown step"), "got {err}");
    }

    #[test]
    fn rejects_self_dependency() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "apex-default", "one").depending_on(["a"])
        ]);
        let err = plan.validate().unwrap_err().to_string();
        assert!(err.contains("depends on itself"), "got {err}");
    }

    #[test]
    fn rejects_two_node_cycle() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "apex-default", "one").depending_on(["b"]),
            PlannedStep::new("b", "apex-default", "two").depending_on(["a"]),
        ]);
        let err = plan.validate().unwrap_err().to_string();
        assert!(err.contains("cycle"), "got {err}");
    }

    #[test]
    fn rejects_three_node_cycle() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "apex-default", "one").depending_on(["c"]),
            PlannedStep::new("b", "apex-default", "two").depending_on(["a"]),
            PlannedStep::new("c", "apex-default", "three").depending_on(["b"]),
        ]);
        let err = plan.validate().unwrap_err().to_string();
        assert!(err.contains("cycle"), "got {err}");
    }

    #[test]
    fn rejects_empty_plan() {
        assert!(Plan::default().validate().is_err());
    }

    #[test]
    fn diamond_is_accepted() {
        let plan = Plan::new(vec![
            PlannedStep::new("root", "x", "r"),
            PlannedStep::new("l", "x", "l").depending_on(["root"]),
            PlannedStep::new("r", "x", "r").depending_on(["root"]),
            PlannedStep::new("j", "x", "j").depending_on(["l", "r"]),
        ]);
        plan.validate().unwrap();
    }

    #[test]
    fn undeclared_writes_mean_whole_workspace() {
        let step = PlannedStep::new("a", "x", "one");
        assert_eq!(step.effective_writes(), vec![MATCH_ALL]);
        let declared = PlannedStep::new("b", "x", "one").writes(["src/a.rs"]);
        assert_eq!(declared.effective_writes(), vec!["src/a.rs"]);
        let read_only = PlannedStep::new("c", "x", "one").read_only(true);
        assert!(read_only.effective_writes().is_empty());
    }
}

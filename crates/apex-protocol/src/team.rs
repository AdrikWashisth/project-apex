//! Manual multi-agent team specifications.

use serde::{Deserialize, Serialize};

/// Whether team members run one after another or concurrently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStrategy {
    /// Each agent runs after the previous one and receives its findings.
    /// Safe default: agents hand work forward rather than racing.
    #[default]
    Sequential,
    /// Agents run concurrently. Only safe when their write scopes are disjoint.
    Parallel,
}

/// A user-assembled team of agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSpec {
    /// Agent ids, in priority order.
    pub agents: Vec<String>,
    /// How the team executes.
    #[serde(default)]
    pub strategy: TeamStrategy,
    /// Model override applied to every team member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl TeamSpec {
    /// Build a sequential team.
    pub fn new(agents: impl IntoIterator<Item = impl Into<String>>) -> TeamSpec {
        TeamSpec {
            agents: agents.into_iter().map(Into::into).collect(),
            strategy: TeamStrategy::Sequential,
            model: None,
        }
    }

    /// Parse a comma-separated agent list, as typed on the command line.
    pub fn from_csv(csv: &str) -> TeamSpec {
        TeamSpec::new(
            csv.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
        )
    }
}

/// A plan as it appears on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlannedTask {
    pub steps: Vec<crate::plan::PlannedStep>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_csv_from_the_command_line() {
        let spec = TeamSpec::from_csv("apex-debugger, apex-reviewer ,");
        assert_eq!(spec.agents, vec!["apex-debugger", "apex-reviewer"]);
    }

    #[test]
    fn ignores_empty_entries() {
        assert!(TeamSpec::from_csv("  ,  ").agents.is_empty());
    }

    #[test]
    fn defaults_to_sequential() {
        assert_eq!(TeamSpec::new(["a"]).strategy, TeamStrategy::Sequential);
    }

    #[test]
    fn deserialises_without_optional_fields() {
        let spec: TeamSpec = serde_json::from_str(r#"{"agents":["a","b"]}"#).unwrap();
        assert_eq!(spec.agents.len(), 2);
        assert_eq!(spec.strategy, TeamStrategy::Sequential);
    }
}

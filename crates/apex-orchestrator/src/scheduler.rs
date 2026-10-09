//! Dependency-aware, conflict-aware scheduling.
//!
//! The scheduler turns a validated [`Plan`] into *waves*: ordered groups of
//! steps that may run concurrently. Steps in later waves depend on earlier
//! ones. Within a wave, two steps never share a declared write target, so
//! concurrent agents cannot silently clobber each other's files.
//!
//! Conflict detection is deliberately conservative. Two write globs are treated
//! as conflicting unless they are provably disjoint. A false positive only
//! costs parallelism; a false negative would corrupt a user's work.

use apex_core::error::Result;
use serde::{Deserialize, Serialize};

use apex_protocol::{Plan, PlannedStep, MATCH_ALL};

/// Default number of steps allowed to run at once.
pub const DEFAULT_MAX_PARALLEL: usize = 4;

/// Tuning for the scheduler.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Maximum steps running concurrently.
    pub max_parallel: usize,
    /// When true, steps that declare no write scope still conflict with each
    /// other. This is the safe default; disabling it risks concurrent writes
    /// to the same unknown files.
    pub serialize_undeclared: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        SchedulerConfig {
            max_parallel: DEFAULT_MAX_PARALLEL,
            serialize_undeclared: true,
        }
    }
}

/// Compute execution waves for a plan.
///
/// Returns an error if the plan is invalid. Every step appears in exactly one
/// wave, and a step's wave index is always greater than that of its
/// dependencies.
pub fn schedule(plan: &Plan, config: SchedulerConfig) -> Result<Vec<Vec<PlannedStep>>> {
    plan.validate()?;
    let max_parallel = config.max_parallel.max(1);

    let mut completed: Vec<String> = Vec::new();
    let mut remaining: Vec<&PlannedStep> = plan.steps.iter().collect();
    let mut waves: Vec<Vec<PlannedStep>> = Vec::new();

    while !remaining.is_empty() {
        // Steps whose dependencies are all satisfied.
        let ready: Vec<&PlannedStep> = remaining
            .iter()
            .copied()
            .filter(|s| s.depends_on.iter().all(|d| completed.contains(d)))
            .collect();

        if ready.is_empty() {
            // Unreachable for a validated plan (which is acyclic), but fail
            // loudly rather than looping forever if one ever slips through.
            return Err(apex_core::error::ApexError::config(
                "scheduler could not make progress; the plan may be malformed",
            ));
        }

        let mut wave: Vec<PlannedStep> = Vec::new();
        let mut deferred: Vec<&PlannedStep> = Vec::new();

        for step in ready {
            if wave.len() >= max_parallel {
                deferred.push(step);
                continue;
            }
            let conflicts = wave
                .iter()
                .any(|chosen| steps_conflict(chosen, step, config.serialize_undeclared));
            if conflicts {
                deferred.push(step);
            } else {
                wave.push(step.clone());
            }
        }

        // Every step selected this wave is now complete for dependency purposes.
        for step in &wave {
            completed.push(step.id.clone());
            remaining.retain(|s| s.id != step.id);
        }
        waves.push(wave);

        if deferred.is_empty() && remaining.is_empty() {
            break;
        }
    }

    Ok(waves)
}

/// Whether two steps may conflict over a file.
///
/// Undeclared write scopes (empty `writes`) are treated as `**` when
/// `serialize_undeclared` is set, so two undeclared steps never run together.
pub fn steps_conflict(a: &PlannedStep, b: &PlannedStep, serialize_undeclared: bool) -> bool {
    if a.id == b.id {
        return true;
    }
    // Two read-only steps can never conflict: neither can modify a file.
    if a.read_only && b.read_only {
        return false;
    }
    let a_writes: Vec<String> = if a.read_only {
        Vec::new()
    } else if a.writes.is_empty() && serialize_undeclared {
        vec![MATCH_ALL.to_string()]
    } else {
        a.writes.clone()
    };
    let b_writes: Vec<String> = if b.read_only {
        Vec::new()
    } else if b.writes.is_empty() && serialize_undeclared {
        vec![MATCH_ALL.to_string()]
    } else {
        b.writes.clone()
    };

    a_writes
        .iter()
        .any(|x| b_writes.iter().any(|y| globs_conflict(x, y)))
}

/// Conservative overlap test for two path globs.
///
/// Returns `false` only when the globs are provably disjoint: that happens when
/// two literal (glob-free) path segments differ. Everything else — equal
/// paths, a match-all pattern, any glob metacharacter, or a directory prefix
/// — is reported as a conflict. A false positive merely costs parallelism; a
/// false negative would let two agents overwrite the same file.
pub fn globs_conflict(a: &str, b: &str) -> bool {
    let a = a.trim().trim_end_matches('/');
    let b = b.trim().trim_end_matches('/');
    if a.is_empty() || b.is_empty() {
        return true;
    }
    if a == b {
        return true;
    }
    if a == MATCH_ALL || b == MATCH_ALL {
        return true;
    }

    let left: Vec<&str> = a.split('/').collect();
    let right: Vec<&str> = b.split('/').collect();

    for index in 0..left.len().min(right.len()) {
        let (x, y) = (left[index], right[index]);
        // Two literal segments that differ cannot both match the same path.
        if !is_glob_segment(x) && !is_glob_segment(y) && x != y {
            return false;
        }
    }

    // No provable disjointness: assume they overlap.
    true
}

/// Whether a path segment contains glob metacharacters.
fn is_glob_segment(segment: &str) -> bool {
    segment.contains('*') || segment.contains('?') || segment.contains('[')
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_protocol::PlannedStep;

    fn cfg() -> SchedulerConfig {
        SchedulerConfig {
            max_parallel: 8,
            serialize_undeclared: true,
        }
    }

    fn wave_ids(waves: &[Vec<PlannedStep>]) -> Vec<Vec<String>> {
        waves
            .iter()
            .map(|w| w.iter().map(|s| s.id.clone()).collect())
            .collect()
    }

    #[test]
    fn independent_steps_share_a_wave() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "one").writes(["src/a.rs"]),
            PlannedStep::new("b", "x", "two").writes(["docs/b.md"]),
        ]);
        let waves = schedule(&plan, cfg()).unwrap();
        assert_eq!(
            wave_ids(&waves),
            vec![vec!["a".to_string(), "b".to_string()]]
        );
    }

    #[test]
    fn dependencies_force_later_waves() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "one").writes(["src/a.rs"]),
            PlannedStep::new("b", "x", "two")
                .writes(["src/b.rs"])
                .depending_on(["a"]),
        ]);
        let waves = schedule(&plan, cfg()).unwrap();
        assert_eq!(
            wave_ids(&waves),
            vec![vec!["a".to_string()], vec!["b".to_string()]]
        );
    }

    #[test]
    fn conflicting_write_scopes_are_serialised() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "one").writes(["src/main.rs"]),
            PlannedStep::new("b", "x", "two").writes(["src/main.rs"]),
        ]);
        let waves = schedule(&plan, cfg()).unwrap();
        assert_eq!(
            wave_ids(&waves),
            vec![vec!["a".to_string()], vec!["b".to_string()]],
            "two steps writing the same file must not run concurrently"
        );
    }

    #[test]
    fn distinct_files_in_one_directory_run_in_parallel() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "one").writes(["src/lib.rs"]),
            PlannedStep::new("b", "x", "two").writes(["src/other.rs"]),
        ]);
        let waves = schedule(&plan, cfg()).unwrap();
        assert_eq!(
            waves.len(),
            1,
            "provably distinct files should be allowed to run concurrently"
        );
    }

    #[test]
    fn a_directory_claim_conflicts_with_files_inside_it() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "one").writes(["src"]),
            PlannedStep::new("b", "x", "two").writes(["src/other.rs"]),
        ]);
        let waves = schedule(&plan, cfg()).unwrap();
        assert_eq!(waves.len(), 2, "a directory claim overlaps its contents");
    }

    #[test]
    fn a_glob_claim_conflicts_with_a_specific_file() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "one").writes(["src/*.rs"]),
            PlannedStep::new("b", "x", "two").writes(["src/other.rs"]),
        ]);
        let waves = schedule(&plan, cfg()).unwrap();
        assert_eq!(waves.len(), 2, "a glob claim overlaps a concrete file");
    }

    #[test]
    fn read_only_steps_never_conflict() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "one").read_only(true),
            PlannedStep::new("b", "x", "two").read_only(true),
        ]);
        let waves = schedule(&plan, cfg()).unwrap();
        assert_eq!(
            waves.len(),
            1,
            "read-only agents can always run in parallel"
        );
    }

    #[test]
    fn undeclared_steps_are_serialised_by_default() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "one"),
            PlannedStep::new("b", "x", "two"),
        ]);
        let waves = schedule(&plan, cfg()).unwrap();
        assert_eq!(waves.len(), 2, "undeclared write scopes must not overlap");
    }

    #[test]
    fn undeclared_steps_may_run_together_when_opted_in() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "one"),
            PlannedStep::new("b", "x", "two"),
        ]);
        let config = SchedulerConfig {
            max_parallel: 8,
            serialize_undeclared: false,
        };
        let waves = schedule(&plan, config).unwrap();
        assert_eq!(waves.len(), 1);
    }

    #[test]
    fn parallelism_is_capped() {
        let plan = Plan::new(vec![
            PlannedStep::new("a", "x", "1").writes(["f/a"]),
            PlannedStep::new("b", "x", "2").writes(["f/b"]),
            PlannedStep::new("c", "x", "3").writes(["f/c"]),
        ]);
        let config = SchedulerConfig {
            max_parallel: 2,
            serialize_undeclared: true,
        };
        let waves = schedule(&plan, config).unwrap();
        assert!(
            waves.iter().all(|w| w.len() <= 2),
            "waves must respect max_parallel"
        );
        assert_eq!(waves.iter().map(|w| w.len()).sum::<usize>(), 3);
    }

    #[test]
    fn diamond_dependencies_schedule_correctly() {
        let plan = Plan::new(vec![
            PlannedStep::new("root", "x", "root").writes(["plan.txt"]),
            PlannedStep::new("left", "x", "left")
                .writes(["l.txt"])
                .depending_on(["root"]),
            PlannedStep::new("right", "x", "right")
                .writes(["r.txt"])
                .depending_on(["root"]),
            PlannedStep::new("join", "x", "join")
                .writes(["j.txt"])
                .depending_on(["left", "right"]),
        ]);
        let waves = schedule(&plan, cfg()).unwrap();
        assert_eq!(
            wave_ids(&waves),
            vec![
                vec!["root".to_string()],
                vec!["left".to_string(), "right".to_string()],
                vec!["join".to_string()],
            ]
        );
    }

    #[test]
    fn glob_conflict_matrix() {
        // Same file, match-all, directory prefix and globstar all conflict.
        assert!(globs_conflict("src/main.rs", "src/main.rs"));
        assert!(globs_conflict("**", "anything"));
        assert!(globs_conflict("src", "src/main.rs"));
        assert!(globs_conflict("src/*.rs", "src/main.rs"));
        assert!(globs_conflict("src", "src"));
        // Different top-level directories are provably disjoint.
        assert!(!globs_conflict("src/a.rs", "docs/b.md"));
        assert!(!globs_conflict("crates/one", "crates/two"));
    }
}

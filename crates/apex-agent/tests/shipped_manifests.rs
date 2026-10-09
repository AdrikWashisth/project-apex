//! Validate the agent manifests shipped in the repository `agents/` directory.
//!
//! This ensures the documented examples are real, loadable manifests rather
//! than illustrative snippets that drift out of sync with the code.

use apex_agent::AgentManifest;
use std::path::{Path, PathBuf};

/// The repository root, resolved from the crate directory at compile time.
fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../crates/apex-agent; the repo root is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate is nested two levels below the repo root")
        .to_path_buf()
}

#[test]
fn shipped_manifests_are_valid() {
    let dir = repo_root().join("agents");

    assert!(
        dir.exists(),
        "agents directory should exist at {}",
        dir.display()
    );

    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("agents dir is readable") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
        let manifest = AgentManifest::from_toml(&text)
            .unwrap_or_else(|e| panic!("invalid manifest {}: {e}", path.display()));
        assert!(!manifest.instructions.trim().is_empty());
        count += 1;
    }

    assert!(
        count >= 3,
        "expected at least three shipped manifests, found {count}"
    );
}

#[test]
fn reviewer_manifest_cannot_write() {
    let path = repo_root().join("agents").join("apex-reviewer.toml");
    let manifest = AgentManifest::from_toml(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(!manifest.allows_tool("write_file"));
    assert!(!manifest.allows_tool("edit_file"));
    assert!(manifest.allows_tool("read_file"));
    assert!(manifest.allows_tool("git_diff"));
}

#[test]
fn shipped_manifests_round_trip_through_toml() {
    let path = repo_root().join("agents").join("apex-default.toml");
    let manifest = AgentManifest::from_toml(&std::fs::read_to_string(path).unwrap()).unwrap();
    let serialised = manifest.to_toml().unwrap();
    let reparsed = AgentManifest::from_toml(&serialised).unwrap();
    assert_eq!(reparsed.id, manifest.id);
    assert_eq!(reparsed.name, manifest.name);
    assert_eq!(reparsed.instructions, manifest.instructions);
}

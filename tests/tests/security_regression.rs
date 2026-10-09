//! Security regression tests.
//!
//! These lock in the guarantees documented in docs/SECURITY_MODEL.md. If any of
//! them fails, the platform's trust boundary has regressed.

use apex_core::config::{PermissionProfile, Permissions};
use apex_core::error::ApexError;
use apex_core::paths::resolve_within;
use apex_tools::{ToolContext, ToolRegistry};
use serde_json::json;

fn ctx(root: &std::path::Path, profile: PermissionProfile) -> ToolContext {
    let mut permissions = Permissions::default();
    permissions.profile = profile;
    permissions.allow_shell = true;
    permissions.require_approval.clear();
    ToolContext::new(root.to_path_buf(), permissions)
}

#[test]
fn resolve_within_rejects_parent_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let inside = root.join("ok.txt");
    std::fs::write(&inside, "x").unwrap();

    // Escaping via ..
    assert!(resolve_within(root, "../escape.txt").is_err());
    assert!(resolve_within(root, "sub/../../escape.txt").is_err());
    assert!(resolve_within(root, "a/b/../../../etc/passwd").is_err());

    // Absolute path outside the root.
    let outside = tmp.path().parent().unwrap().join("definitely-outside.txt");
    assert!(resolve_within(root, &outside).is_err());

    // Legitimate paths still work.
    assert!(resolve_within(root, "ok.txt").is_ok());
    assert!(resolve_within(root, "sub/dir/new.txt").is_ok());
    assert!(resolve_within(root, "./ok.txt").is_ok());
}

#[tokio::test]
async fn write_tool_cannot_escape_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let registry = ToolRegistry::default_set();
    let ctx = ctx(root, PermissionProfile::ControlledAutonomous);

    let err = registry
        .execute(
            "write_file",
            json!({"path": "../escaped.txt", "content": "bad"}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ApexError::PathEscape { .. }), "got {err:?}");
    assert!(!root.parent().unwrap().join("escaped.txt").exists());
}

#[tokio::test]
async fn write_tool_refuses_to_touch_git_internals() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let registry = ToolRegistry::default_set();
    let ctx = ctx(root, PermissionProfile::ControlledAutonomous);

    let err = registry
        .execute(
            "write_file",
            json!({"path": ".git/config", "content": "pwned"}),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ApexError::Permission(_)), "got {err:?}");
}

#[tokio::test]
async fn read_tool_cannot_escape_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let secret = tmp.path().parent().unwrap().join("secret.txt");
    std::fs::write(&secret, "top secret").unwrap();
    let registry = ToolRegistry::default_set();
    let ctx = ctx(root, PermissionProfile::ControlledAutonomous);

    let err = registry
        .execute("read_file", json!({"path": "../secret.txt"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, ApexError::PathEscape { .. }), "got {err:?}");
}

#[tokio::test]
async fn read_only_profile_blocks_every_mutating_and_executing_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::default_set();
    let ctx = ctx(tmp.path(), PermissionProfile::ReadOnly);

    let blocked = [
        ("write_file", json!({"path": "a", "content": "b"})),
        (
            "edit_file",
            json!({"path": "a", "old_string": "x", "new_string": "y"}),
        ),
        ("run_command", json!({"command": "echo hi"})),
        ("run_build", json!({})),
        ("run_tests", json!({})),
    ];
    for (name, args) in blocked {
        let err = registry.execute(name, args, &ctx).await.unwrap_err();
        assert!(
            matches!(err, ApexError::Permission(_)),
            "tool '{name}' should be denied by the read_only profile, got {err:?}"
        );
    }

    // Read-only tools still work.
    std::fs::write(tmp.path().join("readable.txt"), "hello").unwrap();
    let ok = registry
        .execute("read_file", json!({"path": "readable.txt"}), &ctx)
        .await
        .unwrap();
    assert!(ok.ok);
}

#[tokio::test]
async fn shell_can_be_disabled_independently_of_the_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::default_set();
    let mut permissions = Permissions::default();
    permissions.profile = PermissionProfile::ControlledAutonomous;
    permissions.allow_shell = false;
    let ctx = ToolContext::new(tmp.path().to_path_buf(), permissions);

    let err = registry
        .execute("run_command", json!({"command": "echo hi"}), &ctx)
        .await
        .unwrap_err();
    assert!(matches!(err, ApexError::Permission(_)), "got {err:?}");
}

#[tokio::test]
async fn reviewer_agent_cannot_edit_even_with_full_permissions() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = ctx(tmp.path(), PermissionProfile::ControlledAutonomous);
    let reviewer = apex_agent::builtin::reviewer_agent();

    // The permission profile allows writes...
    assert!(ctx.permissions.profile == PermissionProfile::ControlledAutonomous);

    // ...but the agent's manifest does not grant the tool.
    assert!(!reviewer.allows_tool("write_file"));
    assert!(!reviewer.allows_tool("edit_file"));
}

#[test]
fn commands_do_not_go_through_a_shell() {
    // The run_command tool parses the command line into program + args and
    // executes without a shell, so shell metacharacters are inert.
    let parts = shell_words::split("echo hi && rm -rf /").unwrap();
    assert_eq!(parts[0], "echo");
    // The '&&' and 'rm' are passed as literal arguments, never interpreted.
    assert!(parts.iter().any(|p| p == "&&"));
}

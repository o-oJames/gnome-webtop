//! End-to-end tests for the command line interface.
//!
//! They only exercise read-only commands so they are safe to run anywhere
//! (including inside CI containers without a display).

use std::path::PathBuf;
use std::process::Command;

/// Locate a cargo-built binary next to the test executable:
/// `target/<profile>/deps/cli-<hash>` → `target/<profile>/mount-manager`.
fn binary(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile = exe.parent()?.parent()?;
    let candidate = profile.join(name);
    candidate.is_file().then_some(candidate)
}

fn run(args: &[&str]) -> (bool, String, String) {
    let bin = binary("mount-manager").expect("the mount-manager binary must be built");
    let out = Command::new(bin)
        .args(args)
        .output()
        .expect("run mount-manager");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn help_lists_commands() {
    let (ok, stdout, _) = run(&["help"]);
    assert!(ok);
    for cmd in [
        "list",
        "devices",
        "shares",
        "mount",
        "umount",
        "fstab",
        "discover",
        "auto-mount",
        "info",
    ] {
        assert!(stdout.contains(cmd), "help is missing `{cmd}`:\n{stdout}");
    }
}

#[test]
fn version_reports_cargo_version() {
    let (ok, stdout, _) = run(&["--version"]);
    assert!(ok);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "{stdout}");
    assert!(stdout.contains("Mount Manager"), "{stdout}");
}

#[test]
fn unknown_command_fails_with_hint() {
    let (ok, _, stderr) = run(&["definitely-not-a-command"]);
    assert!(!ok);
    assert!(stderr.contains("Unknown command"), "{stderr}");
    assert!(stderr.contains("mount-manager help"), "{stderr}");
}

#[test]
fn list_produces_json() {
    let (ok, stdout, _) = run(&["list", "--all", "--json"]);
    assert!(ok);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let mounts = value.as_array().expect("array of mounts");
    // Every Linux system has a root filesystem mounted.
    assert!(
        mounts.iter().any(|m| m["target"] == "/"),
        "no root mount in {stdout}"
    );
    let root = mounts.iter().find(|m| m["target"] == "/").unwrap();
    assert!(root.get("fstype").is_some());
    assert!(root.get("kind").is_some());
}

#[test]
fn list_table_has_a_header() {
    let (ok, stdout, _) = run(&["list", "--all"]);
    assert!(ok);
    assert!(stdout.contains("SOURCE"), "{stdout}");
    assert!(stdout.contains("TARGET"), "{stdout}");
}

#[test]
fn devices_produces_json() {
    let (ok, stdout, _) = run(&["devices", "--all", "--json"]);
    assert!(ok);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(value.is_array());
}

#[test]
fn fstab_lists_entries() {
    let (ok, stdout, _) = run(&["fstab", "--json"]);
    assert!(ok);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(value.is_array());
}

#[test]
fn shares_round_trip_through_json() {
    let (ok, stdout, _) = run(&["shares", "--json"]);
    assert!(ok);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(value.is_array());
}

#[test]
fn info_reports_the_environment() {
    let (ok, stdout, _) = run(&["info"]);
    assert!(ok);
    assert!(stdout.contains("escalation"), "{stdout}");
    assert!(stdout.contains("helper"), "{stdout}");
    assert!(stdout.contains("tools"), "{stdout}");
}

#[test]
fn mount_without_source_explains_itself() {
    let (ok, _, stderr) = run(&["mount"]);
    assert!(!ok);
    assert!(stderr.contains("Nothing to mount"), "{stderr}");
}

#[test]
fn umount_of_unknown_target_fails() {
    let (ok, _, stderr) = run(&["umount", "/mnt/not-mounted-anywhere"]);
    assert!(!ok);
    assert!(stderr.contains("not mounted"), "{stderr}");
}

#[test]
fn refuses_dangerous_mount_targets() {
    let (ok, _, stderr) = run(&[
        "mount",
        "--protocol",
        "smb",
        "--host",
        "nas",
        "--path",
        "data",
        "--target",
        "/usr",
        "--escalate",
        "never",
    ]);
    assert!(!ok);
    assert!(stderr.contains("protected"), "{stderr}");
}

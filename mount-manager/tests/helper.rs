//! End-to-end tests for the privileged helper's protocol.
//!
//! These run the real binary through a pipe, exactly like `pkexec`/`sudo` do.
//! Nothing here requires root: the checks are about the *interface* and about
//! the helper refusing to act when it is not privileged.

use mount_manager::ops::*;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Locate a cargo-built binary next to the test executable.
fn binary(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile = exe.parent()?.parent()?;
    let candidate = profile.join(name);
    candidate.is_file().then_some(candidate)
}

fn helper() -> Option<PathBuf> {
    binary("mount-manager-helper")
}

/// Send one operation and parse the JSON answer.
fn call(op: &Op) -> (i32, OpResult) {
    let mut child = Command::new(helper().expect("the helper binary must be built"))
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("helper binary must exist");
    let payload = serde_json::to_string(op).unwrap();
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let result: OpResult =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("bad JSON {stdout:?}: {e}"));
    (out.status.code().unwrap_or(-1), result)
}

fn caller() -> CallerSpec {
    CallerSpec {
        uid: 1000,
        gid: 1000,
        name: "tester".into(),
        home: "/home/tester".into(),
    }
}

#[test]
fn prints_version() {
    let Some(helper) = helper() else { return };
    let out = Command::new(helper).arg("--version").output().expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("mount-manager-helper"), "{text}");
}

#[test]
fn refuses_without_stdin_flag() {
    let Some(helper) = helper() else { return };
    let out = Command::new(helper).output().expect("run");
    assert_eq!(out.status.code(), Some(2));
    // Still valid JSON on stdout, so the caller can show a real error.
    let result: OpResult =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert!(!result.ok);
}

#[test]
fn answers_json_for_garbage_input() {
    let mut child = Command::new(helper().expect("the helper binary must be built"))
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{ not json")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let result: OpResult =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert!(!result.ok);
    assert!(result.message.contains("parsed"), "{}", result.message);
}

#[test]
fn probe_reports_privileges() {
    let (code, result) = call(&Op::Probe(ProbeOp {}));
    assert_eq!(code, 0, "{:?}", result);
    assert!(result.ok);
    assert!(result.message.contains("uid"), "{}", result.message);
}

/// A mount request must be refused unless we really are root — and even as root
/// it must refuse protected targets.
#[test]
fn refuses_to_mount_when_unprivileged() {
    if unsafe { libc::geteuid() } == 0 {
        // Running as root: the request must still fail, because /etc/fstab is a
        // protected target.
        let op = Op::Mount(Box::new(MountOp {
            target: "/etc/fstab".into(),
            create_target: false,
            attempts: vec![MountAttempt::new("//nas/data").fstype("cifs")],
            post: None,
            creds: None,
            persist: None,
            bookmark: None,
            chown_target: None,
            verify: false,
            caller: caller(),
        }));
        let (_, result) = call(&op);
        assert!(!result.ok);
        assert!(
            result.message.to_lowercase().contains("protected"),
            "{}",
            result.message
        );
        return;
    }
    let op = Op::Mount(Box::new(MountOp {
        target: "/mnt/should-not-exist".into(),
        create_target: true,
        attempts: vec![MountAttempt::new("//nas/data").fstype("cifs")],
        post: None,
        creds: None,
        persist: None,
        bookmark: None,
        chown_target: None,
        verify: false,
        caller: caller(),
    }));
    let (code, result) = call(&op);
    assert_ne!(code, 0);
    assert!(!result.ok);
    assert!(
        result.message.contains("administrator rights"),
        "expected a clear rights error, got: {}",
        result.message
    );
    assert!(!std::path::Path::new("/mnt/should-not-exist").exists());
}

#[test]
fn refuses_secret_files_outside_its_directories() {
    if unsafe { libc::geteuid() } == 0 {
        return; // the non-root path is what we want to exercise
    }
    let op = Op::WriteSecret(WriteSecretOp {
        creds: CredSpec {
            kind: CredKind::Generic,
            path: "/root/.ssh/authorized_keys".into(),
            contents: "ssh-rsa AAAA attacker".into(),
            merge_url: None,
            keep: true,
            extra_options: Vec::new(),
        },
        caller: caller(),
    });
    let (_, result) = call(&op);
    assert!(!result.ok);
}

#[test]
fn umount_of_unknown_target_fails_cleanly() {
    let op = Op::Umount(UmountOp {
        target: "/mnt/definitely-not-mounted".into(),
        lazy: false,
        force: false,
        remove_fstab: false,
        remove_bookmark: None,
        remove_dir_if_empty: false,
        expect_source: None,
        cleanup: Vec::new(),
        caller: caller(),
    });
    let (_, result) = call(&op);
    assert!(!result.ok);
}

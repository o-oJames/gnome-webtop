//! The privileged executor: the only code that touches `mount(8)`, `umount(8)`
//! and `/etc/fstab`.
//!
//! It runs either
//! * in-process, when Mount Manager already has root rights, or
//! * inside `mount-manager-helper`, launched through polkit (`pkexec`) or
//!   `sudo` — see [`crate::privilege`].
//!
//! Every request is re-validated here: the UI is *not* trusted, because a
//! compromised session process must not be able to talk the helper into
//! unmounting `/` or scribbling over `/etc/fstab`.

use crate::error::Error;
use crate::exec::{self, Cmd};
use crate::fstab;
use crate::mounts;
use crate::ops::*;
use crate::platform;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MOUNT_TIMEOUT: Duration = Duration::from_secs(90);
const UMOUNT_TIMEOUT: Duration = Duration::from_secs(45);

/// Directories whose (empty) mount points we are allowed to remove again.
const REMOVABLE_MOUNT_ROOTS: &[&str] = &["/media", "/mnt", "/run/media"];

/// Directories that may hold root owned secrets written by this app.
/// `/etc/davfs2` is included because davfs2 only reads credentials from there.
const SECRET_ROOTS: &[&str] = &["/etc/mount-manager", "/run/mount-manager", "/etc/davfs2"];

/// Entry point used by both the helper binary and the in-process path.
pub fn execute(op: &Op) -> OpResult {
    if !platform::is_root() && !op.is_read_only() {
        return OpResult::fail(format!(
            "{} needs administrator rights (running as uid {})",
            op.summary(),
            platform::euid()
        ));
    }
    match op {
        Op::Probe(_) => probe(),
        Op::Mount(m) => mount(m),
        Op::Umount(u) => umount(u),
        Op::MakeDir(d) => make_dir(d),
        Op::FstabAdd(f) => fstab_add(f),
        Op::FstabRemove(f) => fstab_remove(f),
        Op::RemoveSecrets(r) => remove_secrets(r),
        Op::WriteSecret(w) => write_secret_op(w),
        Op::FstabRestore(_) => fstab_restore(),
    }
}

fn probe() -> OpResult {
    let caller = platform::Caller::from_env_if_root(&platform::Caller::current());
    OpResult::ok(format!(
        "running as uid {} ({})",
        platform::euid(),
        caller.name
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// mount
// ─────────────────────────────────────────────────────────────────────────────

fn mount(op: &MountOp) -> OpResult {
    let mut log: Vec<String> = Vec::new();

    let target = match validate_target(&op.target) {
        Ok(t) => t,
        Err(e) => return OpResult::fail(e.message).with_log(log),
    };
    if op.attempts.is_empty() {
        return OpResult::fail("No mount attempt was provided".to_string()).with_log(log);
    }
    log.push(format!("target: {target}"));

    // 1. mount point -------------------------------------------------------
    if op.create_target {
        if let Err(e) = ensure_dir(&target, op.caller.uid, op.caller.gid, 0o755) {
            return OpResult::fail(e.message).with_log(log);
        }
        log.push(format!("mount point ready: {target}"));
    } else if !Path::new(&target).is_dir() {
        return OpResult::fail(format!("Mount point `{target}` does not exist")).with_log(log);
    }

    // Already mounted? Refuse to stack a second mount silently.
    let live = mounts::read_mounts().unwrap_or_default();
    if let Some(existing) = mounts::find_by_target(&live, &target) {
        return OpResult::fail(format!(
            "`{target}` already has {} mounted on it",
            existing.source
        ))
        .with_log(log);
    }

    // 2. credentials file (root owned, mode 0600) --------------------------
    let mut extra_options: Vec<String> = Vec::new();
    if let Some(creds) = &op.creds {
        match write_secret(creds, &mut log) {
            Ok(extra) => extra_options = extra,
            Err(e) => return OpResult::fail(e.message).with_log(log),
        }
    }

    // 3. try each attempt until one works ----------------------------------
    let mut last_error = String::new();
    let mut succeeded: Option<String> = None;
    for attempt in &op.attempts {
        let mut options = attempt.options.clone();
        options.extend(extra_options.iter().cloned());
        let options = crate::model::dedupe_options(options);
        let mut argv = attempt.argv(&target);
        if !options.is_empty() {
            // Rebuild with the merged option list.
            argv = Vec::new();
            argv.push("mount".to_string());
            argv.extend(attempt.flags.clone());
            if let Some(t) = &attempt.fstype {
                argv.push("-t".to_string());
                argv.push(t.clone());
            }
            argv.push("-o".to_string());
            argv.push(options.join(","));
            argv.push(attempt.source.clone());
            argv.push(target.clone());
        }
        let cmd = Cmd::new(&argv[0]).args(&argv[1..]).timeout(MOUNT_TIMEOUT);
        log.push(format!("$ {}", mask_secret_argv(&cmd.render())));
        match exec::run(&cmd) {
            Ok(out) if out.ok() => {
                succeeded = Some(attempt.label.clone().unwrap_or_else(|| "mount".to_string()));
                break;
            }
            Ok(out) => {
                let detail = out.combined();
                last_error = detail.clone();
                log.push(format!("failed: {}", one_line(&detail)));
                if out.timed_out {
                    // A hung network mount will not recover by retrying.
                    break;
                }
            }
            Err(e) => {
                last_error = e.message.clone();
                log.push(format!("failed: {}", e.message));
            }
        }
    }

    let Some(used) = succeeded else {
        // Clean up the temporary credential file before giving up.
        if let Some(creds) = &op.creds {
            if !creds.keep {
                let _ = remove_if_allowed(&creds.path, &mut log);
            }
        }
        let message = friendly_mount_error(&last_error, op);
        return OpResult {
            ok: false,
            message,
            detail: Some(last_error),
            log,
        };
    };
    log.push(format!("mounted using {used}"));

    // 4. post mount fixups -------------------------------------------------
    match &op.post {
        Some(PostStep::RemountRo) => {
            let out = exec::run(
                &Cmd::new("mount")
                    .args(["-o", "remount,ro", &target])
                    .timeout(Duration::from_secs(20)),
            );
            match out {
                Ok(o) if o.ok() => log.push("remounted read-only".to_string()),
                Ok(o) => log.push(format!("remount,ro failed: {}", one_line(&o.combined()))),
                Err(e) => log.push(format!("remount,ro failed: {}", e.message)),
            }
        }
        Some(PostStep::ChownTarget(owner)) => {
            if let Err(e) = chown(&target, owner.uid, owner.gid) {
                log.push(format!("chown failed: {}", e.message));
            } else {
                log.push(format!("ownership set to {}:{}", owner.uid, owner.gid));
            }
        }
        None => {}
    }
    if let Some(owner) = &op.chown_target {
        if let Err(e) = chown(&target, owner.uid, owner.gid) {
            log.push(format!("chown failed: {}", e.message));
        }
    }

    // 5. verify ------------------------------------------------------------
    if op.verify {
        let live = mounts::read_mounts().unwrap_or_default();
        if mounts::find_by_target(&live, &target).is_none() {
            return OpResult {
                ok: false,
                message: format!("`mount` reported success but `{target}` is still not mounted"),
                detail: Some(log.join("\n")),
                log,
            };
        }
    }

    // 6. persistence (/etc/fstab) ------------------------------------------
    if let Some(persist) = &op.persist {
        match fstab::add(&persist.target, &persist.lines, true) {
            Ok(()) => log.push(format!("added /etc/fstab entry for {}", persist.target)),
            Err(e) => log.push(format!("fstab update failed: {}", e.message)),
        }
    }

    // 7. Files sidebar bookmark --------------------------------------------
    if let Some(bm) = &op.bookmark {
        match add_bookmark(bm) {
            Ok(true) => log.push(format!("added bookmark {}", bm.uri)),
            Ok(false) => log.push("bookmark already present".to_string()),
            Err(e) => log.push(format!("bookmark failed: {}", e.message)),
        }
    }

    // 8. temporary credential file can go away now -------------------------
    if let Some(creds) = &op.creds {
        if !creds.keep {
            let _ = remove_if_allowed(&creds.path, &mut log);
        }
    }

    let source = op
        .attempts
        .first()
        .map(|a| a.source.clone())
        .unwrap_or_default();
    OpResult {
        ok: true,
        message: format!("Mounted {source} on {target}"),
        detail: None,
        log,
    }
}

/// Turn common `mount(8)` stderr into something a human can act on.
fn friendly_mount_error(stderr: &str, op: &MountOp) -> String {
    let s = stderr.to_lowercase();
    let hint = if s.contains("permission denied") && s.contains("cifs") {
        "Wrong user name, password or domain for this SMB share."
    } else if s.contains("nt_status_logon_failure") || s.contains("logon failure") {
        "The server rejected these credentials."
    } else if s.contains("access denied") {
        "The server refused access — check the share/export permissions and your IP."
    } else if s.contains("no route to host") || s.contains("network is unreachable") {
        "The server is not reachable from this network."
    } else if s.contains("connection timed out") || s.contains("timed out") {
        "The server did not answer in time — check its firewall and the port."
    } else if s.contains("connection refused") {
        "The server refused the connection — is the service running?"
    } else if s.contains("bad security") || s.contains("mount error(95)") {
        "The server does not support the requested SMB dialect; try `vers=1.0` in Advanced options."
    } else if s.contains("mount error(112)") || s.contains("host is down") {
        "The server only speaks SMB1, which this kernel disabled. Add `vers=1.0` in Advanced options."
    } else if s.contains("access denied by server while mounting")
        || s.contains("mount.nfs: access denied")
    {
        "NFS refused this client — the export list probably does not include your address."
    } else if s.contains("mount.nfs: requested nfs version is not enabled") {
        "The requested NFS version is not enabled on the server (or missing client support)."
    } else if s.contains("mount.nfs: protocol not supported") {
        "NFS client support is missing."
    } else if s.contains("no such device") {
        "The kernel does not know this filesystem type — the client package is missing."
    } else if s.contains("wrong fs type") || s.contains("invalid argument") {
        "Wrong filesystem type or invalid mount options."
    } else if s.contains("special device") && s.contains("does not exist") {
        "The device or share does not exist."
    } else if s.contains("transport endpoint is not connected") {
        "A stale FUSE mount is in the way — unmount it lazily first."
    } else if s.contains("operation not permitted") {
        "Not permitted — FUSE may need `user_allow_other` in /etc/fuse.conf."
    } else if s.contains("mount point does not exist") {
        "The mount point could not be created."
    } else if s.contains("permission denied") {
        "Permission denied."
    } else if s.is_empty() {
        "The mount command did not report why it failed."
    } else {
        "The mount command failed."
    };
    format!("{hint} ({})", op.target)
}

// ─────────────────────────────────────────────────────────────────────────────
// umount
// ─────────────────────────────────────────────────────────────────────────────

fn umount(op: &UmountOp) -> OpResult {
    let mut log: Vec<String> = Vec::new();
    let target = match validate_target(&op.target) {
        Ok(t) => t,
        Err(e) => return OpResult::fail(e.message),
    };

    let live = mounts::read_mounts().unwrap_or_default();
    let Some(entry) = mounts::find_by_target(&live, &target) else {
        return OpResult::fail(format!("`{target}` is not mounted")).with_log(log);
    };
    if let Some(expected) = &op.expect_source {
        if !expected.is_empty() && entry.source != *expected {
            return OpResult::fail(format!(
                "`{target}` now holds {} instead of {} — refresh the list and try again",
                entry.source, expected
            ));
        }
    }
    log.push(format!(
        "unmounting {} ({}) from {target}",
        entry.source, entry.fstype
    ));

    let mut argv = vec!["umount".to_string()];
    if op.force {
        argv.push("-f".to_string());
    }
    if op.lazy {
        argv.push("-l".to_string());
    }
    argv.push(target.clone());
    let cmd = Cmd::new(&argv[0]).args(&argv[1..]).timeout(UMOUNT_TIMEOUT);
    log.push(format!("$ {}", cmd.render()));
    let out = exec::run(&cmd);

    match out {
        Ok(o) if o.ok() => log.push("unmounted".to_string()),
        Ok(o) => {
            let detail = o.combined();
            log.push(format!("failed: {}", one_line(&detail)));
            let busy = detail.contains("busy") || detail.contains("target is busy");
            let holders = if busy {
                mounts::busy_holders(&target)
            } else {
                String::new()
            };
            let mut message = if busy {
                format!("`{target}` is busy — a program is still using it")
            } else {
                format!("Could not unmount `{target}`")
            };
            if busy && !op.lazy {
                message.push_str(". Close the files, or use “Unmount anyway (lazy)”.");
            }
            let detail = if holders.is_empty() {
                detail
            } else {
                format!("{detail}\n\nProcesses using it:\n{holders}")
            };
            return OpResult {
                ok: false,
                message,
                detail: Some(detail),
                log,
            };
        }
        Err(e) => {
            log.push(format!("failed: {}", e.message));
            return OpResult {
                ok: false,
                message: e.message.clone(),
                detail: e.detail.clone(),
                log,
            };
        }
    }

    // Verify.
    let live = mounts::read_mounts().unwrap_or_default();
    if mounts::find_by_target(&live, &target).is_some() && !op.lazy {
        return OpResult {
            ok: false,
            message: format!("`{target}` is still mounted"),
            detail: None,
            log,
        };
    }

    // Clean up what we created for this mount.
    if op.remove_fstab {
        match fstab::remove(&target, true) {
            Ok(true) => log.push("removed /etc/fstab entry".to_string()),
            Ok(false) => log.push("no /etc/fstab entry to remove".to_string()),
            Err(e) => log.push(format!("fstab update failed: {}", e.message)),
        }
    }
    if let Some(uri) = &op.remove_bookmark {
        match remove_bookmark(uri, &op.caller) {
            Ok(removed) => log.push(format!("bookmark removal: {removed}")),
            Err(e) => log.push(format!("bookmark failed: {}", e.message)),
        }
    }
    for path in &op.cleanup {
        let _ = remove_if_allowed(path, &mut log);
    }
    if op.remove_dir_if_empty {
        if let Err(e) = remove_empty_mountpoint(&target) {
            log.push(format!("kept mount point: {}", e.message));
        } else {
            log.push(format!("removed empty mount point {target}"));
        }
    }

    OpResult {
        ok: true,
        message: format!("Unmounted {target}"),
        detail: None,
        log,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// supporting operations
// ─────────────────────────────────────────────────────────────────────────────

fn make_dir(op: &MakeDirOp) -> OpResult {
    let mut log = Vec::new();
    for spec in &op.paths {
        let path = match platform::normalize_absolute(&spec.path) {
            Ok(p) => p,
            Err(e) => return OpResult::fail(e.message),
        };
        if let Err(e) = ensure_dir(&path.to_string_lossy(), spec.uid, spec.gid, spec.mode) {
            return OpResult {
                ok: false,
                message: e.message,
                detail: e.detail,
                log,
            };
        }
        log.push(format!("created {}", path.display()));
    }
    OpResult {
        ok: true,
        message: format!("Created {} director(y/ies)", op.paths.len()),
        detail: None,
        log,
    }
}

fn fstab_add(op: &FstabOp) -> OpResult {
    if op.lines.is_empty() {
        return OpResult::fail("Nothing to add to /etc/fstab".to_string());
    }
    match fstab::add(&op.target, &op.lines, op.backup) {
        Ok(()) => OpResult::ok(format!("Added /etc/fstab entry for {}", op.target)),
        Err(e) => OpResult {
            ok: false,
            message: e.message,
            detail: e.detail,
            log: Vec::new(),
        },
    }
}

fn fstab_remove(op: &FstabOp) -> OpResult {
    match fstab::remove(&op.target, op.backup) {
        Ok(true) => OpResult::ok(format!("Removed /etc/fstab entry for {}", op.target)),
        Ok(false) => OpResult::fail(format!("There is no /etc/fstab entry for {}", op.target)),
        Err(e) => OpResult {
            ok: false,
            message: e.message,
            detail: e.detail,
            log: Vec::new(),
        },
    }
}

fn write_secret_op(op: &WriteSecretOp) -> OpResult {
    let mut log = Vec::new();
    match write_secret(&op.creds, &mut log) {
        Ok(_) => OpResult {
            ok: true,
            message: format!("Stored credentials in {}", op.creds.path),
            detail: None,
            log,
        },
        Err(e) => OpResult {
            ok: false,
            message: e.message,
            detail: e.detail,
            log,
        },
    }
}

fn fstab_restore() -> OpResult {
    match fstab::restore_backup() {
        Ok(()) => OpResult::ok(format!("Restored /etc/fstab from {}", fstab::BACKUP_PATH)),
        Err(e) => OpResult {
            ok: false,
            message: e.message,
            detail: e.detail,
            log: Vec::new(),
        },
    }
}

fn remove_secrets(op: &RemoveSecretsOp) -> OpResult {
    let mut log = Vec::new();
    for path in &op.paths {
        match remove_if_allowed(path, &mut log) {
            Ok(true) => {}
            Ok(false) => log.push(format!("not found: {path}")),
            Err(e) => {
                return OpResult {
                    ok: false,
                    message: e.message,
                    detail: e.detail,
                    log,
                }
            }
        }
    }
    OpResult {
        ok: true,
        message: format!("Removed {} secret file(s)", op.paths.len()),
        detail: None,
        log,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Validate a mount/unmount target.
fn validate_target(target: &str) -> Result<String, Error> {
    let path = platform::normalize_absolute(target)?;
    let s = path.to_string_lossy().to_string();
    if platform::is_protected_target(&s) {
        return Err(Error::new(format!(
            "Refusing to touch `{s}`: it is a protected system path"
        )));
    }
    Ok(s)
}

/// Create a directory (with parents) and set owner/mode.
fn ensure_dir(path: &str, uid: u32, gid: u32, mode: u32) -> Result<(), Error> {
    let p = Path::new(path);
    if p.exists() {
        if !p.is_dir() {
            return Err(Error::new(format!(
                "`{path}` exists but is not a directory"
            )));
        }
        return Ok(());
    }
    fs::create_dir_all(p)
        .map_err(|e| Error::with_detail(format!("Cannot create `{path}`"), e.to_string()))?;
    set_mode(p, mode);
    chown(path, uid, gid)?;
    Ok(())
}

fn chown(path: &str, uid: u32, gid: u32) -> Result<(), Error> {
    let c = std::ffi::CString::new(path).map_err(|e| Error::new(e.to_string()))?;
    let rc = unsafe { libc::chown(c.as_ptr(), uid, gid) };
    if rc == 0 {
        Ok(())
    } else {
        Err(Error::with_detail(
            format!("Cannot change the owner of `{path}`"),
            std::io::Error::last_os_error().to_string(),
        ))
    }
}

fn set_mode(path: &Path, mode: u32) {
    if let Ok(c) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) {
        unsafe {
            libc::chmod(c.as_ptr(), mode);
        }
    }
}

/// Write a root owned credential file. Returns extra mount options.
fn write_secret(creds: &CredSpec, log: &mut Vec<String>) -> Result<Vec<String>, Error> {
    let path = platform::normalize_absolute(&creds.path)?;
    let path_str = path.to_string_lossy().to_string();
    if !SECRET_ROOTS
        .iter()
        .any(|root| path_str.starts_with(&format!("{root}/")))
    {
        return Err(Error::new(format!(
            "Refusing to write a credential file outside {}",
            SECRET_ROOTS.join(" or ")
        )));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            Error::with_detail(format!("Cannot create {}", parent.display()), e.to_string())
        })?;
        set_mode(parent, 0o700);
    }

    match creds.kind {
        CredKind::Davfs => {
            // davfs2 keeps one shared secrets file; merge instead of overwrite.
            let mut lines: Vec<String> = fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .collect();
            if let Some(url) = &creds.merge_url {
                lines.retain(|l| !l.trim().starts_with(url));
            }
            lines.extend(creds.contents.lines().map(String::from));
            write_private(&path, &format!("{}\n", lines.join("\n")))?;
            log.push(format!("updated davfs2 secrets {path_str}"));
        }
        _ => {
            write_private(&path, &creds.contents)?;
            log.push(format!("wrote credentials {path_str}"));
        }
    }
    Ok(creds.extra_options.clone())
}

/// Write a file with mode 0600 owned by root.
fn write_private(path: &Path, contents: &str) -> Result<(), Error> {
    fs::write(path, contents.as_bytes()).map_err(|e| {
        Error::with_detail(format!("Cannot write {}", path.display()), e.to_string())
    })?;
    set_mode(path, 0o600);
    let c = std::ffi::CString::new(path.to_string_lossy().as_bytes())
        .map_err(|e| Error::new(e.to_string()))?;
    unsafe {
        libc::chown(c.as_ptr(), 0, 0);
    }
    Ok(())
}

/// Delete a file, but only inside the directories this app owns.
fn remove_if_allowed(path: &str, log: &mut Vec<String>) -> Result<bool, Error> {
    let p = platform::normalize_absolute(path)?;
    let s = p.to_string_lossy().to_string();
    if !SECRET_ROOTS
        .iter()
        .any(|root| s.starts_with(&format!("{root}/")))
    {
        return Err(Error::new(format!(
            "Refusing to delete `{s}`: outside {}",
            SECRET_ROOTS.join(" or ")
        )));
    }
    match fs::remove_file(&p) {
        Ok(()) => {
            log.push(format!("removed {s}"));
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::with_detail(
            format!("Cannot delete `{s}`"),
            e.to_string(),
        )),
    }
}

/// Remove an empty mount point below /media, /mnt or /run/media.
fn remove_empty_mountpoint(target: &str) -> Result<(), Error> {
    let p = PathBuf::from(target);
    if !REMOVABLE_MOUNT_ROOTS
        .iter()
        .any(|root| target.starts_with(&format!("{root}/")))
    {
        return Err(Error::new(format!(
            "{target} is not below {}",
            REMOVABLE_MOUNT_ROOTS.join(", ")
        )));
    }
    if target == "/media" || REMOVABLE_MOUNT_ROOTS.contains(&target) {
        return Err(Error::new(
            "refusing to remove a standard mount root".to_string(),
        ));
    }
    match fs::read_dir(&p) {
        Ok(mut rd) => {
            if rd.next().is_some() {
                return Err(Error::new("not empty".to_string()));
            }
        }
        Err(_) => return Err(Error::new("cannot read directory".to_string())),
    }
    fs::remove_dir(&p)
        .map_err(|e| Error::with_detail(format!("Cannot remove `{target}`"), e.to_string()))
}

/// Append `uri title` to the user's `~/.gtk-bookmarks`.
fn add_bookmark(bm: &BookmarkSpec) -> Result<bool, Error> {
    let path = Path::new(&bm.home).join(".gtk-bookmarks");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing
        .lines()
        .any(|l| l.split_whitespace().next() == Some(bm.uri.as_str()))
    {
        return Ok(false);
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("{} {}\n", bm.uri, bm.title.trim()));
    fs::write(&path, text.as_bytes()).map_err(|e| {
        Error::with_detail(format!("Cannot write {}", path.display()), e.to_string())
    })?;
    let c = std::ffi::CString::new(path.to_string_lossy().as_bytes())
        .map_err(|e| Error::new(e.to_string()))?;
    unsafe {
        libc::chown(c.as_ptr(), bm.uid, bm.gid);
        libc::chmod(c.as_ptr(), 0o600);
    }
    Ok(true)
}

/// Remove a bookmark line again.
fn remove_bookmark(uri: &str, caller: &CallerSpec) -> Result<String, Error> {
    let home = if caller.home.is_empty() {
        platform::home_for(&caller.name).unwrap_or_else(|| PathBuf::from("/root"))
    } else {
        PathBuf::from(&caller.home)
    };
    let path = home.join(".gtk-bookmarks");
    let Ok(existing) = fs::read_to_string(&path) else {
        return Ok("no bookmark file".to_string());
    };
    let kept: Vec<&str> = existing
        .lines()
        .filter(|l| l.split_whitespace().next() != Some(uri))
        .collect();
    if kept.len() as usize == existing.lines().count() {
        return Ok("bookmark not present".to_string());
    }
    fs::write(&path, (kept.join("\n") + "\n").as_bytes()).map_err(|e| {
        Error::with_detail(format!("Cannot write {}", path.display()), e.to_string())
    })?;
    Ok(format!("removed bookmark {uri}"))
}

/// Hide credential values inside an argv string before it reaches a log.
///
/// Only the secret *values* are replaced, so the rest of the option list stays
/// readable: `-o password=hunter2,uid=1000` → `-o password=***,uid=1000`.
fn mask_secret_argv(rendered: &str) -> String {
    rendered
        .split_whitespace()
        .map(mask_option_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn mask_option_token(token: &str) -> String {
    if !token.contains("pass") && !token.contains("secret") && !token.contains("user") {
        return token.to_string();
    }
    token
        .split(',')
        .map(|opt| match opt.split_once('=') {
            Some((key, value)) => match key.to_ascii_lowercase().as_str() {
                "password" | "pass" | "secret" => format!("{key}=***"),
                // curlftpfs passes `user=name:password`.
                "user" if value.contains(':') => {
                    let (name, _) = value.split_once(':').unwrap_or((value, ""));
                    format!("user={name}:***")
                }
                _ => opt.to_string(),
            },
            None => opt.to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Collapse newlines so one failure stays one log line.
fn one_line(text: &str) -> String {
    let t = text.trim().replace('\n', " | ");
    if t.len() > 400 {
        format!(
            "{}…",
            &t[..t
                .char_indices()
                .take_while(|(i, _)| *i < 400)
                .last()
                .map(|(i, _)| i)
                .unwrap_or(t.len())]
        )
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_protected_targets() {
        assert!(validate_target("/").is_err());
        assert!(validate_target("/usr").is_err());
        assert!(validate_target("/etc/fstab").is_err());
        assert!(validate_target("relative").is_err());
        assert_eq!(validate_target("/mnt/nas/").unwrap(), "/mnt/nas");
    }

    #[test]
    fn masks_passwords_in_logs() {
        assert_eq!(
            mask_secret_argv("mount -t cifs -o password=hunter2,uid=1000 //h/s /mnt/x"),
            "mount -t cifs -o password=***,uid=1000 //h/s /mnt/x"
        );
        assert_eq!(
            mask_secret_argv(
                "mount -t fuse.curlftpfs -o user=jo:hunter2,uid=1000 curlftpfs#ftp://h /mnt/x"
            ),
            "mount -t fuse.curlftpfs -o user=jo:***,uid=1000 curlftpfs#ftp://h /mnt/x"
        );
        assert_eq!(mask_secret_argv("umount /mnt/plain"), "umount /mnt/plain");
    }

    #[test]
    fn friendly_errors_are_actionable() {
        let op = MountOp {
            target: "/mnt/nas".into(),
            create_target: true,
            attempts: vec![MountAttempt::new("//h/s").fstype("cifs")],
            post: None,
            creds: None,
            persist: None,
            bookmark: None,
            chown_target: None,
            verify: true,
            caller: CallerSpec {
                uid: 1000,
                gid: 1000,
                name: "jo".into(),
                home: "/home/jo".into(),
            },
        };
        let msg = friendly_mount_error("mount error(13): Permission denied", &op);
        assert!(
            msg.contains("credentials") || msg.contains("Permission"),
            "{msg}"
        );
        assert!(msg.contains("/mnt/nas"));
        let msg = friendly_mount_error("mount error(112): Host is down", &op);
        assert!(msg.contains("vers=1.0"), "{msg}");
    }

    #[test]
    fn refuses_to_delete_files_outside_secret_roots() {
        let mut log = Vec::new();
        assert!(remove_if_allowed("/etc/passwd", &mut log).is_err());
        assert!(remove_if_allowed("/etc/mount-manager/credentials/nope.cred", &mut log).is_ok());
    }

    #[test]
    fn refuses_to_write_secrets_elsewhere() {
        let mut log = Vec::new();
        let creds = CredSpec {
            kind: CredKind::Cifs,
            path: "/root/.ssh/authorized_keys".into(),
            contents: "x".into(),
            merge_url: None,
            keep: false,
            extra_options: Vec::new(),
        };
        assert!(write_secret(&creds, &mut log).is_err());
    }

    #[test]
    fn refuses_to_remove_standard_mount_roots() {
        assert!(remove_empty_mountpoint("/media").is_err());
        assert!(remove_empty_mountpoint("/etc/foo").is_err());
    }

    #[test]
    fn one_line_collapses() {
        assert_eq!(one_line("a\nb\n"), "a | b");
        assert!(one_line(&"x".repeat(900)).ends_with('…'));
    }
}

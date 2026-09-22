//! High level API used by both the GTK front-end and the CLI.
//!
//! The engine owns the configuration, the escalation [`Runner`] and the
//! activity log. Every method is safe to call from a worker thread: nothing
//! here touches GTK.

use crate::config::AppConfig;
use crate::error::{Error, Result};
use crate::exec::{self, Cmd};
use crate::model::*;
use crate::ops::*;
use crate::privilege::{Method, PromptContext, Prompter, Runner};
use crate::{devices, discover, fstab, mounts, platform, secret, STATE_DIR};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Everything the app can do, without any toolkit involved.
pub struct Engine {
    runner: Runner,
    config: Mutex<AppConfig>,
    log: Mutex<Vec<LogEntry>>,
    prompter: Arc<dyn Prompter>,
    caller: platform::Caller,
}

impl Engine {
    pub fn new(prompter: Arc<dyn Prompter>) -> Arc<Self> {
        let config = AppConfig::load();
        let runner = Runner::new(config.escalation);
        Arc::new(Self {
            runner,
            config: Mutex::new(config),
            log: Mutex::new(Vec::new()),
            prompter,
            caller: platform::Caller::current(),
        })
    }

    /// The user this app acts for.
    pub fn caller(&self) -> &platform::Caller {
        &self.caller
    }

    // ── configuration ───────────────────────────────────────────────────────

    pub fn config(&self) -> AppConfig {
        self.config.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Change the configuration and persist it.
    pub fn update_config<F: FnOnce(&mut AppConfig)>(&self, f: F) -> Result<()> {
        let mut guard = self
            .config
            .lock()
            .map_err(|_| Error::new("Configuration is locked"))?;
        f(&mut guard);
        let snapshot = guard.clone();
        drop(guard);
        snapshot.save()?;
        // The escalation preference may have changed: forget the cached
        // detection so the next operation re-evaluates it.
        self.runner.set_pref(snapshot.escalation);
        Ok(())
    }

    pub fn shares(&self) -> Vec<SavedShare> {
        self.config().shares
    }

    pub fn escalation_description(&self) -> String {
        self.runner.description()
    }

    pub fn escalation_method(&self) -> Method {
        self.runner.method()
    }

    /// Absolute path of the privileged helper that will be used.
    pub fn helper_path(&self) -> PathBuf {
        self.runner.helper().clone()
    }

    /// Which password backend would be used right now.
    pub fn password_backend(&self) -> secret::Backend {
        self.config().password_backend()
    }

    pub fn forget_admin_password(&self) {
        self.runner.forget_password();
        self.push(
            LogLevel::Info,
            "Forgot the administrator password".to_string(),
            None,
        );
    }

    // ── read only views ─────────────────────────────────────────────────────

    /// All mounts, sorted for humans.
    pub fn all_mounts(&self) -> Result<Vec<MountEntry>> {
        mounts::list()
    }

    /// Mounts shown in the main list.
    pub fn visible_mounts(&self) -> Result<Vec<MountEntry>> {
        let cfg = self.config();
        Ok(self
            .all_mounts()?
            .into_iter()
            .filter(|m| cfg.show_system_mounts || m.kind.is_user_visible())
            .collect())
    }

    pub fn devices(&self) -> Result<Vec<BlockDevice>> {
        let all = self.config().show_all_devices;
        let list = devices::list()?;
        Ok(if all {
            list
        } else {
            list.into_iter().filter(|d| d.interesting()).collect()
        })
    }

    pub fn fstab_entries(&self) -> Result<Vec<fstab::FstabEntry>> {
        fstab::read()
    }

    /// Saved shares annotated with their current mount state.
    pub fn share_status(&self) -> Vec<(SavedShare, Option<MountEntry>)> {
        let live = self.all_mounts().unwrap_or_default();
        self.shares()
            .into_iter()
            .map(|s| {
                let mounted = live
                    .iter()
                    .find(|m| m.target == s.request.mount_point)
                    .cloned();
                (s, mounted)
            })
            .collect()
    }

    // ── discovery / testing ─────────────────────────────────────────────────

    pub fn discover(&self, protocol: Protocol) -> Vec<discover::Service> {
        discover::discover_servers(protocol)
    }

    pub fn smb_shares(
        &self,
        host: &str,
        user: &str,
        password: &str,
    ) -> Result<Vec<discover::SmbShare>> {
        discover::smb_shares(host, user, password, Duration::from_secs(12))
    }

    pub fn nfs_exports(&self, host: &str) -> Result<Vec<discover::NfsExport>> {
        discover::nfs_exports(host, Duration::from_secs(12))
    }

    pub fn test_connection(&self, request: ShareRequest) -> Result<String> {
        let request = self.prefill(request)?;
        let report = discover::test_request(&request)?;
        self.push(
            LogLevel::Info,
            format!("Tested {}", request.display_name()),
            Some(report.clone()),
        );
        Ok(report)
    }

    // ── mounting ────────────────────────────────────────────────────────────

    /// Fill in defaults, resolve a stored password and validate.
    fn prefill(&self, mut request: ShareRequest) -> Result<ShareRequest> {
        request = request.normalize(&self.caller)?;
        if request.password.is_empty() {
            let id = request.id();
            if let Some(share) = self.config().shares.iter().find(|s| s.id == id) {
                if let Some(reference) = &share.password_ref {
                    if let Some(pw) = secret::resolve(reference) {
                        request.password = pw;
                    }
                }
            }
        }
        Ok(request)
    }

    /// Decide which mount mechanism to use.
    pub fn resolve_method(&self, request: &ShareRequest) -> MountMethod {
        if request.method != MountMethod::Auto {
            return request.method;
        }
        let can_root = self.runner.method() != Method::None;
        match request.protocol {
            Protocol::Cifs if !can_root && exec::exists("gio") => MountMethod::Gvfs,
            Protocol::Sshfs => {
                if exec::exists("sshfs") {
                    MountMethod::FuseUser
                } else if exec::exists("gio") {
                    MountMethod::Gvfs
                } else {
                    MountMethod::System
                }
            }
            Protocol::WebDav => {
                if exec::exists("gio") {
                    MountMethod::Gvfs
                } else {
                    MountMethod::System
                }
            }
            Protocol::Ftp => {
                if exec::exists("gio") {
                    MountMethod::Gvfs
                } else if exec::exists("curlftpfs") {
                    MountMethod::FuseUser
                } else {
                    MountMethod::System
                }
            }
            Protocol::Cifs
            | Protocol::Nfs
            | Protocol::Block
            | Protocol::Bind
            | Protocol::Custom => MountMethod::System,
        }
    }

    /// Mount a share. Returns a human readable success message.
    pub fn mount(&self, request: ShareRequest, save: bool) -> Result<String> {
        let request = self.prefill(request)?;
        let method = self.resolve_method(&request);

        // Ask for a share password when one is needed and we have none stored.
        let mut request = request;
        if request.needs_password() && request.password.is_empty() {
            if let Some(pw) = self.ask_share_password(&request) {
                request.password = pw;
            }
        }

        self.push(
            LogLevel::Info,
            format!("Mounting {} ({})", request.display_name(), method.short()),
            Some(request.display_source()),
        );

        let result = match method {
            MountMethod::Gvfs => self.mount_gvfs(&request),
            MountMethod::FuseUser => self.mount_fuse(&request),
            MountMethod::System | MountMethod::Auto => self.mount_system(&request),
        };

        match &result {
            Ok(msg) => {
                self.push(LogLevel::Success, msg.clone(), None);
                if save {
                    self.save_share(&request).ok();
                }
            }
            Err(e) => self.push(
                LogLevel::Error,
                format!("Failed to mount {}", request.display_name()),
                Some(e.to_string()),
            ),
        }
        result
    }

    /// Mount a block device with sensible defaults.
    pub fn mount_device(&self, device: &BlockDevice, target: Option<String>) -> Result<String> {
        let caller = &self.caller;
        let slug = platform::slug(&device.title());
        let target = target.unwrap_or_else(|| {
            platform::default_mount_root()
                .join(slug)
                .to_string_lossy()
                .to_string()
        });
        let request = ShareRequest {
            name: device.title(),
            protocol: Protocol::Block,
            local_source: device.path.clone(),
            mount_point: target,
            custom_fstype: device.fstype.clone(),
            persist: false,
            bookmark: true,
            ..Default::default()
        }
        .normalize(caller)?;
        self.mount(request, true)
    }

    /// `gio mount <uri>` — no root needed, shows up in Files immediately.
    fn mount_gvfs(&self, request: &ShareRequest) -> Result<String> {
        if !exec::exists("gio") {
            return Err(Error::new("GVFS (`gio`) is not available")
                .hint("sudo apt install gvfs-backends gio, or choose the system mount method"));
        }
        let uri = request.gvfs_uri(true).ok_or_else(|| {
            Error::new(format!(
                "{} cannot be mounted through GVFS",
                request.protocol.short()
            ))
        })?;
        let public_uri = request.gvfs_uri(false).unwrap_or_else(|| uri.clone());

        let out = exec::run(
            &Cmd::new("gio")
                .arg("mount")
                .arg(&uri)
                .timeout(Duration::from_secs(60)),
        )?;
        if !out.ok() {
            let detail = out.combined();
            let message =
                if detail.contains("Permission denied") || detail.contains("Access denied") {
                    "The server rejected these credentials".to_string()
                } else if detail.contains("Timed out") || detail.contains("timeout") {
                    format!("`{}` did not answer in time", request.host)
                } else {
                    format!("GVFS could not mount {public_uri}")
                };
            return Err(Error::with_detail(message, detail).hint(
                "Tip: the system mount method (admin rights) often works where GVFS gives up.",
            ));
        }

        let mounted = self.mount_point_after_gvfs(request);
        if request.persist {
            self.push(
                LogLevel::Warning,
                "GVFS mounts cannot be added to /etc/fstab".to_string(),
                Some("Use the system mount method if you want this share to come back after a reboot.".to_string()),
            );
        }
        // GVFS chooses the mount point itself (/run/user/<uid>/gvfs/...).
        let path = mounted.unwrap_or_else(|| request.mount_point.clone());
        if request.bookmark {
            self.add_user_bookmark(&format!("file://{path}"), &request.display_name());
        }
        Ok(format!("Mounted {public_uri} at {path}"))
    }

    /// GVFS decides where the share appears; find it.
    fn mount_point_after_gvfs(&self, request: &ShareRequest) -> Option<String> {
        let live = mounts::read_mounts().unwrap_or_default();
        let uris = mounts::gvfs_uris();
        let wanted = request.gvfs_uri(false).unwrap_or_default();
        uris.iter()
            .find(|(_, uri)| uri == &wanted)
            .map(|(path, _)| path.clone())
            .or_else(|| {
                live.iter()
                    .find(|m| m.kind == MountKind::Gvfs)
                    .map(|m| m.target.clone())
            })
    }

    /// `sshfs` / `curlftpfs` as the user — no root needed.
    fn mount_fuse(&self, request: &ShareRequest) -> Result<String> {
        let argv = request.fuse_argv(&self.caller).ok_or_else(|| {
            Error::new(format!(
                "{} has no user FUSE mount",
                request.protocol.short()
            ))
        })?;
        let program = &argv[0];
        if !exec::exists(program) {
            let pkgs = request.protocol.packages().join(" ");
            return Err(Error::new(format!("`{program}` is not installed"))
                .hint(format!("sudo apt install {pkgs}")));
        }
        self.ensure_user_dir(&request.mount_point)?;
        let out = exec::run(
            &Cmd::new(program)
                .args(&argv[1..])
                .timeout(Duration::from_secs(90)),
        )?;
        if !out.ok() {
            let detail = out.combined();
            let message = if detail.contains("Permission denied (publickey")
                || detail.contains("Host key verification failed")
            {
                "SSH authentication failed".to_string()
            } else if detail.contains("Connection refused") {
                format!("`{}` refused the connection", request.host)
            } else {
                format!("`{program}` could not mount {}", request.display_name())
            };
            return Err(Error::with_detail(message, detail));
        }
        // FUSE mounts are not visible to mount(8) bookkeeping, but they are in
        // mountinfo; verify to give a trustworthy answer.
        if !mounts::is_mounted(&request.mount_point) {
            return Err(Error::new(format!(
                "`{program}` exited successfully but {} is not mounted",
                request.mount_point
            )));
        }
        if request.bookmark {
            self.add_user_bookmark(
                &format!("file://{}", request.mount_point),
                &request.display_name(),
            );
        }
        Ok(format!(
            "Mounted {} on {}",
            request.display_source(),
            request.mount_point
        ))
    }

    /// `mount(8)` through the privileged helper.
    fn mount_system(&self, request: &ShareRequest) -> Result<String> {
        if self.runner.method() == Method::None {
            return Err(Error::new(format!(
                "{} needs a system mount, but administrator rights are unavailable",
                request.protocol.short()
            ))
            .hint("Enable sudo/polkit in Preferences, or choose a user-session mount method."));
        }
        if let Some(pkg) = discover::missing_tool_for(request.protocol).first() {
            return Err(Error::new(format!(
                "The `{pkg}` package is required for {} mounts",
                request.protocol.short()
            ))
            .hint(format!("sudo apt install {pkg}")));
        }

        let target = platform::normalize_absolute(&request.mount_point)?
            .to_string_lossy()
            .to_string();
        let id = request.id();
        let persist = request.persist;

        // Credentials file (cifs / davfs) — never on the command line.
        let creds = self.cred_spec(request, &id, persist);
        let persist_spec = if persist {
            let cred_path = creds.as_ref().and_then(|c| c.keep.then(|| c.path.clone()));
            Some(PersistSpec {
                lines: request.fstab_lines(&self.caller, cred_path.as_deref()),
                target: target.clone(),
            })
        } else {
            None
        };
        let bookmark = request.bookmark.then(|| BookmarkSpec {
            uri: format!("file://{target}"),
            title: request.display_name(),
            home: self.caller.home.to_string_lossy().to_string(),
            uid: self.caller.uid,
            gid: self.caller.gid,
        });

        let op = Op::Mount(Box::new(MountOp {
            target: target.clone(),
            create_target: true,
            attempts: request.attempts(&self.caller),
            post: request.post_step(&self.caller),
            creds: creds.clone(),
            persist: persist_spec,
            bookmark,
            chown_target: request.take_ownership.then(|| OwnerSpec {
                uid: self.caller.uid,
                gid: self.caller.gid,
            }),
            verify: true,
            caller: self.caller_spec(),
        }));

        let result = self.run_op(&op)?;
        if !result.ok {
            return Err(Error {
                message: result.message,
                detail: result.detail.or_else(|| Some(result.log.join("\n"))),
            });
        }
        if request.remember_password && !request.password.is_empty() {
            let _ = self.save_share(request);
        }
        Ok(result.message)
    }

    /// Build the credential file spec for protocols that support one.
    fn cred_spec(&self, request: &ShareRequest, id: &str, persist: bool) -> Option<CredSpec> {
        let dir = if persist {
            format!("{STATE_DIR}/credentials")
        } else {
            "/run/mount-manager/credentials".to_string()
        };
        match request.protocol {
            Protocol::Cifs if !request.username.is_empty() || !request.password.is_empty() => {
                let path = format!("{dir}/{id}.cred");
                let mut contents = format!("username={}\n", request.username);
                contents.push_str(&format!(
                    "password={}\n",
                    request.password.replace('\n', "")
                ));
                if !request.domain.trim().is_empty() {
                    contents.push_str(&format!("domain={}\n", request.domain.trim()));
                }
                Some(CredSpec {
                    kind: CredKind::Cifs,
                    path: path.clone(),
                    contents,
                    merge_url: None,
                    keep: persist,
                    extra_options: vec![format!("credentials={path}")],
                })
            }
            Protocol::WebDav if !request.username.is_empty() => {
                let url = request.source();
                Some(CredSpec {
                    kind: CredKind::Davfs,
                    // davfs2 only reads credentials from this shared file, so it
                    // is merged, never deleted (`keep` stays true).
                    path: "/etc/davfs2/secrets".to_string(),
                    contents: format!("{url} {} {}", request.username, request.password),
                    merge_url: Some(url),
                    keep: true,
                    extra_options: Vec::new(),
                })
            }
            _ => None,
        }
    }

    // ── unmounting ──────────────────────────────────────────────────────────

    /// Unmount a live entry, choosing the cheapest working mechanism.
    pub fn unmount(&self, entry: &MountEntry, lazy: bool) -> Result<String> {
        self.push(
            LogLevel::Info,
            format!("Unmounting {}", entry.target),
            Some(entry.source.clone()),
        );
        let result = if entry.unmount_via_gvfs() {
            self.unmount_gvfs(entry)
        } else if entry.unmount_via_fuse() && self.try_fusermount(entry, lazy).is_ok() {
            Ok(format!("Unmounted {}", entry.target))
        } else {
            self.unmount_system(entry, lazy)
        };
        match &result {
            Ok(msg) => self.push(LogLevel::Success, msg.clone(), None),
            Err(e) => self.push(
                LogLevel::Error,
                format!("Failed to unmount {}", entry.target),
                Some(e.to_string()),
            ),
        }
        result
    }

    fn unmount_gvfs(&self, entry: &MountEntry) -> Result<String> {
        if !exec::exists("gio") {
            return Err(Error::new(
                "`gio` is not available to unmount this GVFS share",
            ));
        }
        let uris: Vec<String> = entry
            .gvfs_uri
            .clone()
            .map(|u| vec![u])
            .unwrap_or_else(|| vec![mounts::resolve_gvfs_uri(&entry.target, &[])]);
        let mut last = Error::new("GVFS could not unmount this share");
        for uri in &uris {
            let out = exec::run(
                &Cmd::new("gio")
                    .args(["mount", "-u", uri])
                    .timeout(Duration::from_secs(30)),
            )?;
            if out.ok() {
                if let Some(bm) = self.bookmark_for_target(&entry.target) {
                    self.remove_user_bookmark(&bm);
                }
                return Ok(format!("Unmounted {uri}"));
            }
            last = Error::with_detail(format!("`gio mount -u {uri}` failed"), out.combined());
        }
        // Fall back to the mount point path itself.
        let out = exec::run(
            &Cmd::new("gio")
                .args(["mount", "-u", &entry.target])
                .timeout(Duration::from_secs(30)),
        )?;
        if out.ok() {
            Ok(format!("Unmounted {}", entry.target))
        } else {
            Err(last.hint(&out.combined()))
        }
    }

    /// Try a user level `fusermount -u` first: it needs no password.
    fn try_fusermount(&self, entry: &MountEntry, lazy: bool) -> Result<()> {
        for program in ["fusermount3", "fusermount"] {
            if !exec::exists(program) {
                continue;
            }
            let mut cmd = Cmd::new(program).arg("-u");
            if lazy {
                cmd = cmd.arg("-z");
            }
            cmd = cmd.arg(&entry.target).timeout(Duration::from_secs(30));
            if let Ok(out) = exec::run(&cmd) {
                if out.ok() && !mounts::is_mounted(&entry.target) {
                    return Ok(());
                }
            }
        }
        Err(Error::new("fusermount could not detach this mount"))
    }

    fn unmount_system(&self, entry: &MountEntry, lazy: bool) -> Result<String> {
        if self.runner.method() == Method::None {
            return Err(
                Error::new("Administrator rights are required to unmount this filesystem")
                    .hint("Enable sudo/polkit in Preferences."),
            );
        }
        let cleanup = self.cleanup_paths_for(entry);
        let op = Op::Umount(UmountOp {
            target: entry.target.clone(),
            lazy,
            force: entry.protocol == Protocol::Nfs,
            remove_fstab: entry.managed,
            remove_bookmark: self.bookmark_for_target(&entry.target),
            remove_dir_if_empty: !lazy && entry.kind != MountKind::Fixed,
            expect_source: Some(entry.source.clone()),
            cleanup,
            caller: self.caller_spec(),
        });
        let result = self.run_op(&op)?;
        if result.ok {
            Ok(result.message)
        } else {
            Err(Error {
                message: result.message,
                detail: result.detail.or_else(|| Some(result.log.join("\n"))),
            })
        }
    }

    /// Credential files this app created for `entry`.
    fn cleanup_paths_for(&self, entry: &MountEntry) -> Vec<String> {
        let mut paths = Vec::new();
        for opt in entry.super_options.iter().chain(entry.options.iter()) {
            if let Some(p) = opt.strip_prefix("credentials=") {
                if p.starts_with("/run/mount-manager/") {
                    paths.push(p.to_string());
                }
            }
        }
        paths
    }

    fn bookmark_for_target(&self, target: &str) -> Option<String> {
        let uri = format!("file://{target}");
        let path = self.caller.home.join(".gtk-bookmarks");
        let text = std::fs::read_to_string(path).ok()?;
        text.lines()
            .find(|l| l.split_whitespace().next() == Some(uri.as_str()))
            .map(|_| uri)
    }

    // ── saved shares ────────────────────────────────────────────────────────

    /// Save (or update) a share definition.
    pub fn save_share(&self, request: &ShareRequest) -> Result<String> {
        let request = self.prefill(request.clone())?;
        let id = request.id();
        let saved = self.config().make_saved(&request);
        self.update_config(|cfg| cfg.upsert(saved))?;
        self.push(
            LogLevel::Success,
            format!("Saved share {}", request.display_name()),
            None,
        );
        Ok(id)
    }

    pub fn delete_share(&self, id: &str) -> Result<String> {
        let name = self
            .config()
            .shares
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.request.display_name())
            .unwrap_or_else(|| id.to_string());
        let mut removed_fstab = false;
        if let Some(share) = self.config().shares.iter().find(|s| s.id == id) {
            if share.request.persist {
                // Drop the fstab entry too (needs admin rights).
                let op = Op::FstabRemove(FstabOp {
                    lines: Vec::new(),
                    target: share.request.mount_point.clone(),
                    backup: true,
                    caller: self.caller_spec(),
                });
                if let Ok(res) = self.run_op(&op) {
                    removed_fstab = res.ok;
                }
            }
        }
        self.update_config(|cfg| {
            cfg.remove(id);
        })?;
        self.push(
            LogLevel::Info,
            format!(
                "Deleted share {name}{}",
                if removed_fstab {
                    " (and its /etc/fstab entry)"
                } else {
                    ""
                }
            ),
            None,
        );
        Ok(name)
    }

    /// Password stored for a saved share (used when mounting from the list).
    pub fn password_for(&self, share: &SavedShare) -> Option<String> {
        self.config().password_for(share)
    }

    /// Mount a saved share.
    pub fn mount_saved(&self, id: &str) -> Result<String> {
        let share = self
            .config()
            .shares
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::new(format!("No saved share with id `{id}`")))?;
        let mut request = share.request.clone();
        if request.password.is_empty() {
            if let Some(pw) = self.password_for(&share) {
                request.password = pw;
            }
        }
        let msg = self.mount(request, true)?;
        self.update_config(|cfg| {
            if let Some(s) = cfg.find_mut(id) {
                s.last_used = Some(now_stamp());
            }
        })?;
        Ok(msg)
    }

    /// Mount everything flagged "mount at login" — used by `mount-manager auto-mount`.
    pub fn auto_mount(&self) -> Vec<(String, Result<String>)> {
        let shares: Vec<SavedShare> = self
            .config()
            .shares
            .into_iter()
            .filter(|s| s.auto_mount || s.request.persist)
            .collect();
        let mut out = Vec::new();
        for share in shares {
            let name = share.request.display_name();
            let id = share.id.clone();
            match self.mount_saved(&id) {
                Ok(msg) => out.push((name, Ok(msg))),
                Err(e) => out.push((name, Err(e))),
            }
        }
        out
    }

    // ── directories, bookmarks, opening ─────────────────────────────────────

    /// Create a mount point as the user when possible, else as root.
    pub fn ensure_mount_point(&self, path: &str) -> Result<()> {
        self.ensure_user_dir(path)
    }

    fn ensure_user_dir(&self, path: &str) -> Result<()> {
        let p = platform::normalize_absolute(path)?;
        if p.exists() {
            if p.is_dir() {
                return Ok(());
            }
            return Err(Error::new(format!(
                "`{}` exists but is not a directory",
                path
            )));
        }
        match std::fs::create_dir_all(&p) {
            Ok(()) => Ok(()),
            Err(_) => {
                // Not writable for the user: ask the helper.
                let op = Op::MakeDir(MakeDirOp {
                    paths: vec![DirSpec {
                        path: p.to_string_lossy().to_string(),
                        uid: self.caller.uid,
                        gid: self.caller.gid,
                        mode: 0o755,
                    }],
                    caller: self.caller_spec(),
                });
                let res = self.run_op(&op)?;
                if res.ok {
                    Ok(())
                } else {
                    Err(Error::with_detail(
                        res.message,
                        res.detail.unwrap_or_default(),
                    ))
                }
            }
        }
    }

    fn add_user_bookmark(&self, uri: &str, title: &str) {
        let path = self.caller.home.join(".gtk-bookmarks");
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        if text
            .lines()
            .any(|l| l.split_whitespace().next() == Some(uri))
        {
            return;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("{uri} {}\n", title.trim()));
        let _ = std::fs::write(&path, text);
    }

    fn remove_user_bookmark(&self, uri: &str) {
        let path = self.caller.home.join(".gtk-bookmarks");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let kept: Vec<&str> = text
            .lines()
            .filter(|l| l.split_whitespace().next() != Some(uri))
            .collect();
        let _ = std::fs::write(&path, kept.join("\n") + "\n");
    }

    /// Open a path in the file manager.
    pub fn open(&self, path: &str) -> Result<()> {
        for program in ["gio", "xdg-open"] {
            if !exec::exists(program) {
                continue;
            }
            let args: Vec<String> = if program == "gio" {
                vec!["open".to_string(), path.to_string()]
            } else {
                vec![path.to_string()]
            };
            let out = exec::run(
                &Cmd::new(program)
                    .args(args)
                    .timeout(Duration::from_secs(10)),
            )?;
            if out.ok() {
                return Ok(());
            }
        }
        Err(Error::new(format!(
            "Could not open `{path}` in the file manager"
        )))
    }

    // ── fstab helpers ───────────────────────────────────────────────────────

    /// Add or remove the `/etc/fstab` entry ("mount at login") for a share.
    pub fn set_persistent(&self, request: &ShareRequest, persist: bool) -> Result<String> {
        let request = self.prefill(request.clone())?;
        if persist {
            let id = request.id();
            // Credential files referenced from fstab must exist before boot.
            if let Some(creds) = self.cred_spec(&request, &id, true) {
                let op = Op::WriteSecret(WriteSecretOp {
                    creds: creds.clone(),
                    caller: self.caller_spec(),
                });
                let res = self.run_op(&op)?;
                if !res.ok {
                    return Err(Error::with_detail(
                        res.message,
                        res.detail.unwrap_or_default(),
                    ));
                }
                let op = Op::FstabAdd(FstabOp {
                    lines: request.fstab_lines(&self.caller, Some(&creds.path)),
                    target: request.mount_point.clone(),
                    backup: true,
                    caller: self.caller_spec(),
                });
                return self.finish_op(op);
            }
            let op = Op::FstabAdd(FstabOp {
                lines: request.fstab_lines(&self.caller, None),
                target: request.mount_point.clone(),
                backup: true,
                caller: self.caller_spec(),
            });
            self.finish_op(op)
        } else {
            let op = Op::FstabRemove(FstabOp {
                lines: Vec::new(),
                target: request.mount_point.clone(),
                backup: true,
                caller: self.caller_spec(),
            });
            self.finish_op(op)
        }
    }

    fn finish_op(&self, op: Op) -> Result<String> {
        let res = self.run_op(&op)?;
        if res.ok {
            self.push(LogLevel::Success, res.message.clone(), None);
            Ok(res.message)
        } else {
            self.push(LogLevel::Error, res.message.clone(), res.detail.clone());
            Err(Error {
                message: res.message,
                detail: res.detail,
            })
        }
    }

    /// Restore `/etc/fstab` from the backup Mount Manager made.
    pub fn restore_fstab_backup(&self) -> Result<String> {
        let path = PathBuf::from(fstab::BACKUP_PATH);
        if !path.exists() {
            return Err(Error::new(format!("No backup found at {}", path.display())));
        }
        let op = Op::FstabRestore(FstabOp {
            lines: Vec::new(),
            target: "/etc/fstab".to_string(),
            backup: false,
            caller: self.caller_spec(),
        });
        self.finish_op(op)
    }

    // ── escalation plumbing ─────────────────────────────────────────────────

    fn caller_spec(&self) -> CallerSpec {
        CallerSpec {
            uid: self.caller.uid,
            gid: self.caller.gid,
            name: self.caller.name.clone(),
            home: self.caller.home.to_string_lossy().to_string(),
        }
    }

    /// Run an op with root rights and translate the answer into `Result`.
    pub fn run_op(&self, op: &Op) -> Result<OpResult> {
        self.runner.run(op, self.prompter.as_ref())
    }

    fn ask_share_password(&self, request: &ShareRequest) -> Option<String> {
        let ctx = PromptContext {
            reason: format!("Password for `{}`", request.username),
            detail: format!(
                "{} — {}",
                request.protocol.short(),
                request.display_source()
            ),
            username: request.username.clone(),
            attempt: 1,
            previous_error: None,
        };
        self.prompter.password(&ctx).map(|creds| creds.password)
    }

    // ── activity log ────────────────────────────────────────────────────────

    pub fn push(&self, level: LogLevel, message: String, detail: Option<String>) {
        if let Ok(mut log) = self.log.lock() {
            log.push(LogEntry {
                time: now_stamp(),
                level,
                message,
                detail,
            });
            // Keep the log bounded.
            if log.len() > 500 {
                let drop = log.len() - 500;
                log.drain(0..drop);
            }
        }
    }

    pub fn logs(&self) -> Vec<LogEntry> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoPrompt;
    impl Prompter for NoPrompt {
        fn password(&self, _ctx: &PromptContext) -> Option<crate::privilege::Credentials> {
            None
        }
    }

    fn engine() -> Arc<Engine> {
        Engine::new(Arc::new(NoPrompt))
    }

    #[test]
    fn resolves_methods_from_available_tools() {
        let e = engine();
        let mut req = ShareRequest {
            protocol: Protocol::Cifs,
            host: "nas".into(),
            remote_path: "data".into(),
            mount_point: "/mnt/nas".into(),
            ..Default::default()
        };
        // System by default (or GVFS when we have no way to escalate at all).
        let m = e.resolve_method(&req);
        assert!(
            matches!(m, MountMethod::System | MountMethod::Gvfs),
            "{m:?}"
        );

        req.protocol = Protocol::Sshfs;
        req.username = "jo".into();
        let m = e.resolve_method(&req);
        assert!(matches!(
            m,
            MountMethod::FuseUser | MountMethod::Gvfs | MountMethod::System
        ));

        req.method = MountMethod::Gvfs;
        assert_eq!(e.resolve_method(&req), MountMethod::Gvfs);
    }

    #[test]
    fn log_is_bounded_and_readable() {
        let e = engine();
        for i in 0..600 {
            e.push(LogLevel::Info, format!("line {i}"), None);
        }
        let logs = e.logs();
        assert_eq!(logs.len(), 500);
        assert!(logs.last().unwrap().message.contains("599"));
        assert!(!logs.first().unwrap().message.contains("line 0"));
    }

    #[test]
    fn credential_files_live_in_private_directories() {
        let e = engine();
        let req = ShareRequest {
            protocol: Protocol::Cifs,
            host: "nas".into(),
            remote_path: "data".into(),
            mount_point: "/mnt/nas".into(),
            username: "jo".into(),
            password: "hunter2".into(),
            ..Default::default()
        };
        let creds = e
            .cred_spec(&req, "id1", false)
            .expect("cifs needs a credentials file");
        assert!(creds.path.starts_with("/run/mount-manager/credentials/"));
        assert!(!creds.keep);
        assert_eq!(
            creds.extra_options,
            vec![format!("credentials={}", creds.path)]
        );
        assert!(creds.contents.contains("username=jo"));
        assert!(creds.contents.contains("password=hunter2"));

        let persistent = e.cred_spec(&req, "id1", true).unwrap();
        assert!(persistent
            .path
            .starts_with("/etc/mount-manager/credentials/"));
        assert!(persistent.keep);
    }

    #[test]
    fn fstab_never_contains_the_password() {
        let e = engine();
        let req = ShareRequest {
            protocol: Protocol::Cifs,
            host: "nas".into(),
            remote_path: "data".into(),
            mount_point: "/mnt/nas".into(),
            username: "jo".into(),
            password: "hunter2".into(),
            persist: true,
            ..Default::default()
        };
        let creds = e.cred_spec(&req, "id1", true).unwrap();
        let lines = req.fstab_lines(e.caller(), Some(&creds.path));
        assert!(!lines.join("\n").contains("hunter2"));
        assert!(lines[1].contains("credentials=/etc/mount-manager/credentials/id1.cred"));
    }

    #[test]
    fn mount_rejects_dangerous_targets_before_escalating() {
        let e = engine();
        let req = ShareRequest {
            protocol: Protocol::Cifs,
            host: "nas".into(),
            remote_path: "data".into(),
            mount_point: "/usr".into(),
            ..Default::default()
        };
        let err = e.mount(req, false).unwrap_err();
        assert!(err.message.contains("protected"), "{err}");
    }
}

//! Wire format between the unprivileged UI/CLI and the privileged helper.
//!
//! A single [`Op`] value is serialised to JSON and written to the helper's
//! **stdin** (never argv, so passwords cannot be read from `ps`). The helper
//! answers with one JSON [`OpResult`] on stdout.
//!
//! Keeping every privileged action inside this enum means there is exactly one
//! code path ([`crate::executor`]) that can call `mount(8)`, `umount(8)` or
//! rewrite `/etc/fstab` — it is validated once and shared by both the
//! "already root" case and the "escalated through pkexec/sudo" case.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "payload", rename_all = "kebab-case")]
pub enum Op {
    /// Cheap check that escalation works; returns the effective caller.
    Probe(ProbeOp),
    /// Mount something (with an ordered list of attempts).
    Mount(Box<MountOp>),
    /// Unmount a mount point.
    Umount(UmountOp),
    /// Create (and chown) directories, e.g. mount points.
    MakeDir(MakeDirOp),
    /// Append lines to `/etc/fstab`.
    FstabAdd(FstabOp),
    /// Remove the entry for a mount point from `/etc/fstab`.
    FstabRemove(FstabOp),
    /// Delete root owned secrets created by a previous mount.
    RemoveSecrets(RemoveSecretsOp),
    /// Write a root owned credential file without mounting (fstab entries).
    WriteSecret(WriteSecretOp),
    /// Restore `/etc/fstab` from the backup made before the last change.
    FstabRestore(FstabOp),
}

impl Op {
    /// Human readable summary for the activity log.
    pub fn summary(&self) -> String {
        match self {
            Op::Probe(_) => "probe administrator access".to_string(),
            Op::Mount(m) => format!(
                "mount {} on {}",
                m.attempts.first().map(|a| a.source.as_str()).unwrap_or("?"),
                m.target
            ),
            Op::Umount(u) => format!("unmount {}", u.target),
            Op::MakeDir(d) => format!(
                "create {}",
                d.paths
                    .iter()
                    .map(|p| p.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Op::FstabAdd(f) => format!("add /etc/fstab entry for {}", f.target),
            Op::FstabRemove(f) => format!("remove /etc/fstab entry for {}", f.target),
            Op::RemoveSecrets(_) => "remove stored credentials".to_string(),
            Op::WriteSecret(_) => "store credentials for /etc/fstab".to_string(),
            Op::FstabRestore(_) => "restore /etc/fstab from backup".to_string(),
        }
    }

    /// Ops that only read state and therefore never need a password prompt.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Op::Probe(_))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeOp {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerSpec {
    pub uid: u32,
    pub gid: u32,
    pub name: String,
    pub home: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountAttempt {
    /// `-t <fstype>`; `None` lets `mount(8)` guess (block devices).
    pub fstype: Option<String>,
    /// Source spec: `//host/share`, `host:/export`, `user@host:/path`, `/dev/sdb1`, ...
    pub source: String,
    /// `-o` options (already validated, no spaces).
    pub options: Vec<String>,
    /// Extra positional flags such as `--bind`.
    #[serde(default)]
    pub flags: Vec<String>,
    /// Label shown in the log when this attempt is used.
    #[serde(default)]
    pub label: Option<String>,
}

impl MountAttempt {
    pub fn new<S: Into<String>>(source: S) -> Self {
        Self {
            fstype: None,
            source: source.into(),
            options: Vec::new(),
            flags: Vec::new(),
            label: None,
        }
    }

    pub fn fstype<S: Into<String>>(mut self, t: S) -> Self {
        self.fstype = Some(t.into());
        self
    }

    pub fn options<I, S>(mut self, iter: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.options = iter.into_iter().map(Into::into).collect();
        self
    }

    pub fn flag<S: Into<String>>(mut self, f: S) -> Self {
        self.flags.push(f.into());
        self
    }

    pub fn label<S: Into<String>>(mut self, l: S) -> Self {
        self.label = Some(l.into());
        self
    }

    /// Full argv for `mount(8)`, excluding the target.
    pub fn argv(&self, target: &str) -> Vec<String> {
        let mut v = vec!["mount".to_string()];
        v.extend(self.flags.clone());
        if let Some(t) = &self.fstype {
            v.push("-t".to_string());
            v.push(t.clone());
        }
        if !self.options.is_empty() {
            v.push("-o".to_string());
            v.push(self.options.join(","));
        }
        v.push(self.source.clone());
        v.push(target.to_string());
        v
    }

    /// `mount -t cifs -o ... src target` with credentials masked.
    pub fn render(&self, target: &str) -> String {
        let masked: Vec<String> = self
            .options
            .iter()
            .map(|o| {
                if let Some(k) = o.split('=').next() {
                    if matches!(k, "password" | "pass" | "secret" | "user") && o.contains('=') {
                        return format!("{}=***", k);
                    }
                }
                o.clone()
            })
            .collect();
        let mut attempt = self.clone();
        attempt.options = masked;
        attempt.argv(target).join(" ")
    }
}

/// Optional step after a successful mount.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PostStep {
    /// `mount -o remount,ro <target>` — needed for read-only bind mounts.
    RemountRo,
    /// `chown <uid>:<gid> <target>` for filesystems without uid/gid options.
    ChownTarget(OwnerSpec),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OwnerSpec {
    pub uid: u32,
    pub gid: u32,
}

/// Root owned secret file written before mounting (cifs credentials, davfs2
/// secrets, curl netrc). Never passed on the command line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CredSpec {
    pub kind: CredKind,
    /// Absolute path written by the helper (mode 0600, owner root).
    pub path: String,
    /// Full file contents.
    pub contents: String,
    /// When set, an existing file is merged instead of overwritten (davfs2).
    #[serde(default)]
    pub merge_url: Option<String>,
    /// Keep the file after mounting (needed for `/etc/fstab` entries).
    #[serde(default)]
    pub keep: bool,
    /// Extra options appended to the mount once the file exists.
    #[serde(default)]
    pub extra_options: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CredKind {
    /// `username=..`/`password=..`/`domain=..` file for `mount.cifs`.
    Cifs,
    /// `<url> <user> <password>` line for davfs2 `/etc/davfs2/secrets`.
    Davfs,
    /// `machine <host> login <user> password <pass>` for curl/curlftpfs.
    Netrc,
    /// Plain text file only the helper may read.
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistSpec {
    /// Raw `/etc/fstab` lines (including the `# mount-manager:` marker comment).
    pub lines: Vec<String>,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkSpec {
    /// `file:///mnt/nas` URI appended to `~/.gtk-bookmarks`.
    pub uri: String,
    pub title: String,
    pub home: String,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirSpec {
    pub path: String,
    pub uid: u32,
    pub gid: u32,
    /// Unix mode, e.g. `0o755`.
    pub mode: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountOp {
    pub target: String,
    #[serde(default = "default_true")]
    pub create_target: bool,
    pub attempts: Vec<MountAttempt>,
    #[serde(default)]
    pub post: Option<PostStep>,
    #[serde(default)]
    pub creds: Option<CredSpec>,
    #[serde(default)]
    pub persist: Option<PersistSpec>,
    #[serde(default)]
    pub bookmark: Option<BookmarkSpec>,
    #[serde(default)]
    pub chown_target: Option<OwnerSpec>,
    #[serde(default = "default_true")]
    pub verify: bool,
    pub caller: CallerSpec,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UmountOp {
    pub target: String,
    /// `umount -l` (detach) — only when the user explicitly accepted the warning.
    #[serde(default)]
    pub lazy: bool,
    /// `umount -f` for unreachable NFS servers.
    #[serde(default)]
    pub force: bool,
    /// Also drop the `/etc/fstab` entry for this target.
    #[serde(default)]
    pub remove_fstab: bool,
    /// Bookmark line to remove from `~/.gtk-bookmarks`.
    #[serde(default)]
    pub remove_bookmark: Option<String>,
    /// `rmdir` the mount point afterwards when it is empty and below /media or /mnt.
    #[serde(default)]
    pub remove_dir_if_empty: bool,
    /// Refuse to unmount when the live source differs (stale UI protection).
    #[serde(default)]
    pub expect_source: Option<String>,
    /// Root owned files to delete after unmounting.
    #[serde(default)]
    pub cleanup: Vec<String>,
    pub caller: CallerSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeDirOp {
    pub paths: Vec<DirSpec>,
    pub caller: CallerSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FstabOp {
    #[serde(default)]
    pub lines: Vec<String>,
    pub target: String,
    #[serde(default = "default_true")]
    pub backup: bool,
    pub caller: CallerSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveSecretsOp {
    pub paths: Vec<String>,
    pub caller: CallerSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteSecretOp {
    pub creds: CredSpec,
    pub caller: CallerSpec,
}

/// What the helper (or the in-process executor) returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpResult {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub detail: Option<String>,
    /// Step-by-step transcript shown in the Activity page.
    #[serde(default)]
    pub log: Vec<String>,
}

impl OpResult {
    pub fn ok<M: Into<String>>(message: M) -> Self {
        Self {
            ok: true,
            message: message.into(),
            detail: None,
            log: Vec::new(),
        }
    }

    pub fn fail<M: Into<String>>(message: M) -> Self {
        Self {
            ok: false,
            message: message.into(),
            detail: None,
            log: Vec::new(),
        }
    }

    pub fn with_log(mut self, log: Vec<String>) -> Self {
        self.log = log;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_argv_and_render() {
        let a = MountAttempt::new("//nas/data").fstype("cifs").options([
            "username=jo".to_string(),
            "password=secret".to_string(),
            "uid=1000".to_string(),
        ]);
        assert_eq!(
            a.argv("/mnt/data"),
            vec![
                "mount",
                "-t",
                "cifs",
                "-o",
                "username=jo,password=secret,uid=1000",
                "//nas/data",
                "/mnt/data"
            ]
        );
        let rendered = a.render("/mnt/data");
        assert!(
            rendered.contains("username=jo"),
            "user names are not secret: {rendered}"
        );
        assert!(rendered.contains("password=***"), "{rendered}");
        assert!(!rendered.contains("secret"), "{rendered}");
    }

    #[test]
    fn json_roundtrip() {
        let op = Op::Mount(Box::new(MountOp {
            target: "/mnt/x".into(),
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
        }));
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"op\":\"mount\""));
        let back: Op = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Op::Mount(_)));
        assert_eq!(back.summary(), "mount //h/s on /mnt/x");
    }

    #[test]
    fn bind_flags_come_first() {
        let a = MountAttempt::new("/src").flag("--bind");
        assert_eq!(a.argv("/dst"), vec!["mount", "--bind", "/src", "/dst"]);
    }
}

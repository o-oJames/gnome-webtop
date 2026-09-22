//! Domain model: protocols, share definitions, live mounts, block devices.

use crate::error::{Error, Result};
use crate::ops::MountAttempt;
use crate::platform;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// Protocol
// ─────────────────────────────────────────────────────────────────────────────

/// Filesystem / transport protocols understood by the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// Windows share / Samba (`mount.cifs`, `smb://`).
    Cifs,
    /// Network File System (`mount.nfs`, NFSv3/v4).
    Nfs,
    /// SSH file system (`sshfs`, `sftp://`).
    Sshfs,
    /// WebDAV over HTTP(S) (`davfs2`, `dav://`/`davs://`).
    WebDav,
    /// FTP/FTPS (`curlftpfs`, `ftp://`).
    Ftp,
    /// Local block device: USB stick, SD card, external HDD, disk image.
    Block,
    /// Bind mount of an existing local directory.
    Bind,
    /// Anything else — the user supplies fstype and source.
    Custom,
}

/// How a share is mounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MountMethod {
    /// Decide from the protocol and the tools that are installed.
    Auto,
    /// `mount(8)` through the privileged helper (needs admin rights).
    System,
    /// GVFS/`gio mount` inside the user session (no admin rights, shows up in
    /// Files under `/run/user/<uid>/gvfs`).
    Gvfs,
    /// FUSE binary run by the user (`sshfs`, `curlftpfs`) — no admin rights.
    FuseUser,
}

impl Default for MountMethod {
    fn default() -> Self {
        MountMethod::Auto
    }
}

impl MountMethod {
    pub const ALL: [MountMethod; 4] = [
        MountMethod::Auto,
        MountMethod::System,
        MountMethod::Gvfs,
        MountMethod::FuseUser,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MountMethod::Auto => "Automatic (recommended)",
            MountMethod::System => "System mount (admin rights)",
            MountMethod::Gvfs => "User session / GVFS (no admin rights)",
            MountMethod::FuseUser => "User FUSE mount (no admin rights)",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            MountMethod::Auto => "auto",
            MountMethod::System => "system",
            MountMethod::Gvfs => "gvfs",
            MountMethod::FuseUser => "fuse",
        }
    }

    /// `true` when this method needs root.
    pub fn needs_root(self) -> bool {
        matches!(self, MountMethod::System)
    }
}

impl Protocol {
    pub const ALL: [Protocol; 8] = [
        Protocol::Cifs,
        Protocol::Nfs,
        Protocol::Sshfs,
        Protocol::WebDav,
        Protocol::Ftp,
        Protocol::Block,
        Protocol::Bind,
        Protocol::Custom,
    ];

    /// Network protocols that can be browsed/discovered.
    pub const NETWORK: [Protocol; 5] = [
        Protocol::Cifs,
        Protocol::Nfs,
        Protocol::Sshfs,
        Protocol::WebDav,
        Protocol::Ftp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Cifs => "SMB / CIFS — Windows share, Samba, NAS",
            Protocol::Nfs => "NFS — Linux/Unix network filesystem",
            Protocol::Sshfs => "SSHFS / SFTP — folder over SSH",
            Protocol::WebDav => "WebDAV — HTTP(S) filesystem",
            Protocol::Ftp => "FTP / FTPS — file transfer protocol",
            Protocol::Block => "Local device — USB drive, SD card, disk image",
            Protocol::Bind => "Bind mount — reuse a local directory",
            Protocol::Custom => "Custom — any filesystem type",
        }
    }

    /// Short badge text.
    pub fn short(self) -> &'static str {
        match self {
            Protocol::Cifs => "SMB",
            Protocol::Nfs => "NFS",
            Protocol::Sshfs => "SSHFS",
            Protocol::WebDav => "WebDAV",
            Protocol::Ftp => "FTP",
            Protocol::Block => "Device",
            Protocol::Bind => "Bind",
            Protocol::Custom => "Custom",
        }
    }

    /// `-t` value for `mount(8)`.
    pub fn fstype(self) -> Option<&'static str> {
        match self {
            Protocol::Cifs => Some("cifs"),
            Protocol::Nfs => Some("nfs"),
            Protocol::Sshfs => Some("fuse.sshfs"),
            Protocol::WebDav => Some("davfs"),
            Protocol::Ftp => Some("fuse.curlftpfs"),
            Protocol::Block | Protocol::Bind => None,
            Protocol::Custom => None,
        }
    }

    pub fn default_port(self) -> Option<u16> {
        match self {
            Protocol::Cifs => Some(445),
            Protocol::Nfs => Some(2049),
            Protocol::Sshfs => Some(22),
            Protocol::WebDav => Some(443),
            Protocol::Ftp => Some(21),
            _ => None,
        }
    }

    /// Icon name from the Adwaita/Yaru icon theme.
    pub fn icon(self) -> &'static str {
        match self {
            Protocol::Cifs | Protocol::Nfs | Protocol::WebDav => "folder-remote-symbolic",
            Protocol::Sshfs => "dialog-password-symbolic",
            Protocol::Ftp => "folder-download-symbolic",
            Protocol::Block => "drive-removable-media-symbolic",
            Protocol::Bind => "emblem-symbolic-link-symbolic",
            Protocol::Custom => "drive-harddisk-symbolic",
        }
    }

    /// GVFS URI scheme, when the protocol is supported by GVFS.
    pub fn gvfs_scheme(self) -> Option<&'static str> {
        match self {
            Protocol::Cifs => Some("smb"),
            Protocol::Sshfs => Some("sftp"),
            Protocol::WebDav => Some("davs"),
            Protocol::Ftp => Some("ftp"),
            Protocol::Nfs | Protocol::Block | Protocol::Bind | Protocol::Custom => None,
        }
    }

    /// mDNS service type used for network discovery.
    pub fn mdns_service(self) -> Option<&'static str> {
        match self {
            Protocol::Cifs => Some("_smb._tcp"),
            Protocol::Sshfs => Some("_sftp-ssh._tcp"),
            Protocol::WebDav => Some("_webdavs._tcp"),
            Protocol::Ftp => Some("_ftp._tcp"),
            Protocol::Nfs => Some("_nfs._tcp"),
            _ => None,
        }
    }

    /// FUSE binary used by [`MountMethod::FuseUser`].
    pub fn fuse_program(self) -> Option<&'static str> {
        match self {
            Protocol::Sshfs => Some("sshfs"),
            Protocol::Ftp => Some("curlftpfs"),
            _ => None,
        }
    }

    /// Whether a plain user session can mount this at all without root.
    pub fn user_mountable(self) -> bool {
        matches!(
            self,
            Protocol::Sshfs | Protocol::WebDav | Protocol::Ftp | Protocol::Cifs
        )
    }

    /// `true` when the source is a remote server rather than a local path.
    pub fn is_network(self) -> bool {
        matches!(
            self,
            Protocol::Cifs | Protocol::Nfs | Protocol::Sshfs | Protocol::WebDav | Protocol::Ftp
        )
    }

    /// Package names that provide the client tools for this protocol.
    pub fn packages(self) -> &'static [&'static str] {
        match self {
            Protocol::Cifs => &["cifs-utils", "smbclient"],
            Protocol::Nfs => &["nfs-common"],
            Protocol::Sshfs => &["sshfs", "openssh-client"],
            Protocol::WebDav => &["davfs2", "gvfs-backends"],
            Protocol::Ftp => &["curlftpfs", "gvfs-backends"],
            Protocol::Block => &["util-linux"],
            Protocol::Bind => &["util-linux"],
            Protocol::Custom => &[],
        }
    }

    /// Recognise the fstype reported by the kernel.
    pub fn from_fstype(fstype: &str) -> Protocol {
        let f = fstype.to_ascii_lowercase();
        match f.as_str() {
            "cifs" | "smb3" | "smb2" => Protocol::Cifs,
            "nfs" | "nfs4" | "nfsd" => Protocol::Nfs,
            "fuse.sshfs" | "sshfs" => Protocol::Sshfs,
            "davfs" | "davfs2" | "fuse.davfs2" => Protocol::WebDav,
            "fuse.curlftpfs" | "curlftpfs" => Protocol::Ftp,
            "fuse.gvfsd-fuse" | "gvfsd-fuse" => Protocol::Custom,
            _ => Protocol::Custom,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ShareRequest — what the user wants to mount
// ─────────────────────────────────────────────────────────────────────────────

/// Everything needed to mount one share.
///
/// `password` is `#[serde(skip)]`: it is never written to disk by the config
/// layer (see [`crate::secret`] for where remembered passwords go).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShareRequest {
    /// Friendly name, also used for the mount point slug and fstab marker.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub protocol: Protocol,
    /// Server name/IP (or URL for WebDAV, device node for Block).
    #[serde(default)]
    pub host: String,
    /// Share name / export path / remote directory.
    #[serde(default)]
    pub remote_path: String,
    /// Absolute local mount point.
    #[serde(default)]
    pub mount_point: String,
    #[serde(default)]
    pub username: String,
    #[serde(skip)]
    #[serde(default)]
    pub password: String,
    /// SMB workgroup/domain.
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub port: Option<u16>,
    /// Extra `-o` options supplied by the user.
    #[serde(default)]
    pub extra_options: Vec<String>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub method: MountMethod,
    /// Add an `/etc/fstab` entry so it comes back after a reboot.
    #[serde(default)]
    pub persist: bool,
    /// Add a Files (Nautilus) sidebar bookmark.
    #[serde(default)]
    pub bookmark: bool,
    /// SSH private key for SSHFS.
    #[serde(default)]
    pub identity_file: String,
    /// Remember the password in the keyring/config after a successful mount.
    #[serde(default)]
    pub remember_password: bool,
    /// `chown` the mount point to the current user after mounting (useful for
    /// filesystems that ignore uid/gid options).
    #[serde(default)]
    pub take_ownership: bool,
    /// Source string for [`Protocol::Block`] (`/dev/sdb1`) and [`Protocol::Bind`].
    #[serde(default)]
    pub local_source: String,
    /// Free-form source and fstype for [`Protocol::Custom`].
    #[serde(default)]
    pub custom_source: String,
    #[serde(default)]
    pub custom_fstype: String,
}

impl Default for Protocol {
    fn default() -> Self {
        Protocol::Cifs
    }
}

impl ShareRequest {
    /// Stable id used for credential files and fstab markers.
    pub fn id(&self) -> String {
        let base = if self.name.trim().is_empty() {
            platform::slug(&format!(
                "{}-{}",
                self.host.trim(),
                self.remote_path.trim().trim_end_matches('/')
            ))
        } else {
            platform::slug(self.name.trim())
        };
        // Add a short hash so two different shares never share a credential file.
        let hash = fnv1a(&format!(
            "{}|{}|{}|{}",
            self.protocol.short(),
            self.host,
            self.remote_path,
            self.mount_point
        ));
        format!("{}-{:08x}", base, hash)
    }

    /// Display name used in lists.
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else {
            match self.protocol {
                Protocol::Block => self
                    .local_source
                    .rsplit('/')
                    .next()
                    .unwrap_or("device")
                    .to_string(),
                Protocol::Bind => Path::new(&self.local_source)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "bind".to_string()),
                _ => format!("{}:{}", self.host.trim(), self.remote_path.trim()),
            }
        }
    }

    /// The `mount(8)` source for this request (credentials are never included).
    pub fn source(&self) -> String {
        let host = self.host.trim();
        let path = normalize_remote_path(&self.remote_path);
        match self.protocol {
            Protocol::Cifs => {
                let share = path.trim_start_matches('/');
                let share = if share.is_empty() { "IPC$" } else { share };
                format!("//{}/{}", host.trim_start_matches('/'), share)
            }
            Protocol::Nfs => format!(
                "{}:{}",
                host,
                if path.is_empty() { "/" } else { path.as_str() }
            ),
            Protocol::Sshfs => {
                let user = if self.username.is_empty() {
                    String::new()
                } else {
                    format!("{}@", self.username)
                };
                format!(
                    "{}{}:{}",
                    user,
                    host,
                    if path.is_empty() { "/" } else { path.as_str() }
                )
            }
            Protocol::WebDav => {
                let scheme = if self.port == Some(80) {
                    "http"
                } else {
                    "https"
                };
                let port = match self.port {
                    Some(p) if Some(p) != default_webdav_port(scheme) => format!(":{}", p),
                    _ => String::new(),
                };
                format!(
                    "{}://{}{}{}",
                    scheme,
                    host,
                    port,
                    if path.is_empty() { "/" } else { path.as_str() }
                )
            }
            Protocol::Ftp => {
                let port = match self.port {
                    Some(p) if p != 21 => format!(":{}", p),
                    _ => String::new(),
                };
                format!(
                    "ftp://{}{}{}",
                    host,
                    port,
                    if path.is_empty() { "/" } else { path.as_str() }
                )
            }
            Protocol::Block => self.local_source.trim().to_string(),
            Protocol::Bind => self.local_source.trim().to_string(),
            Protocol::Custom => self.custom_source.trim().to_string(),
        }
    }

    /// Source string safe to show in logs (identical to [`Self::source`], kept
    /// separate so the intent is obvious at call sites).
    pub fn display_source(&self) -> String {
        self.source()
    }

    /// Effective `-t` value.
    pub fn fstype(&self) -> Option<String> {
        match self.protocol {
            Protocol::Custom => {
                let t = self.custom_fstype.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }
            Protocol::Block => {
                // lsblk already told us the fs; otherwise let mount(8) guess.
                let t = self.custom_fstype.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }
            p => p.fstype().map(|s| s.to_string()),
        }
    }

    /// GVFS URI (`smb://user@host/share`) used by [`MountMethod::Gvfs`].
    /// `with_password` embeds credentials — only used right before `gio mount`.
    pub fn gvfs_uri(&self, with_password: bool) -> Option<String> {
        let scheme = self.protocol.gvfs_scheme()?;
        let host = self.host.trim();
        let path = normalize_remote_path(&self.remote_path);
        let path = path.trim_start_matches('/');
        let userinfo = match (
            self.username.is_empty(),
            with_password,
            self.password.is_empty(),
        ) {
            (true, _, _) => String::new(),
            (false, false, _) => format!("{}@", uri_escape(&self.username)),
            (false, true, true) => format!("{}@", uri_escape(&self.username)),
            (false, true, false) => {
                format!(
                    "{}:{}@",
                    uri_escape(&self.username),
                    uri_escape(&self.password)
                )
            }
        };
        let port = match self.port {
            Some(p) if self.protocol.default_port() != Some(p) => format!(":{}", p),
            _ => String::new(),
        };
        Some(format!(
            "{}://{}{}{}{}",
            scheme,
            userinfo,
            host,
            port,
            if path.is_empty() {
                String::new()
            } else {
                format!("/{path}")
            }
        ))
    }

    /// `-o` options shared by every system/FUSE mount.
    fn base_options(&self, uid: u32, gid: u32) -> Vec<String> {
        let mut opts = Vec::new();
        opts.push(if self.read_only {
            "ro".to_string()
        } else {
            "rw".to_string()
        });
        match self.protocol {
            Protocol::Cifs => {
                opts.push(format!("uid={uid}"));
                opts.push(format!("gid={gid}"));
                opts.push("file_mode=0664".to_string());
                opts.push("dir_mode=0775".to_string());
                opts.push("iocharset=utf8".to_string());
                if self.username.is_empty() {
                    opts.push("guest".to_string());
                }
                if !self.domain.trim().is_empty() {
                    opts.push(format!("domain={}", self.domain.trim()));
                }
            }
            // NFS uid/gid mapping is done by the server, so no local options.
            Protocol::Nfs => {}
            Protocol::Sshfs | Protocol::WebDav | Protocol::Ftp => {
                opts.push(format!("uid={uid}"));
                opts.push(format!("gid={gid}"));
                opts.push("file_mode=0664".to_string());
                opts.push("dir_mode=0775".to_string());
            }
            Protocol::Block => {
                let f = self.custom_fstype.to_ascii_lowercase();
                if matches!(
                    f.as_str(),
                    "vfat" | "exfat" | "ntfs" | "ntfs3" | "fuseblk" | "iso9660" | "udf" | ""
                ) {
                    opts.push(format!("uid={uid}"));
                    opts.push(format!("gid={gid}"));
                }
            }
            Protocol::Bind | Protocol::Custom => {}
        }
        if self.protocol.is_network() {
            opts.push("_netdev".to_string());
        }
        opts.extend(
            self.extra_options
                .iter()
                .map(|o| o.trim().to_string())
                .filter(|o| !o.is_empty()),
        );
        dedupe_options(opts)
    }

    /// Ordered `mount(8)` attempts: most specific first, with sane fallbacks
    /// for protocol/version quirks (old SMB servers, NFSv3-only exports, FUSE
    /// without `user_allow_other`, ...).
    pub fn attempts(&self, caller: &platform::Caller) -> Vec<MountAttempt> {
        let uid = caller.uid;
        let gid = caller.gid;
        let source = self.source();
        let base = self.base_options(uid, gid);
        let with = |extra: &[&str]| -> Vec<String> {
            let mut o = base.clone();
            o.extend(extra.iter().map(|s| s.to_string()));
            dedupe_options(o)
        };

        match self.protocol {
            Protocol::Cifs => {
                let mut v = Vec::new();
                // Let the kernel negotiate first, then try explicit versions.
                v.push(
                    MountAttempt::new(source.clone())
                        .fstype("cifs")
                        .options(base.clone())
                        .label("cifs (negotiated)"),
                );
                for vers in ["vers=3.1.1", "vers=3.0", "vers=2.1", "vers=1.0"] {
                    v.push(
                        MountAttempt::new(source.clone())
                            .fstype("cifs")
                            .options(with(&[vers]))
                            .label(format!("cifs {vers}")),
                    );
                }
                v
            }
            Protocol::Nfs => {
                let mut v = Vec::new();
                v.push(
                    MountAttempt::new(source.clone())
                        .fstype("nfs")
                        .options(base.clone())
                        .label("nfs (negotiated)"),
                );
                v.push(
                    MountAttempt::new(source.clone())
                        .fstype("nfs4")
                        .options(base.clone())
                        .label("nfs4"),
                );
                v.push(
                    MountAttempt::new(source.clone())
                        .fstype("nfs")
                        .options(with(&["vers=3", "nolock"]))
                        .label("nfs v3"),
                );
                v
            }
            Protocol::Sshfs => {
                let mut opts = with(&[
                    "default_permissions",
                    "reconnect",
                    "ServerAliveInterval=15",
                    "ServerAliveCountMax=3",
                    "follow_symlinks",
                ]);
                if let Some(p) = self.port {
                    opts.push(format!("port={p}"));
                }
                if !self.identity_file.trim().is_empty() {
                    opts.push(format!("IdentityFile={}", self.identity_file.trim()));
                    opts.push("BatchMode=yes".to_string());
                }
                opts.push(format!(
                    "UserKnownHostsFile={}/.ssh/known_hosts",
                    caller.home.display()
                ));
                opts.push("StrictHostKeyChecking=accept-new".to_string());
                let opts = dedupe_options(opts);
                let mut with_other = opts.clone();
                with_other.push("allow_other".to_string());
                vec![
                    MountAttempt::new(source.clone())
                        .fstype("fuse.sshfs")
                        .options(with_other)
                        .label("sshfs (allow_other)"),
                    MountAttempt::new(source.clone())
                        .fstype("fuse.sshfs")
                        .options(opts)
                        .label("sshfs"),
                ]
            }
            Protocol::WebDav => vec![MountAttempt::new(source.clone())
                .fstype("davfs")
                .options(base.clone())
                .label("davfs")],
            Protocol::Ftp => {
                // curlftpfs takes credentials through the option string or netrc.
                let mut opts = base.clone();
                if !self.username.is_empty() {
                    opts.push(format!("user={}:{}", self.username, self.password));
                }
                opts.push("utf8".to_string());
                vec![MountAttempt::new(format!("curlftpfs#{source}"))
                    .fstype("fuse.curlftpfs")
                    .options(dedupe_options(opts))
                    .label("curlftpfs")]
            }
            Protocol::Block => {
                let mut v = Vec::new();
                if let Some(t) = self.fstype() {
                    v.push(
                        MountAttempt::new(source.clone())
                            .fstype(t.clone())
                            .options(base.clone())
                            .label(format!("{t}")),
                    );
                    // ntfs: prefer the in-kernel driver, then ntfs-3g.
                    if t == "ntfs" {
                        v.push(
                            MountAttempt::new(source.clone())
                                .fstype("ntfs3")
                                .options(base.clone())
                                .label("ntfs3"),
                        );
                    }
                }
                v.push(
                    MountAttempt::new(source.clone())
                        .options(base.clone())
                        .label("auto-detect"),
                );
                v
            }
            Protocol::Bind => {
                vec![MountAttempt::new(source.clone())
                    .flag("--bind")
                    .label("bind")]
            }
            Protocol::Custom => {
                let mut a = MountAttempt::new(source.clone())
                    .options(base.clone())
                    .label("custom");
                if let Some(t) = self.fstype() {
                    a = a.fstype(t);
                }
                vec![a]
            }
        }
    }

    /// Post-mount fixups: read-only bind mounts need a `remount,ro` step, and
    /// "take ownership" chowns the mount point afterwards.
    pub fn post_step(&self, caller: &platform::Caller) -> Option<crate::ops::PostStep> {
        match self.protocol {
            Protocol::Bind if self.read_only => Some(crate::ops::PostStep::RemountRo),
            _ if self.take_ownership => {
                Some(crate::ops::PostStep::ChownTarget(crate::ops::OwnerSpec {
                    uid: caller.uid,
                    gid: caller.gid,
                }))
            }
            _ => None,
        }
    }

    /// FUSE command line for [`MountMethod::FuseUser`] (run without root).
    pub fn fuse_argv(&self, caller: &platform::Caller) -> Option<Vec<String>> {
        let program = self.protocol.fuse_program()?;
        let mut argv = vec![program.to_string()];
        let source = match self.protocol {
            Protocol::Ftp => {
                let creds = if self.username.is_empty() {
                    String::new()
                } else if self.password.is_empty() {
                    format!("{}@", uri_escape(&self.username))
                } else {
                    format!(
                        "{}:{}@",
                        uri_escape(&self.username),
                        uri_escape(&self.password)
                    )
                };
                format!(
                    "ftp://{}{}",
                    creds,
                    self.host.trim().trim_start_matches("ftp://")
                )
            }
            _ => self.source(),
        };
        argv.push(source);
        argv.push(self.mount_point.clone());
        let mut opts = Vec::new();
        opts.push(format!("uid={}", caller.uid));
        opts.push(format!("gid={}", caller.gid));
        if self.read_only {
            opts.push("ro".to_string());
        }
        match self.protocol {
            Protocol::Sshfs => {
                if let Some(p) = self.port {
                    opts.push(format!("port={p}"));
                }
                if !self.identity_file.trim().is_empty() {
                    opts.push(format!("IdentityFile={}", self.identity_file.trim()));
                }
                opts.push("reconnect".to_string());
                opts.push("ServerAliveInterval=15".to_string());
                opts.push("follow_symlinks".to_string());
                opts.push("StrictHostKeyChecking=accept-new".to_string());
            }
            Protocol::Ftp => opts.push("utf8".to_string()),
            _ => {}
        }
        opts.extend(self.extra_options.iter().cloned());
        if !opts.is_empty() {
            argv.push("-o".to_string());
            argv.push(dedupe_options(opts).join(","));
        }
        Some(argv)
    }

    /// `/etc/fstab` lines (marker comment + entry) for "mount at login".
    pub fn fstab_lines(&self, caller: &platform::Caller, cred_path: Option<&str>) -> Vec<String> {
        let target = escape_fstab(&self.mount_point);
        let source = escape_fstab(&self.source());
        let mut opts = self.base_options(caller.uid, caller.gid);

        match self.protocol {
            Protocol::Cifs => {
                // Never store the password in fstab: reference a root only file.
                opts.retain(|o| !o.starts_with("password=") && o != "guest");
                match cred_path {
                    Some(p) => opts.push(format!("credentials={}", p)),
                    None => opts.push("guest".to_string()),
                }
            }
            Protocol::WebDav => {
                if let Some(p) = cred_path {
                    opts.push(format!("secrets={p}"));
                }
            }
            _ => {}
        }
        if self.protocol.is_network() {
            opts.push("nofail".to_string());
            opts.push("x-systemd.mount-timeout=30".to_string());
        } else {
            opts.push("nofail".to_string());
        }
        if self.protocol == Protocol::Bind {
            opts.push("bind".to_string());
        }
        let opts = dedupe_options(opts);
        let fstype = self.fstype().unwrap_or_else(|| "auto".to_string());
        vec![
            format!(
                "# mount-manager: id={} name=\"{}\"",
                self.id(),
                self.display_name()
            ),
            format!("{} {} {} {} 0 0", source, target, fstype, opts.join(",")),
        ]
    }

    /// Suggested mount point when the user leaves the field empty.
    pub fn suggested_mount_point(&self, _caller: &platform::Caller) -> String {
        let root = platform::default_mount_root();
        let slug = match self.protocol {
            Protocol::Block => {
                let base = if self.name.trim().is_empty() {
                    self.local_source.rsplit('/').next().unwrap_or("device")
                } else {
                    self.name.trim()
                };
                platform::slug(base)
            }
            Protocol::Bind => platform::slug(
                self.local_source
                    .trim()
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("bind"),
            ),
            _ => {
                let share = self
                    .remote_path
                    .trim()
                    .trim_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let base = if share.is_empty() {
                    self.host.trim().to_string()
                } else {
                    share
                };
                platform::slug(&base)
            }
        };
        root.join(slug).to_string_lossy().to_string()
    }

    /// Validate the request and fill in defaults (name, mount point, port).
    pub fn normalize(mut self, caller: &platform::Caller) -> Result<Self> {
        self.host = self.host.trim().trim_end_matches('/').to_string();
        self.remote_path = self.remote_path.trim().to_string();
        self.local_source = self.local_source.trim().to_string();
        self.custom_source = self.custom_source.trim().to_string();
        self.username = self.username.trim().to_string();
        self.domain = self.domain.trim().to_string();
        self.name = self.name.trim().to_string();

        // WebDAV accepts a full URL pasted into the host field.
        if self.protocol == Protocol::WebDav {
            if let Some(rest) = self.host.strip_prefix("https://") {
                self.port = self.port.or(Some(443));
                let (h, p) = split_host_path(rest);
                self.host = h;
                if self.remote_path.is_empty() {
                    self.remote_path = p;
                }
            } else if let Some(rest) = self.host.strip_prefix("http://") {
                self.port = self.port.or(Some(80));
                let (h, p) = split_host_path(rest);
                self.host = h;
                if self.remote_path.is_empty() {
                    self.remote_path = p;
                }
            }
        }

        if self.port.is_none() {
            self.port = self.protocol.default_port();
        }
        if self.name.is_empty() {
            self.name = self.display_name();
        }
        if self.mount_point.trim().is_empty() {
            self.mount_point = self.suggested_mount_point(caller);
        }
        self.mount_point = self.mount_point.trim().trim_end_matches('/').to_string();
        if self.mount_point.is_empty() {
            self.mount_point = "/".to_string();
        }

        // Normalise remote paths per protocol.
        match self.protocol {
            Protocol::Cifs => {
                self.remote_path = self.remote_path.trim_matches('/').to_string();
                if self.remote_path.is_empty() {
                    return Err(Error::new(
                        "SMB needs a share name (for example `public` or `backups`).",
                    ));
                }
            }
            Protocol::Nfs => {
                if self.remote_path.is_empty() {
                    self.remote_path = "/".to_string();
                } else if !self.remote_path.starts_with('/') {
                    self.remote_path = format!("/{}", self.remote_path);
                }
            }
            Protocol::Sshfs => {
                if self.remote_path.is_empty() {
                    self.remote_path = "/".to_string();
                } else if !self.remote_path.starts_with('/') && !self.remote_path.starts_with('~') {
                    self.remote_path = format!("/{}", self.remote_path);
                }
            }
            _ => {}
        }

        self.validate()?;
        Ok(self)
    }

    /// Reject anything that could damage the system or that cannot work.
    pub fn validate(&self) -> Result<()> {
        let target = platform::normalize_absolute(&self.mount_point)?;
        let target_str = target.to_string_lossy().to_string();
        if platform::is_protected_target(&target_str) {
            return Err(Error::new(format!(
                "`{target_str}` is a protected system path and cannot be used as a mount point."
            )));
        }

        match self.protocol {
            Protocol::Cifs => {
                if self.host.is_empty() {
                    return Err(Error::new("SMB needs a server name or IP address."));
                }
                reject_spaces(&self.host, "server")?;
                reject_spaces(&self.remote_path, "share name")?;
            }
            Protocol::Nfs => {
                if self.host.is_empty() {
                    return Err(Error::new("NFS needs a server name or IP address."));
                }
                reject_spaces(&self.host, "server")?;
                reject_spaces(&self.remote_path, "export path")?;
            }
            Protocol::Sshfs => {
                if self.host.is_empty() {
                    return Err(Error::new("SSHFS needs a server name or IP address."));
                }
                reject_spaces(&self.host, "server")?;
                if !self.identity_file.trim().is_empty() {
                    let p = PathBuf::from(self.identity_file.trim());
                    if !p.is_absolute() {
                        return Err(Error::new("The SSH key path must be absolute (for example /home/you/.ssh/id_ed25519)."));
                    }
                }
            }
            Protocol::WebDav => {
                if self.host.is_empty() {
                    return Err(Error::new("WebDAV needs a server name or full URL."));
                }
                reject_spaces(&self.host, "server")?;
            }
            Protocol::Ftp => {
                if self.host.is_empty() {
                    return Err(Error::new("FTP needs a server name or IP address."));
                }
                reject_spaces(&self.host, "server")?;
            }
            Protocol::Block | Protocol::Bind => {
                if self.local_source.is_empty() {
                    return Err(Error::new("Choose the device or directory to mount."));
                }
                let src = platform::normalize_absolute(&self.local_source)?;
                if self.protocol == Protocol::Block && !src.to_string_lossy().starts_with("/dev/") {
                    return Err(Error::new(format!(
                        "`{}` does not look like a block device (expected /dev/...).",
                        self.local_source
                    )));
                }
                if self.protocol == Protocol::Bind && !src.is_dir() {
                    return Err(Error::new(format!(
                        "`{}` is not an existing directory.",
                        self.local_source
                    )));
                }
            }
            Protocol::Custom => {
                if self.custom_source.is_empty() {
                    return Err(Error::new(
                        "Enter the mount source (for example `//nas/data` or `/dev/mapper/vg-lv`).",
                    ));
                }
            }
        }

        if let Some(p) = self.port {
            if p == 0 {
                return Err(Error::new("Port 0 is not valid."));
            }
        }
        for opt in &self.extra_options {
            let o = opt.trim();
            if o.is_empty() {
                continue;
            }
            if o.contains(char::is_whitespace) {
                return Err(Error::new(format!(
                    "Mount option `{o}` must not contain spaces."
                )));
            }
            if o.contains(',') {
                return Err(Error::new(format!(
                    "List mount options separately — `{o}` contains a comma."
                )));
            }
        }
        Ok(())
    }

    /// `true` when the request needs a password that we do not have.
    pub fn needs_password(&self) -> bool {
        match self.protocol {
            Protocol::Cifs => !self.username.is_empty(),
            Protocol::Sshfs => !self.username.is_empty() && self.identity_file.trim().is_empty(),
            Protocol::WebDav | Protocol::Ftp => !self.username.is_empty(),
            _ => false,
        }
    }
}

fn default_webdav_port(scheme: &str) -> Option<u16> {
    if scheme == "http" {
        Some(80)
    } else {
        Some(443)
    }
}

fn reject_spaces(value: &str, what: &str) -> Result<()> {
    if value.contains(char::is_whitespace) {
        return Err(Error::new(format!(
            "The {what} must not contain spaces (got `{value}`)."
        )));
    }
    Ok(())
}

fn split_host_path(input: &str) -> (String, String) {
    match input.find('/') {
        Some(i) => (input[..i].to_string(), input[i..].to_string()),
        None => (input.to_string(), "/".to_string()),
    }
}

/// Make sure a remote path is absolute-ish and has no trailing slash.
fn normalize_remote_path(path: &str) -> String {
    let p = path.trim();
    if p.is_empty() {
        return "/".to_string();
    }
    let p = p.strip_prefix("file://").unwrap_or(p);
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        "/".to_string()
    } else if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    }
}

/// Percent-encode the characters that are not allowed in a URI userinfo part.
pub fn uri_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Remove duplicate `-o` options, keeping the last occurrence.
pub fn dedupe_options(opts: Vec<String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for o in opts {
        let o = o.trim().trim_matches(',').to_string();
        if o.is_empty() {
            continue;
        }
        // `ro` and `rw` are the same knob, so they must not both survive.
        let key = match o.as_str() {
            "ro" | "rw" => "read_only".to_string(),
            other => other.split('=').next().unwrap_or(other).to_string(),
        };
        if let Some(pos) = seen.iter().position(|k| *k == key) {
            // Later value wins (user supplied options override defaults).
            out.remove(pos);
            seen.remove(pos);
        }
        seen.push(key);
        out.push(o);
    }
    out
}

/// Escape spaces/tabs for `/etc/fstab` fields.
pub fn escape_fstab(value: &str) -> String {
    value
        .replace('\\', "\\134")
        .replace(' ', "\\040")
        .replace('\t', "\\011")
        .replace('\n', "\\012")
}

/// Inverse of [`escape_fstab`].
pub fn unescape_fstab(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[i + 1..i + 4], 8) {
                out.push(byte);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Tiny non-cryptographic hash used to build stable ids.
pub fn fnv1a(input: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in input.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

// ─────────────────────────────────────────────────────────────────────────────
// Live mounts
// ─────────────────────────────────────────────────────────────────────────────

/// Why a mount is interesting to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MountKind {
    /// Remote filesystem (SMB, NFS, SSHFS, WebDAV, FTP, ...).
    Network,
    /// Removable/external block device.
    Removable,
    /// Internal, always-present disk.
    Fixed,
    /// Bind mount of another local directory.
    Bind,
    /// Mounted by GVFS inside the user session.
    Gvfs,
    /// Kernel/pseudo filesystem, hidden by default.
    System,
}

impl MountKind {
    pub fn label(self) -> &'static str {
        match self {
            MountKind::Network => "Network",
            MountKind::Removable => "Removable",
            MountKind::Fixed => "Internal disk",
            MountKind::Bind => "Bind",
            MountKind::Gvfs => "User session",
            MountKind::System => "System",
        }
    }

    /// Kinds shown in the default (filtered) view.
    pub fn is_user_visible(self) -> bool {
        !matches!(self, MountKind::System)
    }
}

/// One mounted filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountEntry {
    pub source: String,
    pub target: String,
    pub fstype: String,
    /// Superblock options (field 11 of mountinfo).
    pub super_options: Vec<String>,
    /// Per-mount options (field 6 of mountinfo).
    pub options: Vec<String>,
    pub protocol: Protocol,
    pub kind: MountKind,
    pub writable: bool,
    pub size: Option<u64>,
    pub used: Option<u64>,
    pub avail: Option<u64>,
    /// GVFS URI when this is a `fuse.gvfsd-fuse` mount.
    pub gvfs_uri: Option<String>,
    /// `true` when `/etc/fstab` has an entry for this target.
    pub in_fstab: bool,
    /// `true` when the fstab entry was written by Mount Manager.
    pub managed: bool,
}

impl MountEntry {
    /// Bold title in the mounts list.
    pub fn title(&self) -> String {
        match self.kind {
            MountKind::Gvfs => self.gvfs_uri.clone().unwrap_or_else(|| self.source.clone()),
            MountKind::Removable | MountKind::Fixed => {
                let label = self.label();
                if label.is_empty() {
                    self.source.clone()
                } else {
                    label
                }
            }
            _ => self.source.clone(),
        }
    }

    /// Volume label, when available.
    pub fn label(&self) -> String {
        for opt in self.super_options.iter().chain(self.options.iter()) {
            if let Some(rest) = opt.strip_prefix("LABEL=") {
                return rest.to_string();
            }
        }
        // `/dev/disk/by-label/FOO` style sources.
        if let Some(rest) = self.source.strip_prefix("/dev/disk/by-label/") {
            return crate::model::unescape_fstab(rest);
        }
        String::new()
    }

    /// Icon name for the list row.
    pub fn icon(&self) -> &'static str {
        match self.kind {
            MountKind::Network => self.protocol.icon(),
            MountKind::Removable => "drive-removable-media-symbolic",
            MountKind::Fixed => "drive-harddisk-symbolic",
            MountKind::Bind => "emblem-symbolic-link-symbolic",
            MountKind::Gvfs => "folder-remote-symbolic",
            MountKind::System => "drive-harddisk-system-symbolic",
        }
    }

    /// Short badge text (`SMB`, `nfs4`, `ext4`, ...).
    pub fn badge(&self) -> String {
        if self.protocol != Protocol::Custom {
            self.protocol.short().to_string()
        } else {
            self.fstype.clone()
        }
    }

    /// Subtitle: `on /mnt/nas · 12.3 GB free of 100 GB`.
    pub fn subtitle(&self) -> String {
        let mut parts = vec![format!("on {}", self.target)];
        if let (Some(size), Some(avail)) = (self.size, self.avail) {
            parts.push(format!(
                "{} free of {}",
                platform::human_size(avail),
                platform::human_size(size)
            ));
        }
        if !self.writable {
            parts.push("read-only".to_string());
        }
        parts.join(" · ")
    }

    /// `true` when unmounting should go through `gio` instead of `umount`.
    pub fn unmount_via_gvfs(&self) -> bool {
        self.kind == MountKind::Gvfs || self.fstype == "fuse.gvfsd-fuse"
    }

    /// `true` when the user can try `fusermount -u` before escalating.
    pub fn unmount_via_fuse(&self) -> bool {
        self.fstype.starts_with("fuse.") && !self.unmount_via_gvfs()
    }
}

/// A block device reported by `lsblk`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockDevice {
    pub name: String,
    pub path: String,
    pub label: String,
    pub fstype: String,
    pub size: u64,
    pub mountpoints: Vec<String>,
    pub removable: bool,
    pub model: String,
    pub vendor: String,
    pub kind: String,
    pub transport: String,
    pub uuid: String,
    /// Whole-disk parent (e.g. `sdb` for `sdb1`), empty for top level devices.
    pub parent: String,
}

impl BlockDevice {
    pub fn title(&self) -> String {
        let mut t = if self.label.is_empty() {
            self.model.clone()
        } else {
            self.label.clone()
        };
        if t.is_empty() {
            t = self.name.clone();
        }
        t
    }

    pub fn subtitle(&self) -> String {
        let mut parts = Vec::new();
        parts.push(self.path.clone());
        if !self.fstype.is_empty() {
            parts.push(self.fstype.clone());
        }
        if self.size > 0 {
            parts.push(platform::human_size(self.size));
        }
        if !self.model.is_empty() && self.model != self.label {
            parts.push(self.model.clone());
        }
        parts.join(" · ")
    }

    pub fn mounted(&self) -> bool {
        !self.mountpoints.is_empty()
    }

    /// Devices that are worth showing: have a filesystem, or are removable.
    pub fn interesting(&self) -> bool {
        (!self.fstype.is_empty() && self.kind != "loop") || self.removable
    }

    pub fn icon(&self) -> &'static str {
        if self.removable || self.transport == "usb" {
            "drive-removable-media-symbolic"
        } else if self.kind == "rom" {
            "drive-optical-symbolic"
        } else {
            "drive-harddisk-symbolic"
        }
    }
}

/// A share the user saved for later (config file + optional keyring secret).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedShare {
    pub id: String,
    pub request: ShareRequest,
    /// Keyring attribute value, or `b64:...` fallback blob.
    #[serde(default)]
    pub password_ref: Option<String>,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub last_used: Option<String>,
    /// Mount automatically from the CLI (`mount-manager auto-mount`).
    #[serde(default)]
    pub auto_mount: bool,
}

/// One line of the activity log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub time: String,
    pub level: LogLevel,
    pub message: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl LogLevel {
    pub fn icon(self) -> &'static str {
        match self {
            LogLevel::Info => "dialog-information-symbolic",
            LogLevel::Success => "object-select-symbolic",
            LogLevel::Warning => "dialog-warning-symbolic",
            LogLevel::Error => "dialog-error-symbolic",
        }
    }
}

/// ISO-8601-ish timestamp without external crates.
pub fn now_stamp() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, mi, s)
}

/// Days-from-civil algorithm (Howard Hinnant) — UTC.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let h = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller() -> platform::Caller {
        platform::Caller {
            uid: 1000,
            gid: 1000,
            name: "jo".into(),
            home: PathBuf::from("/home/jo"),
        }
    }

    fn req(p: Protocol) -> ShareRequest {
        ShareRequest {
            name: "NAS".into(),
            protocol: p,
            host: "nas.local".into(),
            remote_path: "data".into(),
            mount_point: "/mnt/nas".into(),
            username: "jo".into(),
            password: "s3cr3t".into(),
            ..Default::default()
        }
    }

    #[test]
    fn sources_per_protocol() {
        assert_eq!(req(Protocol::Cifs).source(), "//nas.local/data");
        assert_eq!(req(Protocol::Nfs).source(), "nas.local:/data");
        assert_eq!(req(Protocol::Sshfs).source(), "jo@nas.local:/data");
        assert_eq!(req(Protocol::WebDav).source(), "https://nas.local/data");
        assert_eq!(req(Protocol::Ftp).source(), "ftp://nas.local/data");
        let mut b = req(Protocol::Block);
        b.local_source = "/dev/sdb1".into();
        assert_eq!(b.source(), "/dev/sdb1");
    }

    #[test]
    fn gvfs_uri_masks_or_includes_password() {
        let r = req(Protocol::Cifs);
        assert_eq!(r.gvfs_uri(false).unwrap(), "smb://jo@nas.local/data");
        assert_eq!(r.gvfs_uri(true).unwrap(), "smb://jo:s3cr3t@nas.local/data");
    }

    #[test]
    fn password_never_lands_in_fstab() {
        let r = req(Protocol::Cifs);
        let lines = r.fstab_lines(&caller(), Some("/etc/mount-manager/credentials/x.cred"));
        let entry = &lines[1];
        assert!(
            entry.starts_with("//nas.local/data /mnt/nas cifs "),
            "{entry}"
        );
        assert!(entry.contains("credentials=/etc/mount-manager/credentials/x.cred"));
        assert!(!entry.contains("s3cr3t"));
        assert!(entry.contains("uid=1000"));
        assert!(entry.contains("_netdev"));
        assert!(entry.contains("nofail"));
    }

    #[test]
    fn cifs_attempt_ladder_ends_with_legacy_smb1() {
        let v = req(Protocol::Cifs).attempts(&caller());
        assert!(v.len() >= 4);
        assert!(v.last().unwrap().options.join(",").contains("vers=1.0"));
        assert!(v.iter().all(|a| a.fstype.as_deref() == Some("cifs")));
    }

    #[test]
    fn sshfs_tries_allow_other_first() {
        let v = req(Protocol::Sshfs).attempts(&caller());
        assert_eq!(v.len(), 2);
        assert!(v[0].options.iter().any(|o| o == "allow_other"));
        assert!(!v[1].options.iter().any(|o| o == "allow_other"));
        assert!(v[0]
            .options
            .iter()
            .any(|o| o == "UserKnownHostsFile=/home/jo/.ssh/known_hosts"));
    }

    #[test]
    fn bind_uses_bind_flag_and_remount_ro() {
        let mut r = req(Protocol::Bind);
        r.local_source = "/srv/data".into();
        r.read_only = true;
        let v = r.attempts(&caller());
        assert_eq!(v[0].flags, vec!["--bind"]);
        assert!(matches!(
            r.post_step(&caller()),
            Some(crate::ops::PostStep::RemountRo)
        ));
    }

    #[test]
    fn validation_rejects_dangerous_targets() {
        let mut r = req(Protocol::Cifs);
        r.mount_point = "/usr".into();
        assert!(r.normalize(&caller()).is_err());
        let mut r = req(Protocol::Cifs);
        r.mount_point = "relative/path".into();
        assert!(r.normalize(&caller()).is_err());
        let mut r = req(Protocol::Cifs);
        r.remote_path = "".into();
        assert!(r.normalize(&caller()).is_err());
    }

    #[test]
    fn normalize_fills_defaults() {
        let mut r = req(Protocol::Nfs);
        r.name = "".into();
        r.mount_point = "".into();
        r.remote_path = "".into();
        let n = r.normalize(&caller()).unwrap();
        assert_eq!(n.remote_path, "/");
        assert_eq!(n.port, Some(2049));
        assert!(
            n.mount_point.starts_with("/media/jo/") || n.mount_point.starts_with("/mnt/"),
            "{}",
            n.mount_point
        );
        assert!(!n.name.is_empty());
    }

    #[test]
    fn webdav_url_in_host_field_is_split() {
        let mut r = req(Protocol::WebDav);
        r.host = "https://files.example.com/remote.php/dav".into();
        r.remote_path = String::new();
        let n = r.normalize(&caller()).unwrap();
        assert_eq!(n.host, "files.example.com");
        assert_eq!(n.remote_path, "/remote.php/dav");
        assert_eq!(n.source(), "https://files.example.com/remote.php/dav");
    }

    #[test]
    fn options_dedupe_keeps_last() {
        let opts = dedupe_options(vec![
            "rw".into(),
            "uid=0".into(),
            "ro".into(),
            "uid=1000".into(),
        ]);
        assert_eq!(opts, vec!["ro", "uid=1000"]);
    }

    #[test]
    fn fstab_escaping_roundtrip() {
        let raw = "/mnt/my share";
        let esc = escape_fstab(raw);
        assert_eq!(esc, "/mnt/my\\040share");
        assert_eq!(unescape_fstab(&esc), raw);
    }

    #[test]
    fn uri_escape_encodes_specials() {
        assert_eq!(uri_escape("a b@c/d"), "a%20b%40c%2Fd");
    }

    #[test]
    fn timestamp_looks_sane() {
        let t = now_stamp();
        assert_eq!(t.len(), 19);
        assert_eq!(t.chars().nth(4), Some('-'));
        assert!(t.starts_with("20"));
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        let a = req(Protocol::Cifs);
        let mut b = req(Protocol::Cifs);
        b.remote_path = "other".into();
        assert_eq!(a.id(), a.clone().id());
        assert_ne!(a.id(), b.id());
        assert!(a.id().starts_with("NAS-"));
    }
}

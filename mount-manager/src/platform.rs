//! Information about the calling user, XDG locations and small path utilities.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// Effective uid (root when the helper runs elevated).
pub fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

/// Effective gid.
pub fn egid() -> u32 {
    unsafe { libc::getegid() }
}

pub fn is_root() -> bool {
    euid() == 0
}

/// The user that asked for the operation. When we run inside the helper (root
/// via pkexec/sudo) the real user is taken from the environment.
pub struct Caller {
    pub uid: u32,
    pub gid: u32,
    pub name: String,
    pub home: PathBuf,
}

impl Caller {
    /// Caller of the *current* process.
    pub fn current() -> Self {
        Self {
            uid: euid(),
            gid: egid(),
            name: user_name(),
            home: home_dir(),
        }
    }

    /// Caller when running as root on behalf of somebody else.
    pub fn from_env_if_root(fallback: &Caller) -> Self {
        if !is_root() {
            return fallback.clone();
        }
        let uid = env_u32("PKEXEC_UID")
            .or_else(|| env_u32("SUDO_UID"))
            .unwrap_or(fallback.uid);
        let gid = env_u32("PKEXEC_GID")
            .or_else(|| env_u32("SUDO_GID"))
            .unwrap_or(fallback.gid);
        let name = std::env::var("SUDO_USER")
            .ok()
            .filter(|n| !n.is_empty() && n != "root")
            .or_else(|| passwd_field(uid, 0).filter(|n| n != "root"))
            .unwrap_or_else(|| fallback.name.clone());
        let home = if name == fallback.name {
            fallback.home.clone()
        } else {
            home_for(&name).unwrap_or_else(|| fallback.home.clone())
        };
        Self {
            uid,
            gid,
            name,
            home,
        }
    }

    pub fn runtime_dir(&self) -> PathBuf {
        match std::env::var("XDG_RUNTIME_DIR") {
            Ok(d) if !d.is_empty() => PathBuf::from(d),
            _ => PathBuf::from(format!("/run/user/{}", self.uid)),
        }
    }
}

impl Clone for Caller {
    fn clone(&self) -> Self {
        Self {
            uid: self.uid,
            gid: self.gid,
            name: self.name.clone(),
            home: self.home.clone(),
        }
    }
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
}

/// Login name (`$USER`, falling back to the passwd database).
pub fn user_name() -> String {
    if let Ok(u) = std::env::var("USER") {
        if !u.is_empty() {
            return u;
        }
    }
    if let Ok(u) = std::env::var("LOGNAME") {
        if !u.is_empty() {
            return u;
        }
    }
    passwd_field(euid(), 0).unwrap_or_else(|| "user".to_string())
}

/// Home directory of the current user.
pub fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    home_for(&user_name()).unwrap_or_else(|| PathBuf::from("/root"))
}

/// Look up the home directory of `name` in `/etc/passwd`.
pub fn home_for(name: &str) -> Option<PathBuf> {
    let text = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in text.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 6 && f[0] == name {
            return Some(PathBuf::from(f[5]));
        }
    }
    None
}

/// Field 0 (name) of the passwd entry owning `uid`.
fn passwd_field(uid: u32, want: usize) -> Option<String> {
    let text = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in text.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() > want && f[2].parse::<u32>().ok() == Some(uid) {
            return Some(f[want].to_string());
        }
    }
    None
}

/// `/run/user/<uid>` of the current caller.
pub fn runtime_dir() -> PathBuf {
    Caller::current().runtime_dir()
}

/// Per-user configuration directory: `~/.config/mount-manager`.
pub fn config_dir() -> PathBuf {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(d) if !d.is_empty() => PathBuf::from(d).join("mount-manager"),
        _ => home_dir().join(".config").join("mount-manager"),
    }
}

/// Default root directory used when the user does not pick a mount point.
pub fn default_mount_root() -> PathBuf {
    let caller = Caller::current();
    let media = PathBuf::from("/media").join(&caller.name);
    if media.is_dir() || is_writable(&media) {
        media
    } else {
        PathBuf::from("/mnt")
    }
}

fn is_writable(p: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(p.to_string_lossy().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Filesystem size/used/free in bytes for `path` (all `None` when unavailable).
pub fn disk_usage(path: &Path) -> Option<(u64, u64, u64)> {
    let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    let frsize = st.f_frsize as u64;
    if frsize == 0 {
        return None;
    }
    let total = st.f_blocks as u64 * frsize;
    let free = st.f_bfree as u64 * frsize;
    let avail = st.f_bavail as u64 * frsize;
    if total == 0 {
        return None;
    }
    Some((total, total.saturating_sub(free), avail))
}

/// Human readable byte count (`1.5 GB`).
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value as u64, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// Turn an arbitrary label into a safe single path component.
pub fn slug(input: &str) -> String {
    let mut out = String::new();
    for c in input.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        } else if c.is_whitespace() || c == '/' || c == '\\' || c == ':' {
            out.push('-');
        }
        // Everything else (control chars, quotes, ...) is dropped.
    }
    let trimmed = out.trim_matches(|c| c == '-' || c == '.').to_string();
    let trimmed = if trimmed.is_empty() {
        "share".to_string()
    } else {
        trimmed
    };
    // Keep mount points short and readable.
    if trimmed.len() > 48 {
        trimmed.chars().take(48).collect::<String>()
    } else {
        trimmed
    }
}

/// Ensure `path` is absolute and free of `.`/`..` components *lexically*.
///
/// We deliberately do **not** resolve symlinks: `/media/user/data` must stay
/// exactly what the user typed so that `umount` matches the mountinfo record.
pub fn normalize_absolute(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(Error::new(format!("`{}` is not an absolute path", path)));
    }
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::RootDir => out.push("/"),
            std::path::Component::Normal(s) => out.push(s),
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return Err(Error::new(format!(
                        "`{}` escapes the filesystem root",
                        path
                    )));
                }
            }
            std::path::Component::CurDir => {}
            std::path::Component::Prefix(_) => {}
        }
    }
    Ok(out)
}

/// Paths that must never be unmounted or used as a mount target.
pub const PROTECTED_TARGETS: &[&str] = &[
    "/",
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/home",
    "/lib",
    "/lib32",
    "/lib64",
    "/proc",
    "/root",
    "/run",
    "/sbin",
    "/srv",
    "/sys",
    "/tmp",
    "/usr",
    "/var",
    "/boot/efi",
    "/snap",
    "/etc/hosts",
    "/etc/resolv.conf",
    "/etc/hostname",
];

/// Directories that must never contain a mount point created by this app —
/// mounting over anything in here can hide system files or break a boot.
pub const PROTECTED_PREFIXES: &[&str] = &[
    "/etc/",
    "/usr/",
    "/bin/",
    "/sbin/",
    "/lib/",
    "/lib32/",
    "/lib64/",
    "/boot/",
    "/dev/",
    "/proc/",
    "/sys/",
    "/snap/",
    "/var/lib/",
    "/var/run/",
    "/run/dbus/",
    "/run/user/",
    "/run/lock/",
];

/// `true` when `target` must not be mounted on or unmounted.
///
/// Refuses exact matches ([`PROTECTED_TARGETS`]), anything *inside* a system
/// directory ([`PROTECTED_PREFIXES`]) and anything that is an ancestor of a
/// protected path (mounting over `/u` would hide `/usr`).
pub fn is_protected_target(target: &str) -> bool {
    let t = target.trim_end_matches('/');
    let t = if t.is_empty() { "/" } else { t };

    if PROTECTED_PREFIXES
        .iter()
        .any(|prefix| t.starts_with(prefix))
    {
        return true;
    }
    for p in PROTECTED_TARGETS {
        let p = p.trim_end_matches('/');
        let p = if p.is_empty() { "/" } else { p };
        if t == p {
            return true;
        }
        if p.starts_with(&format!("{t}/")) && t != "/" {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_path_safe() {
        assert_eq!(slug("My Share/Folder"), "My-Share-Folder");
        assert_eq!(slug(""), "share");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("a b:c\\d"), "a-b-c-d");
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert!(human_size(5_000_000_000).ends_with("GB"));
    }

    #[test]
    fn normalize_rejects_relative_and_escaping() {
        assert_eq!(
            normalize_absolute("/mnt/a/./b/").unwrap(),
            PathBuf::from("/mnt/a/b")
        );
        assert!(normalize_absolute("mnt/a").is_err());
        assert!(normalize_absolute("/mnt/../../etc").is_err());
    }

    #[test]
    fn protected_targets() {
        assert!(is_protected_target("/"));
        assert!(is_protected_target("/usr"));
        assert!(is_protected_target("/etc/"));
        assert!(is_protected_target("/etc/fstab"));
        assert!(is_protected_target("/var/lib/docker"));
        assert!(is_protected_target("/run/user/1000/gvfs/x"));
        assert!(!is_protected_target("/media/user/nas"));
        assert!(!is_protected_target("/mnt/data"));
        assert!(!is_protected_target("/srv/nfs"));
        assert!(!is_protected_target("/home/user/shares/data"));
    }
}

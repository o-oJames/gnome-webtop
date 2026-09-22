//! Reading the live mount table (`/proc/self/mountinfo`) and classifying it.

use crate::error::{Error, Result};
use crate::model::{MountEntry, MountKind, Protocol};
use crate::{exec, fstab, platform};
use std::path::Path;

/// Pseudo filesystems that are pure kernel plumbing.
const SYSTEM_FSTYPES: &[&str] = &[
    "proc",
    "procfs",
    "sysfs",
    "devfs",
    "devpts",
    "tmpfs",
    "cgroup",
    "cgroup2",
    "pstore",
    "securityfs",
    "debugfs",
    "tracefs",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "binfmt_misc",
    "autofs",
    "efivarfs",
    "bpf",
    "ramfs",
    "nsfs",
    "rpc_pipefs",
    "selinuxfs",
    "overlay",
    "squashfs",
    "erofs",
    "fuse.lxcfs",
    "nsfs",
    "fuse.portal",
];

/// Mount points that are always system plumbing.
const SYSTEM_TARGET_PREFIXES: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/var/lib/docker",
    "/var/lib/snapd",
    "/snap",
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/hostname",
    "/tmp",
    "/boot",
];

/// Read every currently mounted filesystem.
pub fn list() -> Result<Vec<MountEntry>> {
    let mut entries = read_mounts()?;
    let fstab_entries = fstab::read().unwrap_or_default();
    let gvfs = gvfs_uris();

    for e in &mut entries {
        // /etc/fstab bookkeeping.
        if let Some(f) = fstab_entries.iter().find(|f| f.target == e.target) {
            e.in_fstab = true;
            e.managed = f.marker.is_some();
        }
        // GVFS URIs so the Files sidebar can be addressed directly.
        if e.kind == MountKind::Gvfs && e.gvfs_uri.is_none() {
            e.gvfs_uri = Some(resolve_gvfs_uri(&e.target, &gvfs));
        }
        // Sizes: skip pseudo filesystems, they are meaningless and slow.
        if e.kind != MountKind::System && statvfs_is_useful(e) {
            if let Some((size, used, avail)) = platform::disk_usage(Path::new(&e.target)) {
                e.size = Some(size);
                e.used = Some(used);
                e.avail = Some(avail);
            }
        }
    }

    // Stable, user friendly order: network first, then removable, then the rest.
    entries.sort_by(|a, b| {
        let rank = |e: &MountEntry| match e.kind {
            MountKind::Network => 0,
            MountKind::Gvfs => 1,
            MountKind::Removable => 2,
            MountKind::Fixed => 3,
            MountKind::Bind => 4,
            MountKind::System => 5,
        };
        rank(a).cmp(&rank(b)).then_with(|| a.target.cmp(&b.target))
    });
    Ok(entries)
}

/// `statvfs` only tells us something interesting for real filesystems.
fn statvfs_is_useful(e: &MountEntry) -> bool {
    !matches!(
        e.fstype.as_str(),
        "tmpfs" | "squashfs" | "autofs" | "overlay"
    )
}

/// Read and parse the mount table.
pub fn read_mounts() -> Result<Vec<MountEntry>> {
    let info = std::fs::read_to_string("/proc/self/mountinfo");
    let entries = match info {
        Ok(text) => parse_mountinfo(&text),
        Err(_) => {
            // Fallback for kernels/containers without mountinfo.
            let text = std::fs::read_to_string("/proc/mounts")
                .map_err(|e| Error::with_detail("Cannot read the mount table", e.to_string()))?;
            parse_proc_mounts(&text)
        }
    };
    Ok(entries)
}

/// Parse `/proc/self/mountinfo`.
///
/// Format: `36 35 98:0 /mnt1 /mnt/2 rw,noatime master:1 - ext3 /dev/root rw`
pub fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(' ').collect();
        if fields.len() < 10 {
            continue;
        }
        // Optional fields start at index 6 and run until the "-" separator.
        let Some(sep) = fields[6..].iter().position(|f| *f == "-") else {
            continue;
        };
        let sep = sep + 6;
        if fields.len() < sep + 3 {
            continue;
        }
        let root = unescape_mountinfo(fields[3]);
        let target = unescape_mountinfo(fields[4]);
        let options: Vec<String> = fields[5]
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        let fstype = fields[sep + 1].to_string();
        let source = unescape_mountinfo(fields[sep + 2]);
        let super_options: Vec<String> = fields[sep + 3]
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        out.push(build_entry(
            source,
            target,
            root,
            fstype,
            options,
            super_options,
        ));
    }
    out
}

/// Parse `/proc/mounts` (`device mountpoint fstype options dump pass`).
pub fn parse_proc_mounts(text: &str) -> Vec<MountEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let options: Vec<String> = f[3]
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        out.push(build_entry(
            unescape_mountinfo(f[0]),
            unescape_mountinfo(f[1]),
            // /proc/mounts has no "root" field; assume a full filesystem.
            "/".to_string(),
            f[2].to_string(),
            options,
            Vec::new(),
        ));
    }
    out
}

fn build_entry(
    source: String,
    target: String,
    root: String,
    fstype: String,
    options: Vec<String>,
    super_options: Vec<String>,
) -> MountEntry {
    let writable = !options.iter().any(|o| o == "ro");
    let kind = classify(&source, &target, &root, &fstype, &options);
    let protocol = match kind {
        MountKind::Gvfs => guess_gvfs_protocol(&source, &target),
        MountKind::Bind => Protocol::Bind,
        MountKind::Removable | MountKind::Fixed => Protocol::Block,
        _ => Protocol::from_fstype(&fstype),
    };
    MountEntry {
        source,
        target,
        fstype,
        super_options,
        options,
        protocol,
        kind,
        writable,
        size: None,
        used: None,
        avail: None,
        gvfs_uri: None,
        in_fstab: false,
        managed: false,
    }
}

/// Decide how interesting a mount is.
///
/// `root` is the mountinfo "root" field: a value other than `/` means the mount
/// only exposes part of a filesystem, i.e. a bind mount (or btrfs subvolume).
pub fn classify(
    source: &str,
    target: &str,
    root: &str,
    fstype: &str,
    options: &[String],
) -> MountKind {
    let f = fstype.to_ascii_lowercase();

    if f == "fuse.gvfsd-fuse" || (target.starts_with("/run/user/") && target.contains("/gvfs")) {
        // `/run/user/<uid>/gvfs` itself is plumbing; the shares mounted *below*
        // it are what the user cares about.
        let is_gvfs_root = target.trim_end_matches('/').ends_with("/gvfs");
        return if is_gvfs_root {
            MountKind::System
        } else {
            MountKind::Gvfs
        };
    }
    // Anything mounted on a system path (Docker's /etc/hosts, /etc/resolv.conf,
    // snap, /var/lib/docker overlays, …) is not something the user manages.
    if crate::platform::is_protected_target(target) {
        return MountKind::System;
    }
    if is_system_fstype(&f) {
        // tmpfs and friends are only interesting when mounted somewhere odd.
        if target == "/" || SYSTEM_TARGET_PREFIXES.iter().any(|p| target.starts_with(p)) {
            return MountKind::System;
        }
    }
    if is_network_fstype(&f) {
        return MountKind::Network;
    }
    if options.iter().any(|o| o == "bind") {
        return MountKind::Bind;
    }
    // A non-"/" root means we are looking at part of another filesystem.
    if !root.is_empty() && root != "/" && target != "/" && !f.starts_with("btrfs") {
        return MountKind::Bind;
    }
    if source.starts_with("/dev/") {
        if is_removable_source(source, &f) {
            return MountKind::Removable;
        }
        return MountKind::Fixed;
    }
    if source == "none" || source.is_empty() {
        return MountKind::Bind;
    }
    MountKind::System
}

fn is_network_fstype(fstype: &str) -> bool {
    const NETWORK: &[&str] = &[
        "cifs",
        "smb3",
        "smb2",
        "nfs",
        "nfs4",
        "sshfs",
        "fuse.sshfs",
        "davfs",
        "davfs2",
        "fuse.davfs2",
        "curlftpfs",
        "fuse.curlftpfs",
        "fuse.rclone",
        "rclone",
        "coda",
        "afs",
        "fuse.juicefs",
        "9p",
        "gfs2",
        "ocfs2",
        "fuse.glusterfs",
        "ceph",
        "lustre",
        "fuse.s3fs",
        "fuse.gdrive",
    ];
    NETWORK.contains(&fstype)
}

fn is_system_fstype(fstype: &str) -> bool {
    SYSTEM_FSTYPES.contains(&fstype)
}

/// USB sticks, SD cards and optical media — anything not a fixed system disk.
fn is_removable_source(source: &str, fstype: &str) -> bool {
    if matches!(
        fstype,
        "iso9660" | "udf" | "vfat" | "exfat" | "msdos" | "ntfs" | "ntfs3" | "fuseblk"
    ) {
        // fuseblk is almost always ntfs-3g on removable media.
        if source.contains("usb")
            || source.starts_with("/dev/sd")
            || source.starts_with("/dev/mmcblk")
        {
            return true;
        }
    }
    if source.starts_with("/dev/disk/by-path/") && source.contains("usb") {
        return true;
    }
    if source.starts_with("/dev/mmcblk") || source.starts_with("/dev/sr") {
        return true;
    }
    // /dev/sdXN is removable when the whole disk says so (checked via lsblk in
    // devices.rs); here we only treat sd* with a FAT/exFAT/NTFS fs as such.
    source.starts_with("/dev/sd")
        && matches!(
            fstype,
            "vfat" | "exfat" | "ntfs" | "ntfs3" | "iso9660" | "udf"
        )
}

/// Best-effort protocol detection for a GVFS fuse mount.
fn guess_gvfs_protocol(source: &str, target: &str) -> Protocol {
    let t = target.to_ascii_lowercase();
    let s = source.to_ascii_lowercase();
    if t.contains("smb") || s.contains("smb") {
        Protocol::Cifs
    } else if t.contains("sftp") || s.contains("sftp") || t.contains("ssh") {
        Protocol::Sshfs
    } else if t.contains("dav") {
        Protocol::WebDav
    } else if t.contains("ftp") {
        Protocol::Ftp
    } else if t.contains("nfs") {
        Protocol::Nfs
    } else {
        Protocol::Custom
    }
}

/// Resolve the GVFS URI behind a `fuse.gvfsd-fuse` mount point.
///
/// GVFS names its mount directories after the URI
/// (`/run/user/1000/gvfs/smb:host=nas,share=data`), so the URI can be rebuilt
/// offline; `gio mount -li` is consulted first when it is available.
pub fn resolve_gvfs_uri(target: &str, from_gio: &[(String, String)]) -> String {
    if let Some((_, uri)) = from_gio.iter().find(|(path, _)| path == target) {
        return uri.clone();
    }
    mountpoint_to_gvfs_uri(target)
}

/// `/run/user/1000/gvfs/smb:host=nas,share=data` → `smb://nas/data`.
pub fn mountpoint_to_gvfs_uri(target: &str) -> String {
    let base = target
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let Some((scheme, params)) = base.split_once(':') else {
        return format!("file://{target}");
    };
    let mut host = String::new();
    let mut user = String::new();
    let mut port = String::new();
    let mut path = String::new();
    let mut ssl = false;
    for pair in params.split(',') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let v = uri_unescape(v);
        match k {
            "host" => host = v,
            "user" => user = v,
            "port" => port = v,
            "share" | "prefix" | "path" => {
                let v = v.trim_start_matches('/');
                path = if v.is_empty() {
                    String::new()
                } else {
                    format!("/{v}")
                }
            }
            "ssl" | "tls" => ssl = v == "true" || v == "1",
            _ => {}
        }
    }
    if host.is_empty() {
        host = params.to_string();
    }
    let scheme = if ssl && scheme == "dav" {
        "davs"
    } else {
        scheme
    };
    let userinfo = if user.is_empty() {
        String::new()
    } else {
        format!("{user}@")
    };
    let port = if port.is_empty() || port == "0" {
        String::new()
    } else {
        format!(":{port}")
    };
    format!("{scheme}://{userinfo}{host}{port}{path}")
}

/// Decode `%2F` style escapes used in GVFS mount directory names.
pub fn uri_unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Ask `gio mount -li` for the URIs of every GVFS mount (user session only).
/// Returns `(local mount path, uri)` pairs.
pub fn gvfs_uris() -> Vec<(String, String)> {
    let Ok(out) = exec::run(
        &exec::Cmd::new("gio")
            .args(["mount", "-li"])
            .timeout(std::time::Duration::from_secs(8)),
    ) else {
        return Vec::new();
    };
    if !out.ok() {
        return Vec::new();
    }
    let mut result: Vec<(String, String)> = Vec::new();
    let mut current_uri: Option<String> = None;
    for line in out.stdout.lines() {
        let l = line.trim();
        if l.is_empty() {
            current_uri = None;
            continue;
        }
        // Block header: "Mount(0): NAS -> smb://nas/data"
        if l.starts_with("Mount(") {
            if let Some(rest) = l.split_once("): ").map(|(_, r)| r) {
                let uri = match rest.split_once(" -> ") {
                    Some((_, u)) => u.trim().to_string(),
                    None => rest.trim().to_string(),
                };
                current_uri = uri.contains("://").then_some(uri);
            }
            continue;
        }
        if let Some((key, value)) = l.split_once('=') {
            let value = value.trim().to_string();
            match key.trim() {
                "uri" if value.contains("://") => current_uri = Some(value),
                "default_location" | "path" | "x-content" => {
                    if let Some(uri) = current_uri.clone() {
                        if let Some(path) = value.strip_prefix("file://") {
                            if !result.iter().any(|(p, _)| p == path) {
                                result.push((path.to_string(), uri));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    result
}

/// `/proc/self/mountinfo` escapes spaces as `\040`.
pub fn unescape_mountinfo(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&value[i + 1..i + 4], 8) {
                out.push(b);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Find a live mount by target path (tolerates trailing slashes).
pub fn find_by_target(entries: &[MountEntry], target: &str) -> Option<MountEntry> {
    let want = target.trim_end_matches('/');
    let want = if want.is_empty() { "/" } else { want };
    entries
        .iter()
        .find(|e| e.target.trim_end_matches('/') == want)
        .cloned()
}

/// `true` when `target` currently has something mounted on it.
pub fn is_mounted(target: &str) -> bool {
    read_mounts()
        .map(|list| find_by_target(&list, target).is_some())
        .unwrap_or(false)
}

/// Processes holding `target` open, used to explain "target is busy".
pub fn busy_holders(target: &str) -> String {
    let mut lines = Vec::new();
    if let Ok(out) = exec::run(
        &exec::Cmd::new("fuser")
            .args(["-m", "-v", target])
            .timeout(std::time::Duration::from_secs(8)),
    ) {
        let text = out.combined();
        if !text.trim().is_empty() {
            lines.extend(text.lines().take(12).map(|l| l.trim().to_string()));
        }
    }
    if lines.is_empty() {
        if let Ok(out) = exec::run(
            &exec::Cmd::new("lsof")
                .args(["-F", "pcn", "--", target])
                .timeout(std::time::Duration::from_secs(8)),
        ) {
            lines.extend(out.stdout.lines().take(12).map(|l| l.trim().to_string()));
        }
    }
    if lines.is_empty() {
        "No process list available (install `psmisc` for `fuser`)".to_string()
    } else {
        lines.join("\n")
    }
}

/// Verify `/etc/fstab` with `findmnt --verify` when util-linux is new enough.
pub fn verify_fstab() -> Option<String> {
    let out = exec::run(
        &exec::Cmd::new("findmnt")
            .args(["--verify", "--verbose", "--tab-file", "/etc/fstab"])
            .timeout(std::time::Duration::from_secs(15)),
    )
    .ok()?;
    Some(out.combined())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
22 1 0:21 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw
23 1 0:22 / /sys rw,nosuid,nodev,noexec,relatime - sysfs sys rw
27 1 8:2 / / ext4 rw,relatime - ext4 /dev/sda2 rw,errors=remount-ro
30 27 0:26 / /tmp tmpfs rw,nosuid,nodev - tmpfs tmpfs rw
100 27 0:50 / /run/user/1000/gvfs/smb:host=nas,share=data rw,relatime - fuse.gvfsd-fuse gvfsd-fuse rw
101 27 0:60 / /mnt/nas rw,relatime - cifs //nas/data rw,vers=3.1.1,uid=1000
102 27 8:17 / /media/jo/STICK rw,nosuid,nodev,relatime - vfat /dev/sdb1 rw,fmask=0022
103 27 8:2 /srv /srv/data rw,relatime - ext4 /dev/sda2 rw
104 27 0:62 / /mnt/nfs rw,relatime - nfs4 10.0.0.1:/export rw,vers=4.2
105 27 0:61 / /mnt/sshfs rw,nosuid,nodev,relatime - fuse.sshfs jo@nas:/ rw
106 27 0:3 /my\\040dir /mnt/spaced rw,relatime - ext4 /dev/sda3 rw
";

    #[test]
    fn parses_mountinfo() {
        let v = parse_mountinfo(SAMPLE);
        assert_eq!(v.len(), 11);
        let root = v.iter().find(|e| e.target == "/").unwrap();
        assert_eq!(root.fstype, "ext4");
        assert!(root.writable);
        let spaced = v.iter().find(|e| e.target == "/mnt/spaced").unwrap();
        assert_eq!(spaced.source, "/dev/sda3");
    }

    #[test]
    fn container_plumbing_is_hidden() {
        let text = "500 27 254:1 /docker/containers/x/resolv.conf /etc/hosts rw,relatime - ext4 /dev/vda1 rw\n                    501 27 0:60 / /run/user/1001/gvfs rw,relatime - fuse.gvfsd-fuse gvfsd-fuse rw\n                    502 27 0:60 / /run/user/1001/gvfs/smb:host=nas,share=data rw,relatime - fuse.gvfsd-fuse gvfsd-fuse rw\n";
        let v = parse_mountinfo(text);
        let kind = |t: &str| v.iter().find(|e| e.target == t).map(|e| e.kind);
        assert_eq!(kind("/etc/hosts"), Some(MountKind::System));
        assert_eq!(kind("/run/user/1001/gvfs"), Some(MountKind::System));
        assert_eq!(
            kind("/run/user/1001/gvfs/smb:host=nas,share=data"),
            Some(MountKind::Gvfs)
        );
    }

    #[test]
    fn classification() {
        let v = parse_mountinfo(SAMPLE);
        let by = |t: &str| v.iter().find(|e| e.target == t).unwrap();
        assert_eq!(by("/proc").kind, MountKind::System);
        assert_eq!(by("/sys").kind, MountKind::System);
        assert_eq!(by("/tmp").kind, MountKind::System);
        // The root filesystem is system plumbing, not an "external path".
        assert_eq!(by("/").kind, MountKind::System);
        assert_eq!(
            by("/run/user/1000/gvfs/smb:host=nas,share=data").kind,
            MountKind::Gvfs
        );
        assert_eq!(by("/mnt/nas").kind, MountKind::Network);
        assert_eq!(by("/mnt/nas").protocol, Protocol::Cifs);
        assert_eq!(by("/media/jo/STICK").kind, MountKind::Removable);
        assert_eq!(by("/mnt/nfs").kind, MountKind::Network);
        assert_eq!(by("/mnt/nfs").protocol, Protocol::Nfs);
        assert_eq!(by("/mnt/sshfs").protocol, Protocol::Sshfs);
        assert_eq!(by("/srv/data").kind, MountKind::Bind);
    }

    #[test]
    fn gvfs_entry_gets_uri_and_writability() {
        let v = parse_mountinfo(SAMPLE);
        let g = v.iter().find(|e| e.kind == MountKind::Gvfs).unwrap();
        assert!(g.unmount_via_gvfs());
        assert!(!v
            .iter()
            .find(|e| e.target == "/mnt/nas")
            .unwrap()
            .unmount_via_gvfs());
        assert!(v
            .iter()
            .find(|e| e.target == "/mnt/sshfs")
            .unwrap()
            .unmount_via_fuse());
    }

    #[test]
    fn proc_mounts_fallback() {
        let text = "//nas/data /mnt/nas cifs rw,vers=3.0 0 0\n";
        let v = parse_proc_mounts(text);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].kind, MountKind::Network);
    }

    #[test]
    fn unescape_octal() {
        assert_eq!(unescape_mountinfo("/mnt/my\\040dir"), "/mnt/my dir");
        assert_eq!(unescape_mountinfo("/plain"), "/plain");
        assert_eq!(unescape_mountinfo("/tail\\"), "/tail\\");
    }

    #[test]
    fn find_by_target_tolerates_slash() {
        let v = parse_mountinfo(SAMPLE);
        assert!(find_by_target(&v, "/mnt/nas/").is_some());
        assert!(find_by_target(&v, "/mnt/missing").is_none());
    }

    #[test]
    fn gvfs_mountpoint_to_uri() {
        assert_eq!(
            mountpoint_to_gvfs_uri("/run/user/1000/gvfs/smb:host=nas,share=data"),
            "smb://nas/data"
        );
        assert_eq!(
            mountpoint_to_gvfs_uri("/run/user/1000/gvfs/smb:host=nas,share=data,user=jo"),
            "smb://jo@nas/data"
        );
        assert_eq!(
            mountpoint_to_gvfs_uri("/run/user/1000/gvfs/sftp:host=example.com,port=2222"),
            "sftp://example.com:2222"
        );
        assert_eq!(
            mountpoint_to_gvfs_uri(
                "/run/user/1000/gvfs/dav:host=example.com,ssl=true,prefix=%2Fremote.php%2Fdav"
            ),
            "davs://example.com/remote.php/dav"
        );
        assert_eq!(uri_unescape("%2Fremote%2F"), "/remote/");
    }
}

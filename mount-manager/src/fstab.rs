//! Safe reading and editing of `/etc/fstab`.
//!
//! Writing to fstab is one of the few ways a mount tool can render a machine
//! unbootable, so every change goes through [`write_atomic`]: a temporary file
//! in the same directory, `fsync`, `rename(2)`, and a backup of the previous
//! version. Entries created by this app carry a `# mount-manager:` marker so
//! they can be found and removed again.

use crate::error::{Error, Result};
use crate::model::{escape_fstab, unescape_fstab};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const PATH: &str = "/etc/fstab";
pub const BACKUP_PATH: &str = "/etc/fstab.mount-manager.bak";
pub const MARKER: &str = "# mount-manager:";

/// One parsed fstab entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FstabEntry {
    pub source: String,
    pub target: String,
    pub fstype: String,
    pub options: Vec<String>,
    pub dump: u32,
    pub pass: u32,
    /// Value of the `id=` field in the preceding `# mount-manager:` comment.
    pub marker: Option<String>,
    /// 1-based line number of the entry itself.
    pub line: usize,
    pub raw: String,
}

impl FstabEntry {
    pub fn option(&self, key: &str) -> Option<String> {
        self.options.iter().find_map(|o| {
            if o == key {
                Some(String::new())
            } else {
                o.strip_prefix(&format!("{key}=")).map(|v| v.to_string())
            }
        })
    }

    pub fn has_option(&self, key: &str) -> bool {
        self.options
            .iter()
            .any(|o| o == key || o.starts_with(&format!("{key}=")))
    }
}

/// Whole file: parsed entries plus the raw lines (needed for rewriting).
#[derive(Debug, Clone, Default)]
pub struct Fstab {
    pub entries: Vec<FstabEntry>,
    pub lines: Vec<String>,
}

/// Read and parse `/etc/fstab`. A missing file is not an error.
pub fn read() -> Result<Vec<FstabEntry>> {
    Ok(read_full()?.entries)
}

/// Read the file keeping the raw lines.
pub fn read_full() -> Result<Fstab> {
    let text = match fs::read_to_string(PATH) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(Error::with_detail(
                format!("Cannot read {PATH}"),
                e.to_string(),
            ))
        }
    };
    Ok(parse(&text))
}

/// Parse fstab content.
pub fn parse(text: &str) -> Fstab {
    let mut entries = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut pending_marker: Option<String> = None;

    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim();
        lines.push(raw.to_string());

        if trimmed.starts_with('#') || trimmed.is_empty() {
            if let Some(rest) = trimmed.strip_prefix(MARKER) {
                pending_marker = rest
                    .split_whitespace()
                    .find(|f| f.starts_with("id="))
                    .map(|f| f.trim_start_matches("id=").to_string());
            }
            continue;
        }
        // A non-comment line consumes any pending marker.
        let marker = pending_marker.take();
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        let options = fields[3]
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        entries.push(FstabEntry {
            source: unescape_fstab(fields[0]),
            target: unescape_fstab(fields[1]),
            fstype: fields[2].to_string(),
            options,
            dump: fields.get(4).and_then(|v| v.parse().ok()).unwrap_or(0),
            pass: fields.get(5).and_then(|v| v.parse().ok()).unwrap_or(0),
            marker,
            line: line_no,
            raw: raw.to_string(),
        });
    }
    Fstab { entries, lines }
}

/// Build the marker comment that precedes an app-managed entry.
pub fn marker_line(id: &str, name: &str) -> String {
    format!("{MARKER} id={id} name=\"{}\"", name.replace('"', "'"))
}

/// Append `lines` for `target`, replacing any previous entry for that target.
pub fn add(target: &str, new_lines: &[String], backup: bool) -> Result<()> {
    let mut fstab = read_full()?;
    if fstab
        .entries
        .iter()
        .any(|e| e.target == target && e.marker.is_none())
    {
        return Err(Error::new(format!(
            "`{target}` already has a hand written /etc/fstab entry. Edit it manually or remove it first."
        )));
    }
    // Replacing an existing managed entry is fine; a hand written one is not
    // (checked above).
    remove_target_lines(&mut fstab.lines, target);
    if backup {
        backup_file()?;
    }
    // Drop a trailing run of blank lines, then add a blank separator line.
    while fstab
        .lines
        .last()
        .map(|l| l.trim().is_empty())
        .unwrap_or(false)
    {
        fstab.lines.pop();
    }
    if !fstab.lines.is_empty() {
        fstab.lines.push(String::new());
    }
    fstab.lines.extend(new_lines.iter().cloned());
    write_atomic(&fstab.lines)
}

/// Remove the entry (and its marker comment) for `target`.
pub fn remove(target: &str, backup: bool) -> Result<bool> {
    let mut fstab = read_full()?;
    let removed = remove_target_lines(&mut fstab.lines, target);
    if removed == 0 {
        return Ok(false);
    }
    if backup {
        backup_file()?;
    }
    write_atomic(&fstab.lines)?;
    Ok(true)
}

/// Drop the entry lines for `target` plus any preceding marker comment.
/// Returns how many entry lines were removed.
fn remove_target_lines(lines: &mut Vec<String>, target: &str) -> usize {
    let escaped = escape_fstab(target);
    let mut removed = 0usize;
    let mut idx = 0;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        let is_entry = !trimmed.is_empty() && !trimmed.starts_with('#');
        let matches = is_entry
            && trimmed
                .split_whitespace()
                .nth(1)
                .map(|t| t == escaped || unescape_fstab(t) == target)
                .unwrap_or(false);
        if matches {
            // Also remove a marker comment directly above (and a blank line).
            if idx > 0 && lines[idx - 1].trim().starts_with(MARKER) {
                lines.remove(idx - 1);
                idx -= 1;
            }
            lines.remove(idx);
            removed += 1;
            continue; // do not advance: the next line moved into `idx`
        }
        idx += 1;
    }
    removed
}

/// Write the file back atomically, preserving mode/ownership.
pub fn write_atomic(lines: &[String]) -> Result<()> {
    let target = Path::new(PATH);
    let tmp = tmp_path(target);
    let (mode, uid, gid) = stat_of(target);

    let mut f = fs::File::create(&tmp).map_err(|e| {
        Error::with_detail(format!("Cannot create {}", tmp.display()), e.to_string())
    })?;
    for line in lines {
        writeln!(f, "{line}")
            .map_err(|e| Error::with_detail("Cannot write /etc/fstab", e.to_string()))?;
    }
    f.sync_all().ok();
    drop(f);

    if let Ok(c) = std::ffi::CString::new(tmp.to_string_lossy().as_bytes()) {
        unsafe {
            libc::chmod(c.as_ptr(), mode);
            libc::chown(c.as_ptr(), uid, gid);
        }
    }
    fs::rename(&tmp, target)
        .map_err(|e| Error::with_detail(format!("Cannot replace {PATH}"), e.to_string()))?;
    Ok(())
}

fn tmp_path(target: &Path) -> PathBuf {
    target.with_extension(format!("mount-manager.{}", std::process::id()))
}

fn stat_of(path: &Path) -> (u32, u32, u32) {
    match fs::metadata(path) {
        Ok(m) => {
            use std::os::unix::fs::MetadataExt;
            (m.mode() & 0o7777, m.uid(), m.gid())
        }
        Err(_) => (0o644, 0, 0),
    }
}

/// Copy the current fstab aside before changing it.
pub fn backup_file() -> Result<()> {
    match fs::read(PATH) {
        Ok(data) => {
            fs::write(BACKUP_PATH, &data).map_err(|e| {
                Error::with_detail(format!("Cannot write {BACKUP_PATH}"), e.to_string())
            })?;
            unsafe {
                if let Ok(c) = std::ffi::CString::new(BACKUP_PATH) {
                    libc::chmod(c.as_ptr(), 0o644);
                }
            }
            Ok(())
        }
        Err(_) => Ok(()), // nothing to back up
    }
}

/// Restore the most recent backup (used by `mount-manager fstab --restore`).
pub fn restore_backup() -> Result<()> {
    let data = fs::read(BACKUP_PATH)
        .map_err(|e| Error::with_detail(format!("No backup at {BACKUP_PATH}"), e.to_string()))?;
    let target = Path::new(PATH);
    let tmp = tmp_path(target);
    fs::write(&tmp, &data)
        .map_err(|e| Error::with_detail("Cannot write temporary fstab", e.to_string()))?;
    fs::rename(&tmp, target)
        .map_err(|e| Error::with_detail("Cannot restore /etc/fstab", e.to_string()))?;
    Ok(())
}

/// Entries that Mount Manager itself created.
pub fn managed_entries() -> Vec<FstabEntry> {
    read()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.marker.is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# /etc/fstab: static file system information.
#
# <file system> <mount point>   <type>  <options>       <dump>  <pass>
UUID=1234-5678 /               ext4    errors=remount-ro 0       1
UUID=ABCD       /home           ext4    defaults        0       2
# mount-manager: id=nas-data-1a2b3c4d name="NAS data"
//nas/data      /mnt/nas        cifs    credentials=/etc/mount-manager/credentials/nas.cred,uid=1000,_netdev,nofail 0 0
/tmp/swapfile   none            swap    sw              0       0
"#;

    #[test]
    fn parses_entries_and_markers() {
        let f = parse(SAMPLE);
        assert_eq!(f.entries.len(), 4);
        let nas = f.entries.iter().find(|e| e.target == "/mnt/nas").unwrap();
        assert_eq!(nas.marker.as_deref(), Some("nas-data-1a2b3c4d"));
        assert_eq!(nas.source, "//nas/data");
        assert_eq!(nas.fstype, "cifs");
        assert_eq!(
            nas.option("credentials").as_deref(),
            Some("/etc/mount-manager/credentials/nas.cred")
        );
        assert!(nas.has_option("_netdev"));
        assert!(f
            .entries
            .iter()
            .find(|e| e.target == "/")
            .unwrap()
            .marker
            .is_none());
    }

    #[test]
    fn removes_entry_with_its_marker() {
        let mut f = parse(SAMPLE);
        let before = f.lines.len();
        let n = remove_target_lines(&mut f.lines, "/mnt/nas");
        assert_eq!(n, 1);
        assert_eq!(f.lines.len(), before - 2);
        assert!(!f.lines.iter().any(|l| l.contains("mount-manager")));
        assert!(!f.lines.iter().any(|l| l.contains("//nas/data")));
        // Still parses and the other entries are untouched.
        let f2 = parse(&f.lines.join("\n"));
        assert_eq!(f2.entries.len(), 3);
        assert!(f2.entries.iter().any(|e| e.target == "/home"));
    }

    #[test]
    fn removes_nothing_for_unknown_target() {
        let mut f = parse(SAMPLE);
        assert_eq!(remove_target_lines(&mut f.lines, "/mnt/nope"), 0);
    }

    #[test]
    fn escaped_targets_are_matched() {
        let text = "# mount-manager: id=x name=\"my share\"\n//nas/data /mnt/my\\040share cifs defaults 0 0\n";
        let mut f = parse(text);
        assert_eq!(f.entries[0].target, "/mnt/my share");
        assert_eq!(remove_target_lines(&mut f.lines, "/mnt/my share"), 1);
        assert!(f.lines.iter().all(|l| !l.contains("nas")));
    }

    #[test]
    fn marker_line_format() {
        assert_eq!(
            marker_line("abc123", "My \"NAS\""),
            "# mount-manager: id=abc123 name=\"My 'NAS'\""
        );
    }

    #[test]
    fn handles_missing_trailing_newline_and_crlf() {
        let f = parse("UUID=1 /mnt/a ext4 defaults 0 0");
        assert_eq!(f.entries.len(), 1);
        let f = parse("UUID=1 /mnt/a ext4 defaults 0 0\r\n");
        assert_eq!(f.entries[0].target, "/mnt/a");
        assert_eq!(f.entries[0].pass, 0);
    }
}

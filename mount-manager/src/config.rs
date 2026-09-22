//! Persistent user configuration: saved shares and preferences.

use crate::error::{Error, Result};
use crate::model::{now_stamp, SavedShare, ShareRequest};
use crate::platform;
use crate::secret::{self, Backend};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// How the app should obtain root rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EscalationPref {
    /// pkexec (polkit) first, fall back to `sudo` with an in-app password prompt.
    #[default]
    Auto,
    /// Always use polkit / `pkexec`.
    Pkexec,
    /// Always use `sudo` (password typed inside the app).
    Sudo,
    /// Never escalate: only user-session (GVFS/FUSE) mounts are possible.
    Never,
}

impl EscalationPref {
    pub const ALL: [EscalationPref; 4] = [
        EscalationPref::Auto,
        EscalationPref::Pkexec,
        EscalationPref::Sudo,
        EscalationPref::Never,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EscalationPref::Auto => "Automatic (polkit, then sudo)",
            EscalationPref::Pkexec => "Always use polkit (pkexec)",
            EscalationPref::Sudo => "Always use sudo (ask inside the app)",
            EscalationPref::Never => "Never ask for admin rights",
        }
    }
}

/// Everything stored in `~/.config/mount-manager/config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Bumped when the on-disk format changes.
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub shares: Vec<SavedShare>,
    #[serde(default)]
    pub escalation: EscalationPref,
    /// Default parent directory for new mount points.
    #[serde(default)]
    pub mount_root: String,
    /// Offer to remember passwords for saved shares.
    #[serde(default = "default_true")]
    pub remember_passwords: bool,
    #[serde(default)]
    pub show_system_mounts: bool,
    /// Show every block device instead of only the interesting ones.
    #[serde(default)]
    pub show_all_devices: bool,
    /// Re-read the mount table every N seconds (0 = off).
    #[serde(default = "default_refresh")]
    pub refresh_secs: u32,
    /// Ask before a lazy (`umount -l`) unmount.
    #[serde(default = "default_true")]
    pub confirm_lazy_umount: bool,
}

fn default_version() -> u32 {
    1
}
fn default_true() -> bool {
    true
}
fn default_refresh() -> u32 {
    15
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            shares: Vec::new(),
            escalation: EscalationPref::Auto,
            mount_root: platform::default_mount_root().to_string_lossy().to_string(),
            remember_passwords: true,
            show_system_mounts: false,
            show_all_devices: false,
            refresh_secs: default_refresh(),
            confirm_lazy_umount: true,
        }
    }
}

impl AppConfig {
    pub fn dir() -> PathBuf {
        platform::config_dir()
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.json")
    }

    /// Load from disk, falling back to defaults for a missing/corrupt file.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<AppConfig>(&text) {
            Ok(mut cfg) => {
                if cfg.mount_root.trim().is_empty() {
                    cfg.mount_root = Self::default().mount_root;
                }
                cfg
            }
            Err(e) => {
                // Never lose the user's file silently: keep the broken one aside.
                let backup = path.with_extension("corrupt.json");
                let _ = fs::rename(&path, &backup);
                let mut cfg = Self::default();
                cfg.version = 0; // marker so the UI can mention the reset
                let _ = e;
                cfg
            }
        }
    }

    /// Save atomically with `0600` permissions.
    pub fn save(&self) -> Result<()> {
        let dir = Self::dir();
        fs::create_dir_all(&dir).map_err(|e| {
            Error::with_detail(format!("Cannot create {}", dir.display()), e.to_string())
        })?;
        set_mode(&dir, 0o700);

        let text = serde_json::to_string_pretty(self)
            .map_err(|e| Error::with_detail("Cannot serialise configuration", e.to_string()))?;
        let tmp = Self::path().with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp).map_err(|e| {
                Error::with_detail(format!("Cannot write {}", tmp.display()), e.to_string())
            })?;
            f.write_all(text.as_bytes())
                .map_err(|e| Error::with_detail("Cannot write configuration", e.to_string()))?;
            f.sync_all().ok();
        }
        set_mode(&tmp, 0o600);
        fs::rename(&tmp, Self::path()).map_err(|e| {
            Error::with_detail(
                format!("Cannot replace {}", Self::path().display()),
                e.to_string(),
            )
        })?;
        Ok(())
    }

    pub fn find(&self, id: &str) -> Option<&SavedShare> {
        self.shares.iter().find(|s| s.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut SavedShare> {
        self.shares.iter_mut().find(|s| s.id == id)
    }

    /// Insert or replace a share (matched by id).
    pub fn upsert(&mut self, share: SavedShare) {
        match self.shares.iter().position(|s| s.id == share.id) {
            Some(i) => self.shares[i] = share,
            None => self.shares.push(share),
        }
        self.shares
            .sort_by(|a, b| a.request.display_name().cmp(&b.request.display_name()));
    }

    /// Remove a share by id (also forgets its stored password).
    pub fn remove(&mut self, id: &str) -> Option<SavedShare> {
        let pos = self.shares.iter().position(|s| s.id == id)?;
        let share = self.shares.remove(pos);
        if let Some(reference) = &share.password_ref {
            secret::clear(reference);
        }
        Some(share)
    }

    /// Build a [`SavedShare`] from a request, storing the password when asked.
    pub fn make_saved(&self, request: &ShareRequest) -> SavedShare {
        let id = request.id();
        let existing = self.find(&id).cloned();
        let mut share = SavedShare {
            id: id.clone(),
            request: request.clone(),
            password_ref: existing.as_ref().and_then(|s| s.password_ref.clone()),
            created: existing
                .as_ref()
                .map(|s| s.created.clone())
                .unwrap_or_else(now_stamp),
            last_used: existing.as_ref().and_then(|s| s.last_used.clone()),
            auto_mount: existing.map(|s| s.auto_mount).unwrap_or(request.persist),
        };
        if request.remember_password && !request.password.is_empty() {
            let (reference, _backend) = secret::store(
                &id,
                &format!("Mount Manager — {}", request.display_name()),
                &request.password,
            );
            share.password_ref = reference;
        } else if !request.remember_password {
            if let Some(reference) = share.password_ref.take() {
                secret::clear(&reference);
            }
        }
        share
    }

    /// Fill `request.password` from the stored reference.
    pub fn password_for(&self, share: &SavedShare) -> Option<String> {
        share.password_ref.as_ref().and_then(|r| secret::resolve(r))
    }

    /// Which password backend would be used right now (for the UI hint).
    pub fn password_backend(&self) -> Backend {
        if !self.remember_passwords {
            Backend::None
        } else if secret::keyring_available() {
            Backend::Keyring
        } else {
            Backend::ConfigFile
        }
    }
}

fn set_mode(path: &std::path::Path, mode: u32) {
    if let Ok(c) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) {
        unsafe {
            libc::chmod(c.as_ptr(), mode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Protocol;

    /// The config tests change a process-wide environment variable, so they
    /// must not run concurrently with each other.
    static CONFIG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Point XDG_CONFIG_HOME at a temp dir so tests never touch the real config.
    /// The returned guard keeps the other config tests out until we are done.
    fn temp_config_dir(tag: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
        let guard = CONFIG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir =
            std::env::temp_dir().join(format!("mount-manager-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        fs::create_dir_all(AppConfig::dir()).unwrap();
        (guard, AppConfig::dir())
    }

    #[test]
    fn defaults_are_sane() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.escalation, EscalationPref::Auto);
        assert!(cfg.remember_passwords);
        assert!(!cfg.show_system_mounts);
        assert!(!cfg.mount_root.is_empty());
    }

    #[test]
    fn roundtrip_shares_and_prefs() {
        let (_guard, dir) = temp_config_dir("roundtrip");
        let mut cfg = AppConfig::default();
        cfg.show_system_mounts = true;
        let req = ShareRequest {
            name: "NAS".into(),
            protocol: Protocol::Cifs,
            host: "nas".into(),
            remote_path: "data".into(),
            mount_point: "/mnt/nas".into(),
            username: "jo".into(),
            password: "secret".into(),
            remember_password: false,
            ..Default::default()
        };
        let saved = cfg.make_saved(&req);
        cfg.upsert(saved);
        cfg.save().expect("save");

        let loaded = AppConfig::load();
        assert_eq!(loaded.shares.len(), 1);
        assert!(loaded.show_system_mounts);
        let share = &loaded.shares[0];
        assert_eq!(share.request.host, "nas");
        assert!(
            share.request.password.is_empty(),
            "passwords are never serialised"
        );
        assert!(share.password_ref.is_none(), "remember_password was false");

        // Upsert replaces instead of duplicating.
        let mut cfg2 = loaded;
        let mut req2 = req.clone();
        req2.mount_point = "/mnt/nas2".into();
        let saved2 = cfg2.make_saved(&req2);
        cfg2.upsert(saved2);
        assert_eq!(cfg2.shares.len(), 2);
        let id = req.id();
        cfg2.remove(&id);
        assert_eq!(cfg2.shares.len(), 1);
        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn corrupt_config_is_replaced_not_fatal() {
        let (_guard, dir) = temp_config_dir("corrupt");
        fs::write(AppConfig::path(), "{ not json").unwrap();
        let cfg = AppConfig::load();
        assert_eq!(cfg.version, 0);
        assert!(AppConfig::path().with_extension("corrupt.json").exists());
        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn config_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_guard, dir) = temp_config_dir("perms");
        AppConfig::default().save().unwrap();
        let mode = fs::metadata(AppConfig::path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "config must not be world readable");
        let dmode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dmode, 0o700);
        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}

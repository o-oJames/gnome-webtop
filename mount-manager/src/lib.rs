//! # Mount Manager
//!
//! Core (toolkit independent) logic for the Mount Manager application.
//!
//! The crate is split in three layers so that the privileged helper stays tiny
//! and the whole thing is unit-testable without a display server:
//!
//! * **model / mounts / fstab / devices / discover** — read-only inspection of
//!   the system (`/proc/self/mountinfo`, `/etc/fstab`, `lsblk`, network probes).
//! * **ops / executor** — the privileged action set. `executor::execute` is the
//!   *only* place where `mount(8)` / `umount(8)` / `/etc/fstab` are touched and
//!   it is shared by the in-process path (already root) and by
//!   `mount-manager-helper` (running as root through pkexec/sudo).
//! * **engine** — high level orchestration used by both the GTK user interface
//!   and the CLI.
//!
//! The GUI lives in [`ui`] and is only compiled with the `gui` feature.

pub mod cli;
pub mod config;
pub mod devices;
pub mod discover;
pub mod engine;
pub mod error;
pub mod exec;
pub mod executor;
pub mod fstab;
pub mod model;
pub mod mounts;
pub mod ops;
pub mod platform;
pub mod privilege;
pub mod secret;

#[cfg(feature = "gui")]
pub mod ui;

pub use error::{Error, Result};

/// Application id, also used for the .desktop file and the polkit vendor.
pub const APP_ID: &str = "io.github.mount_manager";
/// Human readable name.
pub const APP_NAME: &str = "Mount Manager";
/// Version, injected from Cargo.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// polkit action id installed in /usr/share/polkit-1/actions.
pub const POLKIT_ACTION: &str = "io.github.mount_manager.manage";
/// Default install location of the privileged helper.
pub const HELPER_PATH: &str = "/usr/lib/mount-manager/mount-manager-helper";
/// Environment override for the helper location (used by tests and `cargo run`).
pub const HELPER_ENV: &str = "MOUNT_MANAGER_HELPER";
/// Directory for root owned state (credential files, davfs secrets).
pub const STATE_DIR: &str = "/etc/mount-manager";

/// Short one-line summary used by `--version` and the About dialog.
pub fn about_line() -> String {
    format!("{APP_NAME} {VERSION} — mount SMB, NFS, SFTP, WebDAV, FTP and local drives")
}

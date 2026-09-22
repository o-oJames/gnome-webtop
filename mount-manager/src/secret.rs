//! Password storage.
//!
//! Two backends, chosen at runtime:
//!
//! 1. **Secret Service** (GNOME Keyring / KeePassXC) through the `secret-tool`
//!    command line client — the password never touches disk in plain text.
//! 2. **Fallback**: a base64 blob inside `~/.config/mount-manager/shares.json`,
//!    which is created with mode `0600`. This is *obfuscation, not
//!    encryption*; the UI says so when this backend is active.

use crate::exec::{self, Cmd};
use std::time::Duration;

const SCHEMA_KEY: &str = "mount-manager";

/// `true` when the Secret Service backend is usable.
pub fn keyring_available() -> bool {
    if !exec::exists("secret-tool") {
        return false;
    }
    // A cheap read also proves that a keyring daemon is running and unlocked:
    // `secret-tool lookup` exits non-zero when there is no Secret Service.
    exec::run(
        &Cmd::new("secret-tool")
            .args(["lookup", SCHEMA_KEY, "__probe__"])
            .timeout(Duration::from_secs(4)),
    )
    .map(|o| o.ok())
    .unwrap_or(false)
}

/// Store a password in the keyring. Returns `false` when unavailable.
pub fn keyring_store(id: &str, label: &str, password: &str) -> bool {
    let out = exec::run(
        &Cmd::new("secret-tool")
            .args(["store", &format!("--label={label}"), SCHEMA_KEY, id])
            .stdin_str(format!("{password}\n"))
            .timeout(Duration::from_secs(8)),
    );
    out.map(|o| o.ok()).unwrap_or(false)
}

/// Read a password from the keyring.
pub fn keyring_lookup(id: &str) -> Option<String> {
    let out = exec::run(
        &Cmd::new("secret-tool")
            .args(["lookup", SCHEMA_KEY, id])
            .timeout(Duration::from_secs(8)),
    )
    .ok()?;
    if !out.ok() {
        return None;
    }
    let value = out.stdout.trim_end_matches('\n').to_string();
    (!value.is_empty()).then_some(value)
}

/// Forget a keyring password.
pub fn keyring_clear(id: &str) -> bool {
    exec::run(
        &Cmd::new("secret-tool")
            .args(["clear", SCHEMA_KEY, id])
            .timeout(Duration::from_secs(8)),
    )
    .map(|o| o.ok())
    .unwrap_or(false)
}

/// Backend used for a saved share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Keyring,
    ConfigFile,
    None,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Keyring => "GNOME Keyring (encrypted at rest)",
            Backend::ConfigFile => "Config file (obfuscated, not encrypted)",
            Backend::None => "Not stored — you will be asked every time",
        }
    }
}

/// Store a password using the best available backend.
/// Returns the reference string to keep in the config file.
pub fn store(id: &str, label: &str, password: &str) -> (Option<String>, Backend) {
    if password.is_empty() {
        return (None, Backend::None);
    }
    if keyring_available() && keyring_store(id, label, password) {
        return (Some(format!("keyring:{id}")), Backend::Keyring);
    }
    (
        Some(format!("b64:{}", base64_encode(password.as_bytes()))),
        Backend::ConfigFile,
    )
}

/// Resolve a reference produced by [`store`].
pub fn resolve(reference: &str) -> Option<String> {
    if let Some(id) = reference.strip_prefix("keyring:") {
        return keyring_lookup(id);
    }
    if let Some(blob) = reference.strip_prefix("b64:") {
        return base64_decode(blob).map(|b| String::from_utf8_lossy(&b).into_owned());
    }
    None
}

/// Forget a stored password.
pub fn clear(reference: &str) {
    if let Some(id) = reference.strip_prefix("keyring:") {
        keyring_clear(id);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// base64 (no external crate)
// ─────────────────────────────────────────────────────────────────────────────

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::new();
    for c in input.chars() {
        if c == '=' || c.is_whitespace() {
            break;
        }
        let idx = B64.iter().position(|&b| b as char == c)? as u32;
        buf = (buf << 6) | idx;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        for text in [
            "",
            "f",
            "fo",
            "foo",
            "foob",
            "fooba",
            "foobar",
            "pässwörd",
            "a b@c:d/e",
        ] {
            let enc = base64_encode(text.as_bytes());
            let dec = base64_decode(&enc).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&dec),
                text,
                "failed for {text} ({enc})"
            );
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
    }

    #[test]
    fn store_and_resolve_fallback() {
        // The keyring is not available in CI/containers, so this exercises the
        // config-file path; on a desktop with a keyring it exercises that one.
        let (reference, backend) = store("test-id", "Test share", "hunter2");
        let reference = reference.expect("a reference must be produced");
        assert_eq!(resolve(&reference).as_deref(), Some("hunter2"));
        assert_ne!(backend, Backend::None);
        assert!(reference.starts_with("keyring:") || reference.starts_with("b64:"));
    }

    #[test]
    fn empty_password_is_not_stored() {
        let (reference, backend) = store("test-id", "Test share", "");
        assert!(reference.is_none());
        assert_eq!(backend, Backend::None);
    }
}

//! Getting root rights for a single operation.
//!
//! Order of preference (configurable through [`EscalationPref`]):
//!
//! 1. We are already root (e.g. the CLI was started with `sudo`) → run the
//!    executor in-process.
//! 2. `sudo -n` succeeds (passwordless sudo, common in containers) → no prompt.
//! 3. `pkexec` → polkit asks through the desktop's authentication agent
//!    (GNOME Shell) using the `io.github.mount_manager.manage` action.
//! 4. `sudo -S` with the password typed into the app's own dialog. This is the
//!    fallback that also works when polkit has no agent, which is the case in
//!    containers and minimal sessions.
//!
//! The password is kept in memory only, for the lifetime of the application,
//! and is handed to `sudo` through stdin — never on a command line.

use crate::config::EscalationPref;
use crate::error::{Error, Result};
use crate::exec::{self, Cmd};
use crate::ops::{Op, OpResult};
use crate::platform;
use crate::{executor, HELPER_ENV, HELPER_PATH};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// Internal sentinels used by the sudo retry logic (never shown to the user).
const NEEDS_PASSWORD: &str = "__mount_manager_needs_password__";
const AUTH_FAILED: &str = "__mount_manager_auth_failed__";

/// How a single privileged operation will be carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// We already have root.
    Root,
    /// `sudo -n` works without a password.
    SudoCached,
    /// polkit / `pkexec` dialog.
    Pkexec,
    /// In-app password dialog + `sudo -S`.
    SudoPassword,
    /// Escalation is disabled or impossible.
    None,
}

impl Method {
    pub fn label(self) -> &'static str {
        match self {
            Method::Root => "Running as root",
            Method::SudoCached => "Passwordless sudo available",
            Method::Pkexec => "System authentication dialog (polkit)",
            Method::SudoPassword => "Password prompt inside Mount Manager",
            Method::None => "No administrator rights",
        }
    }

    /// `true` when the user will see a password dialog.
    pub fn asks_for_password(self) -> bool {
        matches!(self, Method::Pkexec | Method::SudoPassword)
    }
}

/// What the UI needs to render a password dialog.
#[derive(Debug, Clone)]
pub struct PromptContext {
    pub reason: String,
    pub detail: String,
    pub username: String,
    pub attempt: u32,
    /// Set when a previous attempt was rejected, so the dialog can say why.
    pub previous_error: Option<String>,
}

impl PromptContext {
    pub fn new<S: Into<String>>(reason: S) -> Self {
        Self {
            reason: reason.into(),
            detail: String::new(),
            username: platform::user_name(),
            attempt: 1,
            previous_error: None,
        }
    }
}

/// A password plus what to do with it afterwards.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub password: String,
    /// Cache it in memory for the rest of the session (never written to disk).
    pub remember: bool,
}

impl Credentials {
    pub fn cached(password: String) -> Self {
        Self {
            password,
            remember: true,
        }
    }
}

/// Asks the user for their password. Implemented by the GTK dialog and by the
/// CLI (`/dev/tty` + `stty -echo`).
pub trait Prompter: Send + Sync {
    /// Return the password, or `None` when the user cancelled.
    fn password(&self, ctx: &PromptContext) -> Option<Credentials>;

    /// Optional: short status message (the GUI turns it into a toast).
    fn status(&self, _message: &str) {}
}

/// Prompter that refuses everything — used when no user interface is available.
pub struct NonInteractive;

impl Prompter for NonInteractive {
    fn password(&self, _ctx: &PromptContext) -> Option<Credentials> {
        None
    }
}

/// Decides how to escalate and runs [`Op`]s.
pub struct Runner {
    pref: Mutex<EscalationPref>,
    /// Resolved once, but may be downgraded (pkexec → sudo) after a failure.
    method: Mutex<Option<Method>>,
    password: Mutex<Option<String>>,
    helper: PathBuf,
}

impl Runner {
    pub fn new(pref: EscalationPref) -> Self {
        Self {
            pref: Mutex::new(pref),
            method: Mutex::new(None),
            password: Mutex::new(None),
            helper: helper_path(),
        }
    }

    /// Current escalation preference.
    pub fn pref(&self) -> EscalationPref {
        self.pref.lock().map(|p| *p).unwrap_or_default()
    }

    /// Change the preference and forget the cached detection result.
    pub fn set_pref(&self, pref: EscalationPref) {
        if let Ok(mut g) = self.pref.lock() {
            *g = pref;
        }
        if let Ok(mut g) = self.method.lock() {
            *g = None;
        }
    }

    pub fn helper(&self) -> &PathBuf {
        &self.helper
    }

    /// Where the helper binary is expected.
    pub fn helper_path() -> PathBuf {
        helper_path()
    }

    /// Remember a password for the rest of the session.
    pub fn set_password(&self, password: Option<String>) {
        if let Ok(mut slot) = self.password.lock() {
            *slot = password;
        }
    }

    pub fn has_password(&self) -> bool {
        self.password.lock().map(|p| p.is_some()).unwrap_or(false)
    }

    pub fn forget_password(&self) {
        self.set_password(None);
    }

    /// Detect (and cache) how escalation will happen.
    pub fn method(&self) -> Method {
        if let Some(m) = self.method.lock().ok().and_then(|g| *g) {
            return m;
        }
        let m = self.detect();
        if let Ok(mut g) = self.method.lock() {
            *g = Some(m);
        }
        m
    }

    /// Force a re-detection (e.g. after polkit failed).
    fn set_method(&self, m: Method) {
        if let Ok(mut g) = self.method.lock() {
            *g = Some(m);
        }
    }

    fn detect(&self) -> Method {
        if platform::is_root() {
            return Method::Root;
        }
        match self.pref() {
            EscalationPref::Never => Method::None,
            EscalationPref::Pkexec => {
                if exec::exists("pkexec") {
                    Method::Pkexec
                } else {
                    Method::None
                }
            }
            EscalationPref::Sudo => {
                if exec::exists("sudo") {
                    if sudo_probe() {
                        Method::SudoCached
                    } else {
                        Method::SudoPassword
                    }
                } else {
                    Method::None
                }
            }
            EscalationPref::Auto => {
                if exec::exists("sudo") && sudo_probe() {
                    // Passwordless sudo: cheapest and never blocks on a dialog.
                    return Method::SudoCached;
                }
                if exec::exists("pkexec") && polkit_available() {
                    return Method::Pkexec;
                }
                if exec::exists("sudo") {
                    return Method::SudoPassword;
                }
                Method::None
            }
        }
    }

    /// Short description for the preferences window.
    pub fn description(&self) -> String {
        let method = self.method();
        let mut text = format!("{} — {}", method.label(), self.helper.display());
        if method == Method::None {
            text.push_str("\nOnly user-session mounts (GVFS/FUSE) are possible.");
        }
        text
    }

    /// Run a privileged operation, asking for a password when needed.
    pub fn run(&self, op: &Op, prompter: &dyn Prompter) -> Result<OpResult> {
        if platform::is_root() {
            return Ok(executor::execute(op));
        }
        let method = self.method();
        match method {
            Method::Root => Ok(executor::execute(op)),
            Method::SudoCached => self.run_sudo(op, None, prompter),
            Method::Pkexec => match self.run_pkexec(op, prompter) {
                Ok(res) => Ok(res),
                Err(e) if e.is_cancelled() => Err(e),
                Err(e) => {
                    // polkit is unusable here (no agent, container, ...): fall
                    // back to sudo for the rest of the session.
                    if self.pref() == EscalationPref::Auto && exec::exists("sudo") {
                        self.set_method(Method::SudoPassword);
                        self.run_sudo(op, None, prompter)
                            .map_err(|e2| Error::with_detail(e.message, e2.to_string()))
                    } else {
                        Err(e)
                    }
                }
            },
            Method::SudoPassword => self.run_sudo(op, None, prompter),
            Method::None => Err(Error::new(format!(
                "Administrator rights are required to {} but escalation is disabled.",
                op.summary()
            ))
            .hint("Enable polkit/sudo, or choose a user-session (GVFS/FUSE) mount method.")),
        }
    }

    // ── pkexec ────────────────────────────────────────────────────────────
    fn run_pkexec(&self, op: &Op, prompter: &dyn Prompter) -> Result<OpResult> {
        let payload = serde_json::to_string(op)?;
        let cmd = Cmd::new("pkexec")
            .args([
                self.helper.to_string_lossy().to_string(),
                "--stdin".to_string(),
            ])
            .env("MOUNT_MANAGER_CALLER_UID", platform::euid().to_string())
            .stdin_str(payload + "\n")
            .timeout(Duration::from_secs(600));
        prompter.status(&format!("Asking for authentication ({})", op.summary()));
        let out = exec::run(&cmd)?;

        if let Some(res) = parse_helper_json(&out.stdout) {
            return Ok(res);
        }
        let stderr = out.combined();
        match out.code {
            // 126: the user dismissed the dialog.
            Some(126) => Err(Error::new("Authentication cancelled by the user")),
            // 127: not authorised, or polkit could not determine the session.
            Some(127) => Err(
                Error::with_detail("polkit could not authorise this action", stderr)
                    .hint("Falling back to sudo when possible."),
            ),
            Some(c) if c >= 128 => Err(Error::with_detail(
                format!("The helper did not run correctly (exit {c})"),
                stderr,
            )),
            None => Err(Error::with_detail(
                "The authentication helper timed out",
                stderr,
            )),
            _ => Err(Error::with_detail(
                format!("Administrator action failed (exit {:?})", out.code),
                stderr,
            )),
        }
    }

    // ── sudo ──────────────────────────────────────────────────────────────
    /// Run the helper through sudo, asking for a password when required.
    fn run_sudo(
        &self,
        op: &Op,
        password: Option<&str>,
        prompter: &dyn Prompter,
    ) -> Result<OpResult> {
        match self.sudo_once(op, password, prompter) {
            Ok(res) => {
                if let Some(pw) = password {
                    self.set_password(Some(pw.to_string()));
                }
                Ok(res)
            }
            // `sudo -n` told us a password is needed: ask inside the app.
            Err(e) if e.message == NEEDS_PASSWORD => {
                self.set_method(Method::SudoPassword);
                self.ask_and_retry(op, prompter, None)
            }
            Err(e) if e.message == AUTH_FAILED => {
                self.set_password(None);
                self.ask_and_retry(
                    op,
                    prompter,
                    Some("That password was not accepted.".to_string()),
                )
            }
            Err(e) => Err(e),
        }
    }

    /// Ask for a password and retry until it works, is cancelled, or three
    /// attempts failed.
    fn ask_and_retry(
        &self,
        op: &Op,
        prompter: &dyn Prompter,
        first_error: Option<String>,
    ) -> Result<OpResult> {
        let mut attempt = 1u32;
        let mut previous_error = first_error;
        loop {
            let Some(creds) = self.next_password(prompter, op, attempt, previous_error.clone())
            else {
                return Err(Error::new("Authentication cancelled by the user"));
            };
            match self.sudo_once(op, Some(&creds.password), prompter) {
                Ok(res) => {
                    self.set_password(creds.remember.then_some(creds.password));
                    return Ok(res);
                }
                Err(e) if e.message == AUTH_FAILED && attempt < 3 => {
                    attempt += 1;
                    previous_error = Some(format!(
                        "That password was not accepted (attempt {}).",
                        attempt - 1
                    ));
                    continue;
                }
                Err(e) if e.message == AUTH_FAILED => {
                    return Err(Error::with_detail(
                        "The password was not accepted after three attempts",
                        e.detail.unwrap_or_default(),
                    ));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// One `sudo` invocation. Auth problems are reported with sentinel messages
    /// so the callers above can decide whether to re-prompt.
    fn sudo_once(
        &self,
        op: &Op,
        password: Option<&str>,
        prompter: &dyn Prompter,
    ) -> Result<OpResult> {
        let helper = self.helper.to_string_lossy().to_string();
        let payload = serde_json::to_string(op)?;

        let (stdin, interactive) = match password {
            // `-n` fails immediately instead of blocking on a hidden prompt.
            None => (payload + "\n", false),
            // `-S` reads the password from stdin; `-k` drops any cached stamp so
            // we stay in control of when a password is used.
            Some(pw) => (format!("{pw}\n{payload}\n"), true),
        };
        let mut cmd = Cmd::new("sudo");
        if interactive {
            cmd = cmd.args(["-S", "-k", "-p", "", "--", &helper, "--stdin"]);
            prompter.status(&format!("Authenticating with sudo ({})", op.summary()));
        } else {
            cmd = cmd.args(["-n", "-p", "", "--", &helper, "--stdin"]);
        }
        let cmd = cmd.stdin_str(stdin).timeout(Duration::from_secs(600));

        let out = exec::run(&cmd)?;
        if let Some(res) = parse_helper_json(&out.stdout) {
            return Ok(res);
        }
        if out.timed_out {
            return Err(Error::new("The administrator action timed out"));
        }

        let stderr = out.combined();
        if stderr.contains("Sorry, try again")
            || stderr.contains("incorrect password attempts")
            || stderr.contains("Authentication failure")
            || stderr.contains("Invalid credentials")
        {
            return Err(Error::with_detail(AUTH_FAILED, stderr));
        }
        if !interactive
            && (stderr.contains("a password is required")
                || stderr.contains("a terminal is required")
                || stderr.contains("sudo: a password"))
        {
            return Err(Error::with_detail(NEEDS_PASSWORD, stderr));
        }
        Err(Error::with_detail(
            format!("`sudo` could not run the helper (exit {:?})", out.code),
            stderr,
        )
        .hint(&format!("Helper path: {}", self.helper.display())))
    }

    /// Cached password first, then a prompt.
    fn next_password(
        &self,
        prompter: &dyn Prompter,
        op: &Op,
        attempt: u32,
        previous_error: Option<String>,
    ) -> Option<Credentials> {
        if attempt == 1 && previous_error.is_none() {
            if let Ok(slot) = self.password.lock() {
                if let Some(pw) = slot.clone() {
                    if !pw.is_empty() {
                        return Some(Credentials::cached(pw));
                    }
                }
            }
        }
        let ctx = PromptContext {
            reason: format!("Administrator rights are needed to {}", op.summary()),
            detail: op.summary(),
            username: platform::user_name(),
            attempt,
            previous_error,
        };
        let creds = prompter.password(&ctx)?;
        if creds.password.is_empty() {
            return None;
        }
        Some(creds)
    }
}

/// Parse the JSON answer of the helper out of (possibly noisy) stdout.
pub fn parse_helper_json(stdout: &str) -> Option<OpResult> {
    // The helper prints exactly one JSON object; ignore anything else (pkexec
    // and sudo may add notices to stdout).
    if let Ok(res) = serde_json::from_str::<OpResult>(stdout.trim()) {
        return Some(res);
    }
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with('{') && line.ends_with('}') {
            if let Ok(res) = serde_json::from_str::<OpResult>(line) {
                return Some(res);
            }
        }
    }
    None
}

/// Absolute path of the privileged helper.
pub fn helper_path() -> PathBuf {
    if let Ok(p) = std::env::var(HELPER_ENV) {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let candidate = PathBuf::from(HELPER_PATH);
    if candidate.exists() {
        return candidate;
    }
    // Development build: use the binary next to the running executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let dev = dir.join("mount-manager-helper");
            if dev.exists() {
                return dev;
            }
        }
    }
    candidate
}

/// Does `sudo -n true` succeed (passwordless sudo)?
pub fn sudo_probe() -> bool {
    if !exec::exists("sudo") {
        return false;
    }
    exec::run(
        &Cmd::new("sudo")
            .args(["-n", "-p", "", "true"])
            .timeout(Duration::from_secs(6)),
    )
    .map(|o| o.ok())
    .unwrap_or(false)
}

/// Is a polkit authority reachable on the system bus?
pub fn polkit_available() -> bool {
    if !exec::exists("pkexec") {
        return false;
    }
    // Asking polkit to enumerate actions proves that polkitd is running; it
    // does not prove that an authentication *agent* exists, which is why
    // `Runner::run` still falls back to sudo on failure.
    if exec::exists("pkaction") {
        return exec::run(
            &Cmd::new("pkaction")
                .args(["--verbose", "--action-id", crate::POLKIT_ACTION])
                .timeout(Duration::from_secs(5)),
        )
        .map(|o| o.ok())
        .unwrap_or(false);
    }
    exec::run(
        &Cmd::new("dbus-send")
            .args([
                "--system",
                "--print-reply",
                "--dest=org.freedesktop.PolicyKit1",
                "/org/freedesktop/PolicyKit1/Authority",
                "org.freedesktop.DBus.Properties.GetAll",
                "string:org.freedesktop.PolicyKit1.Authority",
            ])
            .timeout(Duration::from_secs(5)),
    )
    .map(|o| o.ok())
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{MountAttempt, MountOp, ProbeOp};

    fn probe_op() -> Op {
        Op::Probe(ProbeOp {})
    }

    fn mount_op() -> Op {
        Op::Mount(Box::new(MountOp {
            target: "/mnt/test".into(),
            create_target: true,
            attempts: vec![MountAttempt::new("//h/s").fstype("cifs")],
            post: None,
            creds: None,
            persist: None,
            bookmark: None,
            chown_target: None,
            verify: true,
            caller: crate::ops::CallerSpec {
                uid: 1000,
                gid: 1000,
                name: "jo".into(),
                home: "/home/jo".into(),
            },
        }))
    }

    #[test]
    fn parses_helper_json_with_noise() {
        let json = serde_json::to_string(&OpResult::ok("done")).unwrap();
        let noisy = format!("pkexec: some notice\n{json}\n");
        assert_eq!(parse_helper_json(&noisy).unwrap().message, "done");
        assert!(parse_helper_json("no json here").is_none());
    }

    #[test]
    fn helper_path_honours_env_override() {
        std::env::set_var(HELPER_ENV, "/tmp/fake-helper");
        assert_eq!(helper_path(), PathBuf::from("/tmp/fake-helper"));
        std::env::remove_var(HELPER_ENV);
        assert_eq!(helper_path(), PathBuf::from(HELPER_PATH));
    }

    #[test]
    fn method_labels_and_prompts() {
        assert!(Method::Pkexec.asks_for_password());
        assert!(!Method::SudoCached.asks_for_password());
        assert_eq!(Method::Root.label(), "Running as root");
    }

    #[test]
    fn never_pref_disables_escalation() {
        let runner = Runner::new(EscalationPref::Never);
        if !platform::is_root() {
            assert_eq!(runner.method(), Method::None);
            let err = runner.run(&probe_op(), &NonInteractive).unwrap_err();
            assert!(err.message.contains("Administrator rights"), "{err}");
        }
    }

    #[test]
    fn summaries_are_readable() {
        assert_eq!(probe_op().summary(), "probe administrator access");
        assert_eq!(mount_op().summary(), "mount //h/s on /mnt/test");
    }
}

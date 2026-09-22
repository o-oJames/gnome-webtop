//! Subprocess helpers: capture stdout/stderr, feed stdin, enforce timeouts.
//!
//! Everything that shells out (`mount`, `umount`, `lsblk`, `smbclient`,
//! `pkexec`, `sudo`, ...) goes through [`run`] so behaviour (timeouts, captured
//! output, no shell interpolation) is consistent and testable.

use crate::error::{Error, Result};
use std::io::Write;
use std::process::{Command as StdCommand, Stdio};
use std::time::Duration;

/// Captured result of a subprocess.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    /// stdout + stderr, trimmed; handy for error dialogs.
    pub fn combined(&self) -> String {
        let mut s = String::new();
        let out = self.stdout.trim();
        let err = self.stderr.trim();
        if !out.is_empty() {
            s.push_str(out);
        }
        if !err.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(err);
        }
        s
    }

    /// Trimmed non-empty stdout lines.
    pub fn stdout_lines(&self) -> Vec<String> {
        self.stdout
            .lines()
            .map(|l| l.trim_end().to_string())
            .filter(|l| !l.trim().is_empty())
            .collect()
    }
}

/// Description of a command to run.
#[derive(Debug, Clone, Default)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    /// Written to the child's stdin (used to hand a password to `sudo -S` and
    /// JSON to the helper) — never appears in `ps` output.
    pub stdin: Option<Vec<u8>>,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    pub dir: Option<String>,
}

impl Cmd {
    pub fn new<P: Into<String>>(program: P) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            stdin: None,
            env: Vec::new(),
            timeout: Duration::from_secs(60),
            dir: None,
        }
    }

    pub fn arg<A: Into<String>>(mut self, a: A) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I, S>(mut self, iter: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for a in iter {
            self.args.push(a.into());
        }
        self
    }

    pub fn stdin_bytes(mut self, data: Vec<u8>) -> Self {
        self.stdin = Some(data);
        self
    }

    pub fn stdin_str<S: Into<String>>(self, data: S) -> Self {
        self.stdin_bytes(data.into().into_bytes())
    }

    pub fn env<K: Into<String>, V: Into<String>>(mut self, k: K, v: V) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    pub fn dir<D: Into<String>>(mut self, d: D) -> Self {
        self.dir = Some(d.into());
        self
    }

    /// `mount -t cifs -o ... //h/s /mnt/x` style rendering, for logs.
    pub fn render(&self) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.args.iter().cloned());
        parts
            .iter()
            .map(|p| {
                if p.is_empty() || p.contains(' ') || p.contains('"') {
                    format!("'{}'", p.replace('\'', "'\\''"))
                } else {
                    p.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Run a command, capturing stdout/stderr and killing it after `cmd.timeout`.
pub fn run(cmd: &Cmd) -> Result<Output> {
    let mut child = {
        let mut c = StdCommand::new(&cmd.program);
        c.args(&cmd.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &cmd.env {
            c.env(k, v);
        }
        if let Some(d) = &cmd.dir {
            c.current_dir(d);
        }
        c.spawn().map_err(|e| {
            Error::with_detail(format!("Failed to start `{}`", cmd.program), e.to_string())
        })?
    };

    // Feed stdin from a helper thread: the child may block writing to a full
    // pipe while we are still writing, so nothing may be sequential here.
    let data = cmd.stdin.clone();
    let stdin_pipe = child.stdin.take();
    let writer = match (data, stdin_pipe) {
        (Some(data), Some(mut pipe)) => Some(std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
            let _ = pipe.flush();
            // Closing the pipe here signals EOF to the child.
        })),
        _ => None,
    };

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let out_thread = stdout_pipe.map(|p| std::thread::spawn(move || read_all(p)));
    let err_thread = stderr_pipe.map(|p| std::thread::spawn(move || read_all(p)));

    // Wait with a timeout so a hung network mount can never freeze the caller.
    // `child` stays in this thread so it can be killed when the deadline hits.
    let deadline = std::time::Instant::now() + cmd.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(Output {
                        code: None,
                        stdout: String::new(),
                        stderr: format!("timed out after {}s", cmd.timeout.as_secs()),
                        timed_out: true,
                    });
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(_) => break None,
        }
    };

    if let Some(w) = writer {
        let _ = w.join();
    }
    let stdout = out_thread.and_then(|t| t.join().ok()).unwrap_or_default();
    let stderr = err_thread.and_then(|t| t.join().ok()).unwrap_or_default();

    Ok(Output {
        code: status,
        stdout,
        stderr,
        timed_out: false,
    })
}

/// Run a command and require exit status 0.
pub fn run_ok(cmd: &Cmd) -> Result<Output> {
    let out = run(cmd)?;
    if out.ok() {
        Ok(out)
    } else {
        Err(Error::with_detail(
            format!("`{}` failed (exit {:?})", cmd.render(), out.code),
            out.combined(),
        ))
    }
}

/// `true` when the program can be found in `$PATH`.
pub fn exists(program: &str) -> bool {
    which(program).is_some()
}

/// `true` when the program exists in `$PATH` or in one of the sbin directories
/// (GUI sessions often have a `$PATH` without `/sbin`).
pub fn exists_any(program: &str) -> bool {
    if which(program).is_some() {
        return true;
    }
    for dir in ["/sbin", "/usr/sbin", "/usr/local/sbin", "/usr/libexec"] {
        if std::path::Path::new(&format!("{dir}/{program}")).is_file() {
            return true;
        }
    }
    false
}

/// Locate a program in `$PATH`.
pub fn which(program: &str) -> Option<String> {
    if program.contains('/') {
        return if std::path::Path::new(program).is_file() {
            Some(program.to_string())
        } else {
            None
        };
    }
    for dir in std::env::var_os("PATH")?.to_string_lossy().split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = format!("{}/{}", dir.trim_end_matches('/'), program);
        if let Ok(meta) = std::fs::metadata(&candidate) {
            if meta.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn read_all<R: std::io::Read>(mut r: R) -> String {
    let mut buf = Vec::new();
    if r.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_quotes() {
        let c = Cmd::new("mount")
            .arg("-t")
            .arg("cifs")
            .arg("//h/my share")
            .arg("/mnt/x");
        assert_eq!(c.render(), "mount -t cifs '//h/my share' /mnt/x");
    }

    #[test]
    fn echoes_stdin() {
        let out = run(&Cmd::new("cat")
            .stdin_str("hello\n")
            .timeout(Duration::from_secs(5)))
        .unwrap();
        assert!(out.ok());
        assert_eq!(out.stdout, "hello\n");
    }

    #[test]
    fn timeout_kills() {
        let out = run(&Cmd::new("sleep")
            .arg("30")
            .timeout(Duration::from_millis(300)))
        .unwrap();
        assert!(out.timed_out);
        assert!(!out.ok());
    }

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }
}

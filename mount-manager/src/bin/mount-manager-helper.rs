//! Privileged helper for Mount Manager.
//!
//! It is started by `pkexec` (polkit action `io.github.mount_manager.manage`)
//! or by `sudo`, reads exactly one JSON [`Op`] from **stdin**, executes it as
//! root through [`mount_manager::executor`] and prints one JSON
//! [`OpResult`] on stdout.
//!
//! Nothing is ever taken from argv, so secrets cannot be seen in `ps`, and
//! stdout carries only the JSON answer — diagnostics go to stderr.

use mount_manager::executor;
use mount_manager::ops::{Op, OpResult};
use std::io::{Read, Write};

const MAX_PAYLOAD: usize = 1024 * 1024;

fn main() {
    // Never panic on a closed pipe: the caller may have given up on us.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("mount-manager-helper {}", mount_manager::VERSION);
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("{}", USAGE);
        std::process::exit(if args.is_empty() { 2 } else { 0 });
    }
    if !args.iter().any(|a| a == "--stdin") {
        eprintln!("{}", USAGE);
        // Exit 2 = "used wrong", distinct from 1 = "operation failed".
        answer(OpResult::fail(
            "mount-manager-helper must be called with --stdin and a JSON operation on stdin",
        ));
        std::process::exit(2);
    }

    // Read the operation. A size limit keeps a confused caller from making us
    // allocate without bound.
    let mut payload = String::new();
    if let Err(e) = std::io::stdin()
        .take(MAX_PAYLOAD as u64)
        .read_to_string(&mut payload)
    {
        answer(OpResult::fail(format!(
            "Cannot read the operation from stdin: {e}"
        )));
        std::process::exit(1);
    }
    let payload = payload.trim();
    if payload.is_empty() {
        answer(OpResult::fail("No operation was provided on stdin"));
        std::process::exit(1);
    }

    let op: Op = match serde_json::from_str(payload) {
        Ok(op) => op,
        Err(e) => {
            answer(OpResult::fail(format!(
                "The operation could not be parsed: {e}"
            )));
            std::process::exit(1);
        }
    };

    // Trace what we were asked to do (stderr only, stdout stays clean JSON).
    eprintln!("mount-manager-helper: {}", op.summary());

    let result = executor::execute(&op);
    let code = if result.ok { 0 } else { 1 };
    answer(result);
    std::process::exit(code);
}

/// Print the JSON answer as a single line on stdout.
fn answer(result: OpResult) {
    let mut stdout = std::io::stdout().lock();
    match serde_json::to_string(&result) {
        Ok(json) => {
            let _ = writeln!(stdout, "{json}");
        }
        Err(e) => {
            let _ = writeln!(
                stdout,
                "{{\"ok\":false,\"message\":\"cannot encode result: {e}\",\"log\":[]}}"
            );
        }
    }
    let _ = stdout.flush();
}

const USAGE: &str = "mount-manager-helper — privileged helper for Mount Manager

Usage:
  mount-manager-helper --stdin        read a JSON operation from stdin
  mount-manager-helper --version      print the version
  mount-manager-helper --help         this text

This program is not meant to be run by hand: it is started through pkexec or
sudo by the Mount Manager application, and it refuses to do anything unless it
is running as root.";

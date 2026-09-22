//! Mount Manager entry point: GUI by default, CLI when arguments are given.

use mount_manager::cli;

fn main() {
    // Behave like a normal Unix tool: `mount-manager list | head` must not
    // panic when the reader closes the pipe (Rust turns EPIPE into a panic).
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();

    #[cfg(feature = "gui")]
    {
        // No arguments, `--gui`, or a URI handed to us by the desktop
        // (`Exec=mount-manager %U`) all open the window.
        if args.is_empty() || wants_gui(&args) {
            mount_manager::ui::run(args);
            return;
        }
    }

    #[cfg(not(feature = "gui"))]
    if args.is_empty() || wants_gui(&args) {
        eprintln!("This build of mount-manager has no graphical interface.");
        eprintln!("{}", cli::usage());
        std::process::exit(2);
    }

    std::process::exit(cli::run(&args));
}

/// `true` when the arguments mean "open the window" rather than "run a command".
///
/// Only a *leading* URI counts (`mount-manager smb://nas/data`, as launched by
/// `Exec=mount-manager %U`); `mount-manager mount smb://nas/data` is a command.
#[cfg(feature = "gui")]
fn wants_gui(args: &[String]) -> bool {
    if args.iter().any(|a| a == "--gui") {
        return true;
    }
    match args.first() {
        Some(first) => is_share_uri(first),
        None => false,
    }
}

#[cfg(feature = "gui")]
fn is_share_uri(arg: &str) -> bool {
    const SCHEMES: &[&str] = &[
        "smb://", "cifs://", "nfs://", "sftp://", "ssh://", "dav://", "davs://", "ftp://",
    ];
    SCHEMES.iter().any(|scheme| arg.starts_with(scheme))
}

#[cfg(not(feature = "gui"))]
fn wants_gui(args: &[String]) -> bool {
    args.iter().any(|a| a == "--gui")
}

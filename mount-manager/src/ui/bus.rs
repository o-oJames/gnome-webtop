//! Thread bridge between worker threads (which do the blocking work) and the
//! GTK main loop.
//!
//! GTK objects are not `Send`, so worker threads never touch widgets: they post
//! [`Msg`] values through [`Bus`] and the main loop drains the queue with a
//! `glib` timeout. Blocking questions (the administrator password) use a reply
//! channel, which parks the *worker* thread, never the UI.

use crate::config::AppConfig;
use crate::error::{Error, Result};
use crate::model::{BlockDevice, LogEntry, MountEntry, Protocol, SavedShare, ShareRequest};
use crate::privilege::PromptContext;
use gtk::glib;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One-way message to the main loop.
pub enum Msg {
    /// Refresh finished: mounts (+ the shares that are currently mounted).
    Mounts(Result<Vec<MountEntry>>),
    Devices(Result<Vec<BlockDevice>>),
    Shares(Vec<(SavedShare, Option<MountEntry>)>),
    Logs(Vec<LogEntry>),
    Config(Box<AppConfig>),
    /// A user action finished (mount/unmount/save/delete/...).
    Done {
        action: String,
        result: Result<String>,
    },
    /// Result of "Test connection".
    Test(Result<String>),
    /// Result of network discovery inside the share dialog.
    Discovered {
        protocol: Protocol,
        services: Vec<crate::discover::Service>,
        shares: Option<Result<Vec<crate::discover::SmbShare>>>,
        exports: Option<Result<Vec<crate::discover::NfsExport>>>,
    },
    /// The engine needs the administrator password.
    AskPassword {
        context: PromptContext,
        reply: Sender<Option<crate::privilege::Credentials>>,
    },
    /// Show a transient toast.
    Toast(String),
    /// An unmount failed because the target is busy: offer a lazy unmount.
    LazyOffer {
        target: String,
        detail: String,
    },
    /// Show a blocking error dialog.
    Error {
        title: String,
        error: Error,
    },
    /// Busy spinner on/off.
    Busy(bool),
    /// Ask the main window to open the share dialog (optionally prefilled).
    OpenShareDialog(Option<Box<ShareRequest>>),
    /// A row asked for a specific mount point to be opened in Files.
    Open(String),
    /// Nothing to do, just wake the poller up.
    Ping,
}

/// Cloneable handle used by worker threads.
#[derive(Clone)]
pub struct Bus {
    tx: Arc<Mutex<Sender<Msg>>>,
}

impl Bus {
    /// A bus and the matching receiver (the receiver stays on the main thread).
    pub fn channel() -> (Self, Receiver<Msg>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                tx: Arc::new(Mutex::new(tx)),
            },
            rx,
        )
    }

    /// Send a message; silently ignored after the window is gone.
    pub fn send(&self, msg: Msg) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(msg);
        }
    }

    pub fn toast<S: Into<String>>(&self, text: S) {
        self.send(Msg::Toast(text.into()));
    }

    pub fn done<S: Into<String>>(&self, action: S, result: crate::error::Result<String>) {
        self.send(Msg::Done {
            action: action.into(),
            result,
        });
    }

    pub fn error<T: Into<String>>(&self, title: T, error: Error) {
        self.send(Msg::Error {
            title: title.into(),
            error,
        });
    }
}

/// Report an unmount result, offering a lazy unmount when the target is busy.
pub fn report_unmount(bus: &Bus, target: &str, result: crate::error::Result<String>) {
    if let Err(e) = &result {
        if e.message.contains("busy") {
            bus.send(Msg::LazyOffer {
                target: target.to_string(),
                detail: e.detail.clone().unwrap_or_default(),
            });
        }
    }
    bus.done("Unmount", result);
}

/// Drain everything that arrived, waiting at most `wait` for the first message.
pub fn drain(rx: &Receiver<Msg>, wait: Duration) -> Vec<Msg> {
    let mut out = Vec::new();
    match rx.recv_timeout(wait) {
        Ok(msg) => out.push(msg),
        Err(RecvTimeoutError::Timeout) => return out,
        Err(RecvTimeoutError::Disconnected) => return out,
    }
    while let Ok(msg) = rx.try_recv() {
        out.push(msg);
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Local channels: the same pattern for dialogs that own their workers
// ─────────────────────────────────────────────────────────────────────────────

/// A dialog-private channel. Worker threads get a cheap clone of the sender;
/// the main loop drains it with [`attach`].
pub struct LocalChannel<T> {
    tx: Arc<Mutex<Sender<T>>>,
}

// Manual impl: `#[derive(Clone)]` would add a needless `T: Clone` bound.
impl<T> Clone for LocalChannel<T> {
    fn clone(&self) -> Self {
        Self {
            tx: Arc::clone(&self.tx),
        }
    }
}

impl<T: Send + 'static> LocalChannel<T> {
    pub fn new() -> (Self, Receiver<T>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                tx: Arc::new(Mutex::new(tx)),
            },
            rx,
        )
    }

    pub fn send(&self, value: T) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(value);
        }
    }
}

/// Run `f` on a worker thread and post its result to `channel`.
pub fn spawn<T, F, R>(channel: &LocalChannel<T>, f: F)
where
    T: Send + 'static,
    R: Into<T>,
    F: FnOnce() -> R + Send + 'static,
{
    let channel = channel.clone();
    std::thread::spawn(move || {
        let value = f();
        channel.send(value.into());
    });
}

/// Poll a receiver on the GTK main loop until it is disconnected.
///
/// Returns the [`glib::SourceId`] so the caller can remove the source early
/// (dialogs do that when they are closed).
pub fn attach<T, F>(rx: Receiver<T>, handler: F) -> glib::SourceId
where
    T: 'static,
    F: FnMut(T) + 'static,
{
    let rx = std::rc::Rc::new(std::cell::RefCell::new((rx, handler)));
    glib::timeout_add_local(std::time::Duration::from_millis(90), move || {
        let Ok(mut borrow) = rx.try_borrow_mut() else {
            return glib::ControlFlow::Continue;
        };
        let (rx, handler) = &mut *borrow;
        let mut alive = true;
        match rx.recv_timeout(Duration::from_millis(0)) {
            Ok(msg) => handler(msg),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => alive = false,
        }
        while let Ok(msg) = rx.try_recv() {
            handler(msg);
        }
        if alive {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    })
}

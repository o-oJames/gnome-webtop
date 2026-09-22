//! Bridges [`crate::privilege::Prompter`] to the GTK password dialog.

use crate::privilege::{Credentials, PromptContext, Prompter};
use crate::ui::bus::{Bus, Msg};
use std::sync::mpsc;

/// Asks for passwords through the application window.
///
/// `password()` blocks the calling (worker) thread until the dialog answers,
/// which is exactly what the escalation code expects.
pub struct UiPrompter {
    bus: Bus,
}

impl UiPrompter {
    pub fn new(bus: Bus) -> Self {
        Self { bus }
    }
}

impl Prompter for UiPrompter {
    fn password(&self, ctx: &PromptContext) -> Option<Credentials> {
        let (reply, rx) = mpsc::channel();
        self.bus.send(Msg::AskPassword {
            context: ctx.clone(),
            reply,
        });
        // No timeout on purpose: the dialog stays open until the user answers.
        rx.recv().ok().flatten()
    }

    fn status(&self, message: &str) {
        self.bus.toast(message.to_string());
    }
}

/// Prompter that never answers — used before the window exists.
pub struct Silent;

impl Prompter for Silent {
    fn password(&self, _ctx: &PromptContext) -> Option<Credentials> {
        None
    }
}

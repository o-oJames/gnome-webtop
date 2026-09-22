//! Window chrome plus the plumbing shared by every page.

use crate::engine::Engine;
use crate::error::Error;
use crate::privilege::Prompter;
use crate::ui::bus::Bus;
use crate::ui::prompter::UiPrompter;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// CSS embedded in the binary so the app works no matter where it is installed.
const STYLE_CSS: &str = include_str!("style.css");

/// Load the application stylesheet for every window on this display.
pub fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(STYLE_CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Everything a page needs: window chrome, engine and message bus.
///
/// Cheap to clone (all members are refcounted) and safe to capture in GTK
/// callbacks. **Not `Send`**: to move work off the UI thread, clone
/// `shell.engine` and `shell.bus` (both `Send + Sync`) into the worker and post
/// the result back over the bus.
#[derive(Clone)]
pub struct Shell {
    pub window: adw::ApplicationWindow,
    pub toast: adw::ToastOverlay,
    pub banner: adw::Banner,
    pub spinner: gtk::Spinner,
    pub engine: Arc<Engine>,
    pub bus: Bus,
    /// Detail text belonging to the banner (shared by every `Shell` clone).
    banner_detail: Rc<RefCell<Option<String>>>,
}

impl Shell {
    /// Build the application window skeleton and the engine behind it.
    pub fn new(app: &adw::Application, bus: Bus, engine: Arc<Engine>) -> Self {
        let spinner = gtk::Spinner::builder()
            .spinning(false)
            .valign(gtk::Align::Center)
            .build();

        let banner = adw::Banner::builder()
            .revealed(false)
            .button_label("Details")
            .build();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        content.append(&banner);

        let toast = adw::ToastOverlay::new();
        toast.set_child(Some(&content));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(crate::APP_NAME)
            .default_width(980)
            .default_height(700)
            .width_request(380)
            .height_request(420)
            .content(&toast)
            .icon_name("mount-manager")
            .build();

        Self {
            window,
            toast,
            banner,
            spinner,
            engine,
            bus,
            banner_detail: Rc::new(RefCell::new(None)),
        }
    }

    /// Content box of the window (pages append into it).
    pub fn content(&self) -> gtk::Box {
        self.toast
            .child()
            .and_then(|c| c.downcast::<gtk::Box>().ok())
            .expect("window content is a Box")
    }

    pub fn toast(&self, message: &str) {
        let toast = adw::Toast::builder().title(message).timeout(3).build();
        self.toast.add_toast(toast);
    }

    pub fn busy(&self, busy: bool) {
        self.spinner.set_spinning(busy);
        self.spinner.set_visible(busy);
    }

    /// Show (or hide) the warning strip below the header bar.
    pub fn set_banner(&self, title: &str, detail: Option<String>) {
        *self.banner_detail.borrow_mut() = detail.clone().filter(|d| !d.trim().is_empty());
        if title.is_empty() {
            self.banner.set_revealed(false);
            self.banner.set_title("");
            return;
        }
        self.banner.set_title(title);
        self.banner
            .set_button_label(Some(if self.banner_detail.borrow().is_some() {
                "Details"
            } else {
                "Dismiss"
            }));
        self.banner.set_revealed(true);
    }

    /// Technical detail attached to the currently shown banner.
    pub fn banner_detail(&self) -> Option<String> {
        self.banner_detail.borrow().clone()
    }

    /// Modal error dialog with the technical detail attached.
    pub fn show_error(&self, title: &str, error: &Error) {
        crate::ui::alert::error(self, title, error);
    }

    /// Ask a yes/no question through a modal dialog.
    pub fn confirm<F>(&self, heading: &str, body: &str, confirm_label: &str, on_yes: F)
    where
        F: FnOnce() + 'static,
    {
        crate::ui::alert::confirm(self, heading, body, confirm_label, on_yes);
    }
}

/// Build the engine used by the whole UI (password prompts go through the bus).
pub fn engine(bus: &Bus) -> Arc<Engine> {
    let prompter: Arc<dyn Prompter> = Arc::new(UiPrompter::new(bus.clone()));
    Engine::new(prompter)
}

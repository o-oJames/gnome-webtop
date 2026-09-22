//! GTK4 / libadwaita front-end.
//!
//! Layout: [`shell`] owns the application window chrome and the thread bridge,
//! the four pages ([`mounts_page`], [`devices_page`], [`shares_page`],
//! [`activity_page`]) render lists, and the dialogs ([`share_dialog`],
//! [`password_dialog`], [`browse_dialog`], [`preferences_dialog`]) do the rest.
//! All blocking work happens on worker threads (see [`shell::Shell::spawn`]).

mod activity_page;
mod alert;
mod browse_dialog;
mod devices_page;
mod mounts_page;
mod password_dialog;
mod preferences_dialog;
mod share_dialog;
mod shares_page;
mod shell;
mod widgets;
mod window;

pub mod bus;
pub mod prompter;

use crate::cli;
use crate::model::ShareRequest;
use gtk::gio::prelude::{ApplicationExt, ApplicationExtManual};
use std::cell::RefCell;
use std::rc::Rc;

/// Start the graphical application.
pub fn run(args: Vec<String>) {
    // A URI handed to us by the desktop (`Exec=mount-manager %U`) pre-fills the
    // "add share" dialog.
    let prefilled: Rc<RefCell<Vec<ShareRequest>>> = Rc::new(RefCell::new(
        args.iter()
            .filter(|a| a.contains("://"))
            .filter_map(|a| cli::parse_uri(a).ok())
            .collect(),
    ));

    let app = adw::Application::builder()
        .application_id(crate::APP_ID)
        .build();

    app.connect_startup(|_app| {
        shell::install_css();
    });

    app.connect_activate(move |app| {
        let requests = prefilled.borrow_mut().drain(..).collect::<Vec<_>>();
        window::build(app, requests);
    });

    // GApplication would try to parse our CLI flags itself, so hand it a clean
    // argv; the CLI is handled in main.rs before we get here.
    app.run_with_args(&["mount-manager"]);
}

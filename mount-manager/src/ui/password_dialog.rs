//! The administrator password dialog.
//!
//! This is the "ask the user for the password when root rights are needed" part
//! of the app. It is shown when neither polkit nor passwordless sudo can be
//! used (typical inside containers). The answer travels back to the *blocked
//! worker thread* through a one-shot channel, so the UI never waits.

use crate::privilege::{Credentials, PromptContext};
use crate::ui::shell::Shell;
use adw::prelude::*;
use gtk::glib;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

/// Show the dialog; `reply` receives the password or `None` when cancelled.
pub fn ask(shell: &Shell, ctx: PromptContext, reply: Sender<Option<Credentials>>) {
    let mut builder = adw::Window::builder()
        .title("Authentication required")
        .modal(true)
        .default_width(460)
        .resizable(false)
        .icon_name("dialog-password-symbolic");
    if let Some(app) = shell.window.application() {
        builder = builder.application(&app);
    }
    let window = builder.build();

    let header = adw::HeaderBar::builder().build();
    let cancel = gtk::Button::builder().label("Cancel").build();
    let unlock = gtk::Button::builder()
        .label("Unlock")
        .css_classes(vec!["suggested-action".to_string()])
        .build();
    header.pack_start(&cancel);
    header.pack_end(&unlock);

    let title = gtk::Label::builder()
        .label(&ctx.reason)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["heading".to_string()])
        .build();
    let body = gtk::Label::builder()
        .label(body_text(&ctx))
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
        .build();
    let error_label = gtk::Label::builder()
        .label(ctx.previous_error.clone().unwrap_or_default())
        .wrap(true)
        .xalign(0.0)
        .visible(ctx.previous_error.is_some())
        .css_classes(vec!["error-text".to_string(), "caption".to_string()])
        .build();

    let user_row = adw::EntryRow::builder()
        .title("User name")
        .text(&ctx.username)
        .editable(false)
        .build();
    let password_row = adw::PasswordEntryRow::builder()
        .title("Password")
        .activates_default(true)
        .build();
    let remember_row = adw::SwitchRow::builder()
        .title("Remember for this session")
        .subtitle("Kept in memory until Mount Manager quits — never written to disk")
        .active(true)
        .build();
    let group = adw::PreferencesGroup::builder().build();
    group.add(&user_row);
    group.add(&password_row);
    group.add(&remember_row);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_start(6)
        .margin_end(6)
        .margin_bottom(6)
        .build();
    content.append(&title);
    content.append(&body);
    content.append(&error_label);
    content.append(&group);

    // AdwPreferencesPage only accepts groups, so wrap the box in one.
    let wrapper = adw::PreferencesGroup::new();
    wrapper.add(&content);
    let page = adw::PreferencesPage::builder().build();
    page.add(&wrapper);

    let toolbar = adw::ToolbarView::builder().build();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&page));
    window.set_content(Some(&toolbar));

    // ── answer plumbing: exactly one reply, whichever way we close ────────
    let answered = Rc::new(Cell::new(false));
    let reply = Rc::new(Mutex::new(Some(reply)));
    let finish: Rc<dyn Fn(Option<Credentials>)> = Rc::new({
        let (answered, reply, window) = (Rc::clone(&answered), Rc::clone(&reply), window.clone());
        move |creds: Option<Credentials>| {
            if answered.replace(true) {
                return;
            }
            if let Ok(mut slot) = reply.lock() {
                if let Some(reply) = slot.take() {
                    let _ = reply.send(creds);
                }
            }
            window.close();
        }
    });

    unlock.connect_clicked(glib::clone!(
        #[strong]
        password_row,
        #[strong]
        remember_row,
        #[strong]
        finish,
        move |_| {
            let password = password_row.text().to_string();
            if password.is_empty() {
                password_row.add_css_class("error");
                password_row.grab_focus();
                return;
            }
            password_row.remove_css_class("error");
            finish(Some(Credentials {
                password,
                remember: remember_row.is_active(),
            }));
        },
    ));

    password_row.connect_activate(glib::clone!(
        #[strong]
        password_row,
        #[strong]
        remember_row,
        #[strong]
        finish,
        move |_| {
            let password = password_row.text().to_string();
            if password.is_empty() {
                password_row.add_css_class("error");
                return;
            }
            finish(Some(Credentials {
                password,
                remember: remember_row.is_active(),
            }));
        },
    ));

    cancel.connect_clicked(glib::clone!(
        #[strong]
        finish,
        move |_| finish(None),
    ));
    window.connect_close_request(glib::clone!(
        #[strong]
        finish,
        move |_| {
            finish(None);
            glib::Propagation::Proceed
        },
    ));

    window.set_transient_for(Some(&shell.window));
    window.present();
    password_row.grab_focus();
}

fn body_text(ctx: &PromptContext) -> String {
    let mut text = String::from(
        "Mount Manager needs administrator rights for this action.\n\
         The password is piped straight into sudo — it is never stored and never \
         appears on a command line.",
    );
    if !ctx.detail.is_empty() && ctx.detail != ctx.reason {
        text.push_str(&format!("\n\nAction: {}", ctx.detail));
    }
    if ctx.attempt > 1 {
        text.push_str(&format!("\n\nAttempt {} of 3.", ctx.attempt));
    }
    text
}

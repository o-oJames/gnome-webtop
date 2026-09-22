//! Modal message dialogs.
//!
//! Built from plain libadwaita widgets instead of `AdwAlertDialog` so the app
//! only needs libadwaita ≥ 1.4 (Ubuntu 24.04), and so the "technical details"
//! block can be styled consistently with the rest of the UI.

use crate::error::Error;
use crate::ui::shell::Shell;
use crate::ui::widgets;
use adw::prelude::*;
use gtk::glib;

/// Informational dialog with an optional technical detail block.
pub fn info(shell: &Shell, heading: &str, body: &str, detail: Option<&str>) -> adw::Window {
    build(shell, heading, body, detail, None, None)
}

/// Error dialog; the detail (usually captured stderr) is shown monospaced.
pub fn error(shell: &Shell, title: &str, error: &Error) {
    let window = build(
        shell,
        title,
        &error.message,
        error.detail.as_deref().filter(|d| !d.trim().is_empty()),
        None,
        None,
    );
    window.add_css_class("error-dialog");
}

/// Yes/no dialog. `on_confirm` runs on the UI thread when accepted.
pub fn confirm<F>(shell: &Shell, heading: &str, body: &str, confirm_label: &str, on_confirm: F)
where
    F: FnOnce() + 'static,
{
    build(
        shell,
        heading,
        body,
        None,
        Some(confirm_label.to_string()),
        Some(Box::new(on_confirm)),
    );
}

fn build(
    shell: &Shell,
    heading: &str,
    body: &str,
    detail: Option<&str>,
    confirm_label: Option<String>,
    on_confirm: Option<Box<dyn FnOnce()>>,
) -> adw::Window {
    let mut builder = adw::Window::builder()
        .title(heading)
        .modal(true)
        .default_width(520)
        .resizable(false)
        .icon_name(if confirm_label.is_some() {
            "dialog-question-symbolic"
        } else {
            "dialog-warning-symbolic"
        });
    if let Some(app) = shell.window.application() {
        builder = builder.application(&app);
    }
    let window = builder.build();

    let header = adw::HeaderBar::builder()
        .css_classes(vec!["flat".to_string()])
        .build();
    let cancel = gtk::Button::builder().label("Close").build();
    header.pack_start(&cancel);

    let heading_label = gtk::Label::builder()
        .label(heading)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["title-3".to_string()])
        .build();
    let body_label = gtk::Label::builder()
        .label(body)
        .wrap(true)
        .selectable(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label".to_string()])
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_start(18)
        .margin_end(18)
        .margin_top(6)
        .margin_bottom(18)
        .build();
    content.append(&heading_label);
    content.append(&body_label);
    if let Some(detail) = detail {
        let scroller = gtk::ScrolledWindow::builder()
            .child(&widgets::detail_view(detail))
            .height_request(120)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        content.append(&scroller);
        window.set_default_size(620, 460);
        window.set_resizable(true);
    }

    if let Some(label) = confirm_label {
        let confirm = gtk::Button::builder()
            .label(label)
            .css_classes(vec!["suggested-action".to_string(), "pill".to_string()])
            .build();
        header.pack_end(&confirm);
        // `FnOnce` cannot live in an `Fn` signal handler directly, so it is
        // parked in an Rc<RefCell<..>> and taken out on the first (only) click.
        let on_confirm = std::rc::Rc::new(std::cell::RefCell::new(on_confirm));
        confirm.connect_clicked(glib::clone!(
            #[strong]
            window,
            #[strong]
            on_confirm,
            move |_| {
                window.close();
                if let Some(callback) = on_confirm.borrow_mut().take() {
                    callback();
                }
            },
        ));
        window.set_default_widget(Some(&confirm));
    }

    cancel.connect_clicked(glib::clone!(
        #[strong]
        window,
        move |_| window.close(),
    ));

    let toolbar = adw::ToolbarView::builder().build();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));

    window.set_transient_for(Some(&shell.window));
    window.present();
    window
}

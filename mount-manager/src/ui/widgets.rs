//! Small reusable widgets and helpers so the pages stay readable.

use adw::prelude::*;
use gtk::pango::EllipsizeMode;
use gtk::{Orientation, PolicyType};

/// A rounded chip used for badges (`SMB`, `fstab`, `read-only`, ...).
pub fn badge(text: &str, css_class: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .css_classes(vec![css_class.to_string(), "mount-badge".to_string()])
        .valign(gtk::Align::Center)
        .build();
    label
}

/// Title label of a list row (bold, ellipsised).
pub fn row_title(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .ellipsize(EllipsizeMode::End)
        .hexpand(true)
        .css_classes(vec!["heading".to_string()])
        .build()
}

/// Dimmed secondary label of a list row.
pub fn row_subtitle(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .ellipsize(EllipsizeMode::End)
        .hexpand(true)
        .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
        .build()
}

/// Monospace, selectable block used for command output and error details.
pub fn detail_view(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .selectable(true)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec![
            "monospace".to_string(),
            "dim-label".to_string(),
            "detail-view".to_string(),
        ])
        .build()
}

/// The friendly "nothing here yet" placeholder.
pub fn empty_state(icon: &str, title: &str, body: &str) -> gtk::Box {
    let box_ = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_top(48)
        .margin_bottom(48)
        .margin_start(24)
        .margin_end(24)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .build();
    box_.append(
        &gtk::Image::builder()
            .icon_name(icon)
            .pixel_size(72)
            .css_classes(vec!["empty-state-icon".to_string()])
            .build(),
    );
    box_.append(
        &gtk::Label::builder()
            .label(title)
            .css_classes(vec!["title-2".to_string()])
            .build(),
    );
    box_.append(
        &gtk::Label::builder()
            .label(body)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(vec!["dim-label".to_string()])
            .build(),
    );
    box_
}

/// A list that never shows a selection highlight (rows carry their own buttons).
pub fn list_box() -> gtk::ListBox {
    gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list".to_string()])
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build()
}

/// Scrolled container for a page body.
pub fn scroller(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(child)
        .hscrollbar_policy(PolicyType::Never)
        .vexpand(true)
        .build()
}

/// Remove every row of a [`gtk::ListBox`].
pub fn clear_list(container: &gtk::ListBox) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// A flat icon button with a tooltip.
pub fn icon_button(icon: &str, tooltip: &str, css: &[&str]) -> gtk::Button {
    let mut classes: Vec<String> = css.iter().map(|c| c.to_string()).collect();
    classes.push("flat".to_string());
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .css_classes(classes)
        .valign(gtk::Align::Center)
        .build()
}

/// A labelled button used inside rows.
pub fn text_button(label: &str, tooltip: &str, css: &[&str]) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .tooltip_text(tooltip)
        .css_classes(css.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .valign(gtk::Align::Center)
        .build()
}

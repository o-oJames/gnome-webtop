//! "Activity" page: everything the app did, with the technical detail attached.

use crate::model::{LogEntry, LogLevel};
use crate::ui::shell::Shell;
use crate::ui::widgets;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub struct ActivityPage {
    #[allow(dead_code)]
    shell: Shell,
    root: gtk::Box,
    stack: gtk::Stack,
    list: gtk::ListBox,
    summary: gtk::Label,
    /// Plain text rendering of the log, for "Copy log".
    log_text: Rc<RefCell<String>>,
}

impl ActivityPage {
    pub fn new(shell: Shell) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();

        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(6)
            .margin_start(18)
            .margin_end(18)
            .build();
        let summary = gtk::Label::builder()
            .label("Activity log")
            .xalign(0.0)
            .hexpand(true)
            .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
            .build();
        let copy = widgets::text_button(
            "Copy log",
            "Copy the whole log to the clipboard",
            &["flat", "pill"],
        );
        bar.append(&summary);
        bar.append(&copy);
        root.append(&bar);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(vec!["boxed-list".to_string()])
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        let empty = widgets::empty_state(
            "utilities-system-monitor-symbolic",
            "Nothing happened yet",
            "Mounts, unmounts, connection tests and errors are recorded here — including the exact commands that were run.",
        );
        let stack = gtk::Stack::builder().vexpand(true).build();
        stack.add_named(&widgets::scroller(&list), Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.set_visible_child_name("empty");
        root.append(&stack);

        let log_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        copy.connect_clicked(glib::clone!(
            #[strong]
            log_text,
            #[strong]
            shell,
            move |_| {
                let text = log_text.borrow().clone();
                if text.is_empty() {
                    shell.toast("The log is empty");
                    return;
                }
                if let Some(display) = gtk::gdk::Display::default() {
                    display.clipboard().set_text(&text);
                    shell.toast("Log copied to the clipboard");
                }
            },
        ));

        Self {
            shell,
            root,
            stack,
            list,
            summary,
            log_text,
        }
    }

    pub fn widget(&self) -> gtk::Box {
        self.root.clone()
    }

    /// Newest first, so the last action is visible without scrolling.
    pub fn render(&self, logs: &[LogEntry]) {
        widgets::clear_list(&self.list);
        for entry in logs.iter().rev() {
            self.list.append(&Self::row(entry));
        }
        self.summary.set_label(&format!(
            "{} entr{}",
            logs.len(),
            if logs.len() == 1 { "y" } else { "ies" }
        ));
        self.stack
            .set_visible_child_name(if logs.is_empty() { "empty" } else { "list" });
        *self.log_text.borrow_mut() = logs
            .iter()
            .map(|l| {
                let detail = match &l.detail {
                    Some(d) if !d.trim().is_empty() => {
                        let indented: Vec<String> =
                            d.lines().map(|line| format!("      {line}")).collect();
                        format!("\n{}", indented.join("\n"))
                    }
                    _ => String::new(),
                };
                format!(
                    "[{}] {:<7} {}{}",
                    l.time,
                    level_tag(l.level),
                    l.message,
                    detail
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    fn row(entry: &LogEntry) -> gtk::Widget {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(14)
            .margin_end(14)
            .build();
        row.append(
            &gtk::Image::builder()
                .icon_name(entry.level.icon())
                .pixel_size(16)
                .valign(gtk::Align::Start)
                .margin_top(2)
                .css_classes(vec![level_class(entry.level).to_string()])
                .build(),
        );
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .build();
        text.append(
            &gtk::Label::builder()
                .label(&entry.message)
                .xalign(0.0)
                .wrap(true)
                .hexpand(true)
                .build(),
        );
        if let Some(detail) = &entry.detail {
            if !detail.trim().is_empty() {
                text.append(&widgets::detail_view(detail));
            }
        }
        text.append(&widgets::row_subtitle(&entry.time));
        row.append(&text);
        row.upcast()
    }
}

fn level_tag(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "info",
        LogLevel::Success => "ok",
        LogLevel::Warning => "warn",
        LogLevel::Error => "error",
    }
}

fn level_class(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "dim-label",
        LogLevel::Success => "log-ok",
        LogLevel::Warning => "log-warn",
        LogLevel::Error => "log-error",
    }
}

//! "Mounts" page: everything currently mounted, with unmount/open actions.

use crate::error::{Error, Result};
use crate::model::{MountEntry, MountKind, Protocol};
use crate::mounts;
use crate::ui::shell::Shell;
use crate::ui::widgets;
use adw::prelude::*;
use gtk::glib;

pub struct MountsPage {
    shell: Shell,
    root: gtk::Box,
    stack: gtk::Stack,
    list: gtk::ListBox,
    summary: gtk::Label,
    system_toggle: gtk::ToggleButton,
}

impl MountsPage {
    pub fn new(shell: Shell) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();

        // ── toolbar: count + "show system mounts" ─────────────────────────
        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(6)
            .margin_start(18)
            .margin_end(18)
            .build();
        let summary = gtk::Label::builder()
            .label("Loading mounts…")
            .xalign(0.0)
            .hexpand(true)
            .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
            .build();
        let system_toggle = gtk::ToggleButton::builder()
            .label("System mounts")
            .tooltip_text("Also show kernel and container filesystems")
            .css_classes(vec!["flat".to_string()])
            .build();
        bar.append(&summary);
        bar.append(&system_toggle);
        root.append(&bar);

        // ── list / empty state ────────────────────────────────────────────
        let list = widgets::list_box();
        let list_scroll = widgets::scroller(&list);
        let empty = widgets::empty_state(
            "drive-removable-media-symbolic",
            "Nothing is mounted",
            "Connect a share or plug in a drive — then it will show up here.\nUse “Add Share” to mount an SMB, NFS, SSH, WebDAV or FTP folder.",
        );
        let stack = gtk::Stack::builder().vexpand(true).build();
        stack.add_named(&list_scroll, Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.set_visible_child_name("empty");
        root.append(&stack);

        let page = Self {
            shell: shell.clone(),
            root,
            stack,
            list,
            summary,
            system_toggle: system_toggle.clone(),
        };

        // The toggle only flips a preference; the pump refreshes afterwards.
        let bus = shell.bus.clone();
        let engine = shell.engine.clone();
        system_toggle.connect_toggled(move |button| {
            let show = button.is_active();
            let bus = bus.clone();
            let engine = engine.clone();
            std::thread::spawn(move || {
                let _ = engine.update_config(|cfg| cfg.show_system_mounts = show);
                bus.send(crate::ui::bus::Msg::Config(Box::new(engine.config())));
                bus.send(crate::ui::bus::Msg::Ping);
            });
        });

        page
    }

    pub fn widget(&self) -> gtk::Box {
        self.root.clone()
    }

    pub fn system_toggle(&self) -> &gtk::ToggleButton {
        &self.system_toggle
    }

    /// Rebuild the list from a fresh mount table.
    pub fn render(&self, entries: &[MountEntry]) {
        widgets::clear_list(&self.list);
        for entry in entries {
            self.list.append(&self.row(entry));
        }
        let network = entries
            .iter()
            .filter(|e| e.kind == MountKind::Network || e.kind == MountKind::Gvfs)
            .count();
        let removable = entries
            .iter()
            .filter(|e| e.kind == MountKind::Removable)
            .count();
        let total = entries.len();
        self.summary.set_label(&format!(
            "{total} mount{} — {network} network, {removable} removable",
            if total == 1 { "" } else { "s" }
        ));
        self.stack
            .set_visible_child_name(if total == 0 { "empty" } else { "list" });
    }

    /// One list row per mount.
    fn row(&self, entry: &MountEntry) -> gtk::Widget {
        let shell = self.shell.clone();
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(14)
            .margin_end(14)
            .build();

        row.append(
            &gtk::Image::builder()
                .icon_name(entry.icon())
                .pixel_size(32)
                .valign(gtk::Align::Center)
                .build(),
        );

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let title_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        title_row.append(&widgets::row_title(&entry.title()));
        title_row.append(&widgets::badge(&entry.badge(), badge_class(entry)));
        if entry.in_fstab {
            title_row.append(&widgets::badge(
                if entry.managed { "fstab ✓" } else { "fstab" },
                "badge-managed",
            ));
        }
        if !entry.writable {
            title_row.append(&widgets::badge("read-only", "badge-warn"));
        }
        text.append(&title_row);
        text.append(&widgets::row_subtitle(&entry.subtitle()));
        text.append(&widgets::row_subtitle(&format!(
            "{} · {}",
            entry.kind.label(),
            entry.fstype
        )));
        row.append(&text);

        // ── actions ───────────────────────────────────────────────────────
        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();

        let open = widgets::icon_button("folder-open-symbolic", "Open in Files", &[]);
        let target = entry.target.clone();
        let uri = entry.gvfs_uri.clone();
        open.connect_clicked(glib::clone!(
            #[strong]
            shell,
            move |_| {
                let what = uri.clone().unwrap_or_else(|| format!("file://{}", target));
                let engine = shell.engine.clone();
                let bus = shell.bus.clone();
                std::thread::spawn(move || {
                    let path = what
                        .strip_prefix("file://")
                        .map(str::to_string)
                        .unwrap_or_else(|| what.clone());
                    if let Err(e) = engine.open(&path) {
                        bus.error("Could not open the folder", e);
                    }
                });
            },
        ));
        actions.append(&open);

        let unmount = widgets::text_button("Unmount", "Unmount this filesystem", &["pill"]);
        if entry.kind == MountKind::Fixed {
            unmount.add_css_class("destructive-action");
            unmount.set_tooltip_text(Some("Unmount this internal filesystem — be careful"));
        }
        let entry_data = entry.clone();
        unmount.connect_clicked(glib::clone!(
            #[strong]
            shell,
            move |button| {
                unmount_clicked(&shell, &entry_data, false, button);
            },
        ));
        actions.append(&unmount);

        row.append(&actions);
        row.upcast()
    }
}

fn badge_class(entry: &MountEntry) -> &'static str {
    match entry.protocol {
        Protocol::Cifs => "badge-smb",
        Protocol::Nfs => "badge-nfs",
        Protocol::Sshfs => "badge-ssh",
        Protocol::WebDav => "badge-web",
        Protocol::Ftp => "badge-ftp",
        Protocol::Block => "badge-disk",
        Protocol::Bind => "badge-bind",
        Protocol::Custom => "badge-other",
    }
}

/// Unmount, offering a lazy unmount when the target is busy.
fn unmount_clicked(shell: &Shell, entry: &MountEntry, lazy: bool, button: &gtk::Button) {
    button.set_sensitive(false);
    shell.busy(true);
    let engine = shell.engine.clone();
    let bus = shell.bus.clone();
    let target = entry.target.clone();
    std::thread::spawn(move || {
        // Re-read the mount table so we never act on stale information.
        let result: Result<String> =
            engine
                .all_mounts()
                .and_then(|list| match mounts::find_by_target(&list, &target) {
                    Some(live) => engine.unmount(&live, lazy),
                    None => Err(Error::new(format!("`{target}` is no longer mounted"))),
                });
        if lazy {
            bus.done("Lazy unmount", result);
        } else {
            crate::ui::bus::report_unmount(&bus, &target, result);
        }
        bus.send(crate::ui::bus::Msg::Ping);
    });
}

/// Called by the pump when an unmount failed because the target is busy.
pub fn offer_lazy_unmount(shell: &Shell, target: &str, detail: &str) {
    let target = target.to_string();
    let detail = detail.to_string();
    let shell_for_body = shell.clone();
    shell.confirm(
        "Unmount anyway?",
        &format!(
            "`{target}` is still in use.\n\nA lazy unmount detaches it now and cleans up as soon as the last file is closed.\n\n{}",
            if detail.is_empty() { String::new() } else { detail }
        ),
        "Unmount anyway",
        move || {
            let engine = shell_for_body.engine.clone();
            let bus = shell_for_body.bus.clone();
            std::thread::spawn(move || {
                let result = engine
                    .all_mounts()
                    .and_then(|list| match mounts::find_by_target(&list, &target) {
                        Some(live) => engine.unmount(&live, true),
                        None => Err(Error::new(format!("`{target}` is no longer mounted"))),
                    });
                bus.done("Lazy unmount", result);
                bus.send(crate::ui::bus::Msg::Ping);
            });
        },
    );
}

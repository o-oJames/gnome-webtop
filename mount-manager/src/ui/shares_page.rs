//! "Saved shares" page.

use crate::error::{Error, Result};
use crate::model::{MountEntry, SavedShare};
use crate::mounts;
use crate::ui::bus::Msg;
use crate::ui::shell::Shell;
use crate::ui::widgets;
use adw::prelude::*;
use gtk::glib;

pub struct SharesPage {
    shell: Shell,
    root: gtk::Box,
    stack: gtk::Stack,
    list: gtk::ListBox,
    summary: gtk::Label,
}

impl SharesPage {
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
            .label("Saved shares")
            .xalign(0.0)
            .hexpand(true)
            .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
            .build();
        let add = widgets::text_button(
            "Add Share",
            "Define a new share",
            &["suggested-action", "pill"],
        );
        bar.append(&summary);
        bar.append(&add);
        root.append(&bar);

        let list = widgets::list_box();
        let empty = widgets::empty_state(
            "folder-remote-symbolic",
            "No saved shares yet",
            "Save a share and Mount Manager will remember the server, the path and your preferences — one click to mount it again.",
        );
        let stack = gtk::Stack::builder().vexpand(true).build();
        stack.add_named(&widgets::scroller(&list), Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.set_visible_child_name("empty");
        root.append(&stack);

        let page = Self {
            shell,
            root,
            stack,
            list,
            summary,
        };
        add.connect_clicked(glib::clone!(
            #[strong(rename_to = shell)]
            page.shell,
            move |_| {
                shell.bus.send(Msg::OpenShareDialog(None));
            },
        ));
        page
    }

    pub fn widget(&self) -> gtk::Box {
        self.root.clone()
    }

    pub fn render(&self, shares: &[(SavedShare, Option<MountEntry>)]) {
        widgets::clear_list(&self.list);
        for (share, mounted) in shares {
            self.list.append(&self.row(share, mounted.as_ref()));
        }
        let mounted_count = shares.iter().filter(|(_, m)| m.is_some()).count();
        self.summary.set_label(&format!(
            "{} saved share{} — {} mounted",
            shares.len(),
            if shares.len() == 1 { "" } else { "s" },
            mounted_count
        ));
        self.stack
            .set_visible_child_name(if shares.is_empty() { "empty" } else { "list" });
    }

    fn row(&self, share: &SavedShare, mounted: Option<&MountEntry>) -> gtk::Widget {
        let shell = self.shell.clone();
        let request = share.request.clone();
        let id = share.id.clone();

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
                .icon_name(request.protocol.icon())
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
        title_row.append(&widgets::row_title(&request.display_name()));
        title_row.append(&widgets::badge(request.protocol.short(), "badge-other"));
        if request.persist {
            title_row.append(&widgets::badge("at login", "badge-managed"));
        }
        if share.auto_mount {
            title_row.append(&widgets::badge("auto", "badge-ok"));
        }
        if mounted.is_some() {
            title_row.append(&widgets::badge("mounted", "badge-ok"));
        }
        text.append(&title_row);
        text.append(&widgets::row_subtitle(&format!(
            "{}  →  {}",
            request.display_source(),
            request.mount_point
        )));
        let mut meta = format!("method: {}", request.method.short());
        if let Some(last) = &share.last_used {
            meta.push_str(&format!(" · last used {last}"));
        }
        if share.password_ref.is_some() {
            meta.push_str(" · password stored");
        }
        text.append(&widgets::row_subtitle(&meta));
        row.append(&text);

        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();

        if let Some(entry) = mounted {
            let target = entry.target.clone();
            let unmount = widgets::text_button("Unmount", "Unmount this share", &["pill"]);
            unmount.connect_clicked(glib::clone!(
                #[strong]
                shell,
                move |button| {
                    button.set_sensitive(false);
                    shell.busy(true);
                    let engine = shell.engine.clone();
                    let bus = shell.bus.clone();
                    let target = target.clone();
                    std::thread::spawn(move || {
                        let result: Result<String> = engine.all_mounts().and_then(|list| {
                            match mounts::find_by_target(&list, &target) {
                                Some(live) => engine.unmount(&live, false),
                                None => Err(Error::new(format!("`{target}` is not mounted"))),
                            }
                        });
                        crate::ui::bus::report_unmount(&bus, &target, result);
                        bus.send(Msg::Ping);
                    });
                },
            ));
            actions.append(&unmount);
        } else {
            let mount = widgets::text_button(
                "Mount",
                "Mount this share now",
                &["pill", "suggested-action"],
            );
            let mount_id = id.clone();
            mount.connect_clicked(glib::clone!(
                #[strong]
                shell,
                move |button| {
                    button.set_sensitive(false);
                    shell.busy(true);
                    let engine = shell.engine.clone();
                    let bus = shell.bus.clone();
                    let id = mount_id.clone();
                    std::thread::spawn(move || {
                        let result = engine.mount_saved(&id);
                        bus.done("Mount", result);
                        bus.send(Msg::Ping);
                    });
                },
            ));
            actions.append(&mount);
        }

        let open =
            widgets::icon_button("folder-open-symbolic", "Open the mount point in Files", &[]);
        let target = request.mount_point.clone();
        open.connect_clicked(glib::clone!(
            #[strong]
            shell,
            move |_| {
                let engine = shell.engine.clone();
                let bus = shell.bus.clone();
                let target = target.clone();
                std::thread::spawn(move || {
                    if let Err(e) = engine.open(&target) {
                        bus.error("Could not open the folder", e);
                    }
                });
            },
        ));
        actions.append(&open);

        let edit = widgets::icon_button("document-edit-symbolic", "Edit this share", &[]);
        let edit_request = request.clone();
        edit.connect_clicked(glib::clone!(
            #[strong]
            shell,
            move |_| {
                shell
                    .bus
                    .send(Msg::OpenShareDialog(Some(Box::new(edit_request.clone()))));
            },
        ));
        actions.append(&edit);

        let delete = widgets::icon_button(
            "user-trash-symbolic",
            "Delete this share",
            &["destructive-action"],
        );
        let delete_id = id.clone();
        let name = request.display_name();
        delete.connect_clicked(glib::clone!(
            #[strong] shell,
            move |_| {
            let engine = shell.engine.clone();
            let bus = shell.bus.clone();
            let share_id = delete_id.clone();
            shell.confirm(
                "Delete this share?",
                &format!("“{name}” will be removed from the list. Files on the server are not touched."),
                "Delete",
                move || {
                    std::thread::spawn(move || {
                        let result = engine.delete_share(&share_id);
                        bus.done("Delete share", result);
                        bus.send(Msg::Ping);
                    });
                },
            );
        },
        ));
        actions.append(&delete);

        row.append(&actions);
        row.upcast()
    }
}

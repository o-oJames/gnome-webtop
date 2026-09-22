//! "Devices" page: block devices from `lsblk` with mount/unmount actions.

use crate::error::{Error, Result};
use crate::model::BlockDevice;
use crate::mounts;
use crate::ui::shell::Shell;
use crate::ui::widgets;
use adw::prelude::*;
use gtk::glib;

pub struct DevicesPage {
    shell: Shell,
    root: gtk::Box,
    stack: gtk::Stack,
    list: gtk::ListBox,
    summary: gtk::Label,
}

impl DevicesPage {
    pub fn new(shell: Shell) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();

        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .margin_top(6)
            .margin_start(18)
            .margin_end(18)
            .build();
        let summary = gtk::Label::builder()
            .label("Looking for devices…")
            .xalign(0.0)
            .hexpand(true)
            .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
            .build();
        bar.append(&summary);
        root.append(&bar);

        let list = widgets::list_box();
        let empty = widgets::empty_state(
            "drive-harddisk-symbolic",
            "No external drives",
            "Plug in a USB stick, an SD card or an external disk and it will appear here.\nDrives that are already mounted are listed on the Mounts page.",
        );
        let stack = gtk::Stack::builder().vexpand(true).build();
        stack.add_named(&widgets::scroller(&list), Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.set_visible_child_name("empty");
        root.append(&stack);

        Self {
            shell,
            root,
            stack,
            list,
            summary,
        }
    }

    pub fn widget(&self) -> gtk::Box {
        self.root.clone()
    }

    pub fn render(&self, devices: &[BlockDevice]) {
        widgets::clear_list(&self.list);
        for device in devices {
            self.list.append(&self.row(device));
        }
        let mounted = devices.iter().filter(|d| d.mounted()).count();
        self.summary.set_label(&format!(
            "{} device{} — {} mounted",
            devices.len(),
            if devices.len() == 1 { "" } else { "s" },
            mounted
        ));
        self.stack
            .set_visible_child_name(if devices.is_empty() { "empty" } else { "list" });
    }

    fn row(&self, device: &BlockDevice) -> gtk::Widget {
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
                .icon_name(device.icon())
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
        title_row.append(&widgets::row_title(&device.title()));
        if !device.fstype.is_empty() {
            title_row.append(&widgets::badge(&device.fstype, "badge-disk"));
        }
        if device.removable || device.transport == "usb" {
            title_row.append(&widgets::badge("removable", "badge-managed"));
        }
        if device.mounted() {
            title_row.append(&widgets::badge("mounted", "badge-ok"));
        }
        text.append(&title_row);
        text.append(&widgets::row_subtitle(&device.subtitle()));
        if let Some(mp) = device.mountpoints.first() {
            text.append(&widgets::row_subtitle(&format!("mounted on {mp}")));
        }
        row.append(&text);

        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();

        let data = device.clone();
        if device.mounted() {
            let target = device.mountpoints.first().cloned().unwrap_or_default();
            let open_target = target.clone();
            let unmount = widgets::text_button("Unmount", "Unmount this device", &["pill"]);
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
                        bus.send(crate::ui::bus::Msg::Ping);
                    });
                },
            ));
            actions.append(&unmount);

            let open = widgets::icon_button("folder-open-symbolic", "Open in Files", &[]);
            open.connect_clicked(glib::clone!(
                #[strong]
                shell,
                move |_| {
                    let engine = shell.engine.clone();
                    let bus = shell.bus.clone();
                    let target = open_target.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = engine.open(&target) {
                            bus.error("Could not open the folder", e);
                        }
                    });
                },
            ));
            actions.append(&open);
        } else {
            let mount = widgets::text_button(
                "Mount",
                "Mount this device under /media",
                &["pill", "suggested-action"],
            );
            mount.connect_clicked(glib::clone!(
                #[strong]
                shell,
                move |button| {
                    button.set_sensitive(false);
                    shell.busy(true);
                    let engine = shell.engine.clone();
                    let bus = shell.bus.clone();
                    let data = data.clone();
                    std::thread::spawn(move || {
                        let result = engine.mount_device(&data, None);
                        bus.done("Mount", result);
                        bus.send(crate::ui::bus::Msg::Ping);
                    });
                },
            ));
            actions.append(&mount);

            if device.fstype.is_empty() {
                mount.set_sensitive(false);
                mount.set_tooltip_text(Some(
                    "No filesystem was detected — format the device first (GNOME Disks)",
                ));
            }
        }

        row.append(&actions);
        row.upcast()
    }
}

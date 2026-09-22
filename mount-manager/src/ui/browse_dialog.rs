//! Network browsing dialog: mDNS servers on the left, the shares or NFS exports
//! a server offers on the right.

use crate::discover::{NfsExport, Service, SmbShare};
use crate::error::Result;
use crate::model::Protocol;
use crate::ui::bus::{attach, spawn, LocalChannel};
use crate::ui::shell::Shell;
use crate::ui::widgets;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

enum BrowseMsg {
    Servers(Protocol, Vec<Service>),
    SmbShares(String, Result<Vec<SmbShare>>),
    NfsExports(String, Result<Vec<NfsExport>>),
}

/// What the user picked.
#[derive(Debug, Clone)]
pub struct Pick {
    pub host: String,
    pub path: String,
    pub port: Option<u16>,
}

/// Open the browser for `protocol`; `on_pick` runs on the UI thread.
pub fn open<F>(shell: &Shell, protocol: Protocol, on_pick: F)
where
    F: Fn(Pick) + 'static,
{
    let mut builder = adw::Window::builder()
        .title(format!("Browse {} shares", protocol.short()))
        .modal(true)
        .default_width(760)
        .default_height(520)
        .icon_name("network-workgroup-symbolic");
    if let Some(app) = shell.window.application() {
        builder = builder.application(&app);
    }
    let window = builder.build();

    let header = adw::HeaderBar::builder().build();
    let cancel = gtk::Button::builder().label("Cancel").build();
    let refresh = widgets::icon_button("view-refresh-symbolic", "Search the network again", &[]);
    header.pack_start(&cancel);
    header.pack_end(&refresh);

    // ── host entry + list button ──────────────────────────────────────────
    let host_row = adw::EntryRow::builder()
        .title("Server name or IP address")
        .build();
    let list_button = widgets::text_button(
        "List shares",
        "Ask this server what it offers",
        &["suggested-action", "pill"],
    );
    let host_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .build();
    let host_group = adw::PreferencesGroup::builder()
        .title("Server")
        .description(match protocol {
            Protocol::Nfs => "NFS exports are read with `showmount -e` (package: nfs-common).",
            Protocol::Cifs => {
                "Shares are read with `smbclient -L`. Anonymous listing is tried first."
            }
            _ => "Enter a server name, or pick one discovered on the network.",
        })
        .build();
    host_box.append(&host_row);
    host_box.append(&list_button);
    host_group.add(&host_box);

    // ── two lists side by side ────────────────────────────────────────────
    let servers_label = gtk::Label::builder()
        .label("Servers on this network")
        .xalign(0.0)
        .css_classes(vec!["caption".to_string(), "dim-label".to_string()])
        .margin_bottom(6)
        .build();
    let servers_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list".to_string()])
        .build();
    let servers_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(12)
        .margin_end(6)
        .margin_top(12)
        .build();
    servers_box.append(&servers_label);
    servers_box.append(&widgets::scroller(&servers_list));

    let shares_label = gtk::Label::builder()
        .label("Shares")
        .xalign(0.0)
        .css_classes(vec!["caption".to_string(), "dim-label".to_string()])
        .margin_bottom(6)
        .build();
    let shares_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list".to_string()])
        .build();
    let shares_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(6)
        .margin_end(12)
        .margin_top(12)
        .build();
    shares_box.append(&shares_label);
    shares_box.append(&widgets::scroller(&shares_list));

    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .position(330)
        .resize_start_child(false)
        .resize_end_child(true)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .vexpand(true)
        .build();
    paned.set_start_child(Some(&servers_box));
    paned.set_end_child(Some(&shares_box));

    // ── footer ────────────────────────────────────────────────────────────
    let status = gtk::Label::builder()
        .label("Searching the network…")
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
        .build();
    let use_button = gtk::Button::builder()
        .label("Use this")
        .sensitive(false)
        .css_classes(vec!["suggested-action".to_string(), "pill".to_string()])
        .build();
    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(12)
        .build();
    footer.append(&status);
    footer.append(&use_button);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&host_group);
    content.append(&paned);
    content.append(&footer);

    let toolbar = adw::ToolbarView::builder().build();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));

    // ── state ─────────────────────────────────────────────────────────────
    // Clone the engine up front: the closures below are moved into
    // `idle_add_local`, so they may not borrow `shell`.
    let engine = shell.engine.clone();
    let (tx, rx) = LocalChannel::<BrowseMsg>::new();
    let selection: Rc<RefCell<Option<Pick>>> = Rc::new(RefCell::new(None));

    // Discover mDNS servers.
    let engine_for_discover = engine.clone();
    let do_discover = {
        let (tx, status, servers_list, window) = (
            tx.clone(),
            status.clone(),
            servers_list.clone(),
            window.clone(),
        );
        let engine = engine_for_discover;
        move |proto: Protocol| {
            widgets::clear_list(&servers_list);
            status.set_label("Searching the network (mDNS)…");
            let engine = engine.clone();
            spawn(&tx, move || {
                let services = engine.discover(proto);
                BrowseMsg::Servers(proto, services)
            });
            let _ = window;
        }
    };

    // List the shares/exports of one host.
    let do_list = {
        let (tx, status, shares_list) = (tx.clone(), status.clone(), shares_list.clone());
        move |host: String, proto: Protocol, user: String, password: String| {
            widgets::clear_list(&shares_list);
            status.set_label(&format!("Asking `{host}` for its shares…"));
            let engine = engine.clone();
            if proto == Protocol::Nfs {
                let host2 = host.clone();
                spawn(&tx, move || {
                    BrowseMsg::NfsExports(host2, engine.nfs_exports(&host))
                });
            } else {
                let host2 = host.clone();
                spawn(&tx, move || {
                    BrowseMsg::SmbShares(host2, engine.smb_shares(&host, &user, &password))
                });
            }
        }
    };

    let selection_for_rows = Rc::clone(&selection);
    let render_share_row = {
        let (use_button, status, selection) =
            (use_button.clone(), status.clone(), selection_for_rows);
        move |host: String, path: String, label: String, subtitle: String| {
            let row = clickable_row(&label, &subtitle);
            let (use_button, status, selection) =
                (use_button.clone(), status.clone(), selection.clone());
            let (host, path) = (host.clone(), path.clone());
            row.connect_clicked(move |_| {
                *selection.borrow_mut() = Some(Pick {
                    host: host.clone(),
                    path: path.clone(),
                    port: None,
                });
                use_button.set_sensitive(true);
                status.set_label(&format!("Selected {label}"));
            });
            row
        }
    };

    // ── signal wiring ─────────────────────────────────────────────────────
    let host_row_for_list = host_row.clone();
    let do_list_click = do_list.clone();
    let shell_for_click = shell.clone();
    list_button.connect_clicked(move |_| {
        let host = host_row_for_list.text().trim().to_string();
        if host.is_empty() {
            shell_for_click.toast("Enter a server name first");
            return;
        }
        do_list_click(host, protocol, String::new(), String::new());
    });
    host_row.connect_activate(glib::clone!(
        #[strong]
        list_button,
        move |_| {
            list_button.activate();
        },
    ));

    let do_discover_refresh = do_discover.clone();
    refresh.connect_clicked(move |_| do_discover_refresh(protocol));

    let on_pick = Rc::new(on_pick);
    let pick_for_button = Rc::clone(&on_pick);
    let window_for_button = window.clone();
    let selection_for_button = Rc::clone(&selection);
    use_button.connect_clicked(move |_| {
        if let Some(pick) = selection_for_button.borrow().clone() {
            pick_for_button(pick);
        }
        window_for_button.close();
    });
    cancel.connect_clicked(glib::clone!(
        #[strong]
        window,
        move |_| window.close(),
    ));

    // ── message pump for this dialog ──────────────────────────────────────
    let servers_list_pump = servers_list.clone();
    let shares_list_pump = shares_list.clone();
    let status_pump = status.clone();
    let host_row_pump = host_row.clone();
    let shell_pump = shell.clone();
    let do_list_pump = do_list.clone();
    let render_row = render_share_row.clone();
    attach(rx, move |msg| match msg {
        BrowseMsg::Servers(proto, services) => {
            widgets::clear_list(&servers_list_pump);
            if services.is_empty() {
                status_pump.set_label(
                    "No servers were announced on this network.\nInstall `avahi-utils` for mDNS discovery, or type the server name yourself.",
                );
            } else {
                status_pump.set_label(&format!("{} server(s) found — pick one", services.len()));
            }
            for service in services {
                let row = clickable_row(
                    &service.name,
                    &format!("{} · {}:{}", service.host, service.address, service.port),
                );
                let host = service.host.clone();
                let host_row = host_row_pump.clone();
                let do_list = do_list_pump.clone();
                let port = service.port;
                let shell = shell_pump.clone();
                row.connect_clicked(move |_| {
                    host_row.set_text(&host);
                    let _ = port;
                    let _ = &shell;
                    do_list(host.clone(), proto, String::new(), String::new());
                });
                servers_list_pump.append(&row);
            }
        }
        BrowseMsg::SmbShares(host, result) => match result {
            Ok(shares) => {
                widgets::clear_list(&shares_list_pump);
                status_pump.set_label(&format!("{} share(s) on {host}", shares.len()));
                for share in shares {
                    let subtitle = if share.comment.is_empty() {
                        share.kind.clone()
                    } else {
                        share.comment.clone()
                    };
                    shares_list_pump.append(&render_row(
                        host.clone(),
                        share.name.clone(),
                        share.name.clone(),
                        subtitle,
                    ));
                }
            }
            Err(e) => {
                widgets::clear_list(&shares_list_pump);
                status_pump.set_label(&format!(
                    "{}{}",
                    e.message,
                    e.detail.map(|d| format!("\n{d}")).unwrap_or_default()
                ));
            }
        },
        BrowseMsg::NfsExports(host, result) => match result {
            Ok(exports) => {
                widgets::clear_list(&shares_list_pump);
                status_pump.set_label(&format!("{} export(s) on {host}", exports.len()));
                for export in exports {
                    shares_list_pump.append(&render_row(
                        host.clone(),
                        export.path.clone(),
                        export.path.clone(),
                        export.allowed,
                    ));
                }
            }
            Err(e) => {
                widgets::clear_list(&shares_list_pump);
                status_pump.set_label(&format!(
                    "{}{}",
                    e.message,
                    e.detail.map(|d| format!("\n{d}")).unwrap_or_default()
                ));
            }
        },
    });

    window.set_transient_for(Some(&shell.window));
    window.present();

    // Kick off discovery once the window is on screen.
    let do_discover_start = do_discover;
    glib::idle_add_local(move || {
        do_discover_start(protocol);
        glib::ControlFlow::Break
    });
}

/// A full-width, clickable list row with a title and a subtitle.
pub fn clickable_row(title: &str, subtitle: &str) -> gtk::Button {
    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(10)
        .margin_end(10)
        .build();
    box_.append(&widgets::row_title(title));
    if !subtitle.is_empty() {
        box_.append(&widgets::row_subtitle(subtitle));
    }
    gtk::Button::builder()
        .child(&box_)
        .css_classes(vec!["flat".to_string(), "browse-row".to_string()])
        .hexpand(true)
        .build()
}

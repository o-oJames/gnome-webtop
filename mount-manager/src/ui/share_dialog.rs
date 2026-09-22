//! The "Add / edit share" dialog: the heart of the app.

use crate::error::Result;
use crate::model::{MountMethod, Protocol, ShareRequest};
use crate::platform;
use crate::ui::browse_dialog::{self, Pick};
use crate::ui::bus::{attach, spawn, LocalChannel};
use crate::ui::shell::Shell;
use crate::ui::widgets;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

enum DialogMsg {
    Busy(bool),
    Tested(Result<String>),
    Mounted(Result<String>),
    Devices(Vec<crate::model::BlockDevice>),
}

struct ShareDialog {
    shell: Shell,
    window: adw::Window,
    protocol_row: adw::ComboRow,
    host_row: adw::EntryRow,
    host_browse: gtk::Button,
    path_row: adw::EntryRow,
    path_browse: gtk::Button,
    port_row: adw::EntryRow,
    device_row: adw::ActionRow,
    device_button: gtk::Button,
    bind_row: adw::ActionRow,
    bind_button: gtk::Button,
    custom_source_row: adw::EntryRow,
    custom_fstype_row: adw::EntryRow,
    connection_group: adw::PreferencesGroup,
    user_row: adw::EntryRow,
    pass_row: adw::PasswordEntryRow,
    domain_row: adw::EntryRow,
    identity_row: adw::EntryRow,
    identity_button: gtk::Button,
    remember_row: adw::SwitchRow,
    auth_group: adw::PreferencesGroup,
    name_row: adw::EntryRow,
    target_row: adw::EntryRow,
    target_button: gtk::Button,
    ro_row: adw::SwitchRow,
    method_row: adw::ComboRow,
    ownership_row: adw::SwitchRow,
    options_row: adw::EntryRow,
    persist_row: adw::SwitchRow,
    bookmark_row: adw::SwitchRow,
    save_row: adw::SwitchRow,
    status: gtk::Label,
    test_button: gtk::Button,
    mount_button: gtk::Button,
    tx: LocalChannel<DialogMsg>,
    /// `true` while editing an existing share.
    editing: bool,
    cancel: gtk::Button,
    /// Chosen block device (`/dev/sdb1`) — the row subtitle is only a label.
    device_path: Rc<RefCell<String>>,
    /// Chosen bind source directory.
    bind_path: Rc<RefCell<String>>,
}

/// Open the dialog, optionally pre-filled (edit, or a URI from the desktop).
pub fn open(shell: &Shell, initial: Option<ShareRequest>) {
    let editing = initial
        .as_ref()
        .map(|r| !r.name.is_empty())
        .unwrap_or(false);
    let request = initial.unwrap_or_default();

    let mut builder = adw::Window::builder()
        .title(if editing { "Edit Share" } else { "Add Share" })
        .modal(true)
        .default_width(660)
        .default_height(820)
        .icon_name("mount-manager");
    if let Some(app) = shell.window.application() {
        builder = builder.application(&app);
    }
    let window = builder.build();

    let (tx, rx) = LocalChannel::<DialogMsg>::new();

    // ── header ────────────────────────────────────────────────────────────
    let header = adw::HeaderBar::builder().build();
    let cancel = gtk::Button::builder().label("Cancel").build();
    let test_button = gtk::Button::builder()
        .label("Test")
        .tooltip_text("Check the server and the share without mounting")
        .css_classes(vec!["flat".to_string()])
        .build();
    let mount_button = gtk::Button::builder()
        .label(if editing { "Save" } else { "Mount" })
        .tooltip_text("Mount this share now")
        .css_classes(vec!["suggested-action".to_string()])
        .build();
    header.pack_start(&cancel);
    header.pack_end(&test_button);
    header.pack_end(&mount_button);

    // ── connection group ──────────────────────────────────────────────────
    let protocols: Vec<String> = Protocol::ALL
        .iter()
        .map(|p| p.label().to_string())
        .collect();
    let protocol_row = adw::ComboRow::builder()
        .title("Protocol")
        .model(&gtk::StringList::new(
            &protocols.iter().map(|s| s.as_str()).collect::<Vec<&str>>(),
        ))
        .build();

    let host_row = adw::EntryRow::builder().title("Server").build();
    let host_browse = widgets::icon_button("network-workgroup-symbolic", "Browse the network", &[]);
    host_row.add_suffix(&host_browse);

    let path_row = adw::EntryRow::builder().title("Share name").build();
    let path_browse =
        widgets::icon_button("view-list-symbolic", "List the shares on this server", &[]);
    path_row.add_suffix(&path_browse);

    let port_row = adw::EntryRow::builder().title("Port").build();

    let device_row = adw::ActionRow::builder()
        .title("Device")
        .subtitle("Choose a disk or partition")
        .build();
    let device_button = widgets::text_button("Choose…", "List block devices", &["pill"]);
    device_row.add_suffix(&device_button);

    let bind_row = adw::ActionRow::builder()
        .title("Folder")
        .subtitle("The existing directory to bind")
        .build();
    let bind_button = widgets::text_button("Choose…", "Pick a folder", &["pill"]);
    bind_row.add_suffix(&bind_button);

    let custom_source_row = adw::EntryRow::builder().title("Source").build();
    let custom_fstype_row = adw::EntryRow::builder().title("Filesystem type").build();

    let connection_group = adw::PreferencesGroup::builder()
        .title("Connection")
        .description("Where the files live.")
        .build();
    connection_group.add(&protocol_row);
    connection_group.add(&host_row);
    connection_group.add(&path_row);
    connection_group.add(&port_row);
    connection_group.add(&device_row);
    connection_group.add(&bind_row);
    connection_group.add(&custom_source_row);
    connection_group.add(&custom_fstype_row);

    // ── authentication group ──────────────────────────────────────────────
    let user_row = adw::EntryRow::builder().title("User name").build();
    let pass_row = adw::PasswordEntryRow::builder().title("Password").build();
    let domain_row = adw::EntryRow::builder()
        .title("Domain / workgroup (optional)")
        .build();
    let identity_row = adw::EntryRow::builder()
        .title("SSH private key (optional)")
        .build();
    let identity_button = widgets::icon_button("document-open-symbolic", "Choose a key file", &[]);
    identity_row.add_suffix(&identity_button);
    let remember_row = adw::SwitchRow::builder()
        .title("Remember password")
        .subtitle(crate::secret::Backend::ConfigFile.label())
        .active(false)
        .build();
    let auth_group = adw::PreferencesGroup::builder()
        .title("Authentication")
        .build();
    auth_group.add(&user_row);
    auth_group.add(&pass_row);
    auth_group.add(&domain_row);
    auth_group.add(&identity_row);
    auth_group.add(&remember_row);

    // ── destination group ─────────────────────────────────────────────────
    let name_row = adw::EntryRow::builder()
        .title("Name (used for the mount point)")
        .build();
    let target_row = adw::EntryRow::builder().title("Mount point").build();
    let target_button = widgets::icon_button("folder-symbolic", "Choose a folder", &[]);
    target_row.add_suffix(&target_button);
    let ro_row = adw::SwitchRow::builder()
        .title("Read-only")
        .active(false)
        .build();
    let methods: Vec<String> = MountMethod::ALL
        .iter()
        .map(|m| m.label().to_string())
        .collect();
    let method_row = adw::ComboRow::builder()
        .title("Mount as")
        .model(&gtk::StringList::new(
            &methods.iter().map(|s| s.as_str()).collect::<Vec<&str>>(),
        ))
        .build();
    let ownership_row = adw::SwitchRow::builder()
        .title("Give my user ownership")
        .subtitle("chown the mount point afterwards (useful for NFS exports)")
        .active(false)
        .build();
    let dest_group = adw::PreferencesGroup::builder()
        .title("Destination")
        .build();
    dest_group.add(&name_row);
    dest_group.add(&target_row);
    dest_group.add(&ro_row);
    dest_group.add(&method_row);
    dest_group.add(&ownership_row);

    // ── options group ─────────────────────────────────────────────────────
    let options_row = adw::EntryRow::builder()
        .title("Extra mount options")
        .build();
    let persist_row = adw::SwitchRow::builder()
        .title("Mount at login")
        .subtitle("Writes an /etc/fstab entry (a backup is kept)")
        .active(false)
        .build();
    let bookmark_row = adw::SwitchRow::builder()
        .title("Add to the Files sidebar")
        .active(true)
        .build();
    let save_row = adw::SwitchRow::builder()
        .title("Save this share")
        .subtitle("Remember it in the “Shares” page")
        .active(true)
        .build();
    let options_group = adw::PreferencesGroup::builder()
        .title("Options")
        .description("Extra options are comma separated, for example: vers=1.0,soft,noserverino")
        .build();
    options_group.add(&options_row);
    options_group.add(&persist_row);
    options_group.add(&bookmark_row);
    options_group.add(&save_row);

    // ── test output ───────────────────────────────────────────────────────
    let status = gtk::Label::builder()
        .label("Fill in the fields above, then press Test or Mount.")
        .wrap(true)
        .selectable(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
        .build();
    let status_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(vec!["test-result".to_string()])
        .margin_top(6)
        .build();
    status_box.append(&status);
    let test_group = adw::PreferencesGroup::builder()
        .title("Connection test")
        .build();
    test_group.add(&status_box);

    // ── assemble ──────────────────────────────────────────────────────────
    let page = adw::PreferencesPage::builder().build();
    page.add(&connection_group);
    page.add(&auth_group);
    page.add(&dest_group);
    page.add(&options_group);
    page.add(&test_group);

    let toolbar = adw::ToolbarView::builder().build();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&widgets::scroller(&page)));
    window.set_content(Some(&toolbar));

    let dialog = Rc::new(ShareDialog {
        shell: shell.clone(),
        window: window.clone(),
        protocol_row: protocol_row.clone(),
        host_row: host_row.clone(),
        host_browse: host_browse.clone(),
        path_row: path_row.clone(),
        path_browse: path_browse.clone(),
        port_row: port_row.clone(),
        device_row: device_row.clone(),
        device_button: device_button.clone(),
        bind_row: bind_row.clone(),
        bind_button: bind_button.clone(),
        custom_source_row: custom_source_row.clone(),
        custom_fstype_row: custom_fstype_row.clone(),
        connection_group: connection_group.clone(),
        user_row: user_row.clone(),
        pass_row: pass_row.clone(),
        domain_row: domain_row.clone(),
        identity_row: identity_row.clone(),
        identity_button: identity_button.clone(),
        remember_row: remember_row.clone(),
        auth_group: auth_group.clone(),
        name_row: name_row.clone(),
        target_row: target_row.clone(),
        target_button: target_button.clone(),
        ro_row: ro_row.clone(),
        method_row: method_row.clone(),
        ownership_row: ownership_row.clone(),
        options_row: options_row.clone(),
        persist_row: persist_row.clone(),
        bookmark_row: bookmark_row.clone(),
        save_row: save_row.clone(),
        status: status.clone(),
        test_button: test_button.clone(),
        mount_button: mount_button.clone(),
        tx: tx.clone(),
        editing,
        cancel: cancel.clone(),
        device_path: Rc::new(RefCell::new(request.local_source.clone())),
        bind_path: Rc::new(RefCell::new(request.local_source.clone())),
    });

    fill(&dialog, &request);
    wire(&dialog);

    // Message pump for this dialog.
    let pump = Rc::clone(&dialog);
    attach(rx, move |msg| handle(&pump, msg));

    window.set_transient_for(Some(&shell.window));
    window.present();
    if !editing {
        protocol_row.grab_focus();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// populate / collect
// ─────────────────────────────────────────────────────────────────────────────

fn fill(d: &Rc<ShareDialog>, request: &ShareRequest) {
    let index = Protocol::ALL
        .iter()
        .position(|p| *p == request.protocol)
        .unwrap_or(0) as u32;
    d.protocol_row.set_selected(index);
    d.name_row.set_text(&request.name);
    d.host_row.set_text(&request.host);
    d.path_row.set_text(&request.remote_path);
    d.port_row
        .set_text(&request.port.map(|p| p.to_string()).unwrap_or_default());
    d.user_row.set_text(&request.username);
    d.pass_row.set_text(&request.password);
    d.domain_row.set_text(&request.domain);
    d.identity_row.set_text(&request.identity_file);
    d.target_row.set_text(&request.mount_point);
    d.options_row.set_text(&request.extra_options.join(","));
    d.custom_source_row.set_text(&request.custom_source);
    d.custom_fstype_row.set_text(&request.custom_fstype);
    d.port_row.set_text(
        &request
            .port
            .or_else(|| request.protocol.default_port())
            .map(|p| p.to_string())
            .unwrap_or_default(),
    );
    *d.device_path.borrow_mut() = request.local_source.clone();
    *d.bind_path.borrow_mut() = request.local_source.clone();
    let source = request.local_source.trim();
    d.device_row.set_subtitle(if source.is_empty() {
        "Choose a disk or partition"
    } else {
        source
    });
    d.bind_row.set_subtitle(if source.is_empty() {
        "The existing directory to bind"
    } else {
        source
    });
    d.ro_row.set_active(request.read_only);
    d.persist_row.set_active(request.persist);
    d.bookmark_row.set_active(request.bookmark);
    d.remember_row.set_active(request.remember_password);
    d.ownership_row.set_active(request.take_ownership);
    let method_index = MountMethod::ALL
        .iter()
        .position(|m| *m == request.method)
        .unwrap_or(0) as u32;
    d.method_row.set_selected(method_index);

    // Suggest a name and a mount point when the fields are empty.
    if d.name_row.text().trim().is_empty() && request.protocol.is_network() {
        let share = request
            .remote_path
            .trim()
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        if !share.is_empty() {
            d.name_row.set_text(&share);
        }
    }
    if d.target_row.text().trim().is_empty() {
        d.target_row
            .set_text(&request.suggested_mount_point(&platform::Caller::current()));
    }
    apply_visibility(d, request.protocol);
    // Show where remembered passwords would go.
    let backend = d.shell.engine.password_backend();
    d.remember_row.set_subtitle(backend.label());
}

/// Collect the form into a [`ShareRequest`].
fn collect(d: &ShareDialog) -> ShareRequest {
    let protocol = Protocol::ALL[d.protocol_row.selected() as usize];
    let method = MountMethod::ALL[d.method_row.selected() as usize];
    let port = d.port_row.text().trim().parse::<u16>().ok();
    let options = d
        .options_row
        .text()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let local_source = match protocol {
        Protocol::Block => d.device_path.borrow().clone(),
        Protocol::Bind => d.bind_path.borrow().clone(),
        _ => String::new(),
    };
    ShareRequest {
        name: d.name_row.text().to_string(),
        protocol,
        host: d.host_row.text().to_string(),
        remote_path: d.path_row.text().to_string(),
        mount_point: d.target_row.text().to_string(),
        username: d.user_row.text().to_string(),
        password: d.pass_row.text().to_string(),
        domain: d.domain_row.text().to_string(),
        port,
        extra_options: options,
        read_only: d.ro_row.is_active(),
        method,
        persist: d.persist_row.is_active(),
        bookmark: d.bookmark_row.is_active(),
        identity_file: d.identity_row.text().to_string(),
        remember_password: d.remember_row.is_active(),
        local_source,
        custom_source: d.custom_source_row.text().to_string(),
        custom_fstype: d.custom_fstype_row.text().to_string(),
        take_ownership: d.ownership_row.is_active(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// visibility per protocol
// ─────────────────────────────────────────────────────────────────────────────

fn apply_visibility(d: &ShareDialog, protocol: Protocol) {
    let network = protocol.is_network();
    d.host_row.set_visible(matches!(
        protocol,
        Protocol::Cifs | Protocol::Nfs | Protocol::Sshfs | Protocol::WebDav | Protocol::Ftp
    ));
    d.host_browse.set_visible(network);
    d.path_row.set_visible(network);
    d.path_browse
        .set_visible(matches!(protocol, Protocol::Cifs | Protocol::Nfs));
    d.port_row.set_visible(network);
    d.device_row.set_visible(protocol == Protocol::Block);
    d.bind_row.set_visible(protocol == Protocol::Bind);
    d.custom_source_row
        .set_visible(protocol == Protocol::Custom);
    d.custom_fstype_row
        .set_visible(protocol == Protocol::Custom || protocol == Protocol::Block);
    d.connection_group.set_visible(true);

    let auth = match protocol {
        Protocol::Cifs | Protocol::WebDav | Protocol::Ftp => true,
        Protocol::Sshfs => true,
        Protocol::Nfs => false, // NFS authenticates by client address
        Protocol::Block | Protocol::Bind | Protocol::Custom => false,
    };
    d.auth_group.set_visible(auth);
    d.domain_row.set_visible(protocol == Protocol::Cifs);
    d.identity_row.set_visible(protocol == Protocol::Sshfs);
    d.identity_button.set_visible(protocol == Protocol::Sshfs);
    d.ownership_row.set_visible(protocol != Protocol::Bind);

    // Protocol specific wording.
    match protocol {
        Protocol::Cifs => {
            d.host_row.set_title("Server");
            d.path_row.set_title("Share name");
            d.host_row.set_show_apply_button(false);
        }
        Protocol::Nfs => {
            d.host_row.set_title("Server");
            d.path_row.set_title("Export path");
        }
        Protocol::Sshfs => {
            d.host_row.set_title("Server");
            d.path_row.set_title("Remote directory");
        }
        Protocol::WebDav => {
            d.host_row.set_title("Server or full URL");
            d.path_row.set_title("Path");
        }
        Protocol::Ftp => {
            d.host_row.set_title("Server");
            d.path_row.set_title("Directory");
        }
        _ => {}
    }
    d.mount_button
        .set_label(if d.editing { "Save & mount" } else { "Mount" });
    let _ = network;
}

// ─────────────────────────────────────────────────────────────────────────────
// signals
// ─────────────────────────────────────────────────────────────────────────────

fn wire(d: &Rc<ShareDialog>) {
    // Protocol changes.
    let protocol_dialog = Rc::downgrade(d);
    d.protocol_row.connect_selected_notify(move |row| {
        let Some(dialog) = protocol_dialog.upgrade() else {
            return;
        };
        let protocol = Protocol::ALL[row.selected() as usize];
        apply_visibility(&dialog, protocol);
        // Suggest the default port for the protocol.
        if dialog.port_row.text().trim().is_empty() {
            if let Some(port) = protocol.default_port() {
                dialog.port_row.set_text(&port.to_string());
            }
        } else if let Some(port) = protocol.default_port() {
            let current = dialog.port_row.text().trim().to_string();
            let known: Vec<String> = Protocol::ALL
                .iter()
                .filter_map(|p| p.default_port().map(|n| n.to_string()))
                .collect();
            if known.contains(&current) {
                dialog.port_row.set_text(&port.to_string());
            }
        }
        // Suggest a fresh mount point when the user has not typed one.
        if dialog.target_row.text().trim().is_empty() {
            let request = collect(&dialog);
            dialog
                .target_row
                .set_text(&request.suggested_mount_point(&platform::Caller::current()));
        }
    });

    // Mount.
    let mount_dialog = Rc::downgrade(d);
    d.mount_button.connect_clicked(move |_| {
        let Some(dialog) = mount_dialog.upgrade() else {
            return;
        };
        start_mount(&dialog);
    });
    d.window.set_default_widget(Some(&d.mount_button));

    // Test.
    let test_dialog = Rc::downgrade(d);
    d.test_button.connect_clicked(move |_| {
        let Some(dialog) = test_dialog.upgrade() else {
            return;
        };
        start_test(&dialog);
    });

    // Cancel.
    let cancel_window = d.window.clone();
    d.cancel.connect_clicked(move |_| cancel_window.close());

    // Browse the network (host or path button).
    let host_browse_ref = Rc::downgrade(d);
    d.host_browse.connect_clicked(move |_| {
        let browse_ref = host_browse_ref.clone();
        let Some(dialog) = browse_ref.upgrade() else {
            return;
        };
        let protocol = current_protocol(&dialog);
        let shell = dialog.shell.clone();
        browse_dialog::open(&shell, protocol, move |pick: Pick| {
            let Some(dialog) = browse_ref.upgrade() else {
                return;
            };
            apply_pick(&dialog, &pick);
        });
    });
    let path_browse_ref = Rc::downgrade(d);
    d.path_browse.connect_clicked(move |_| {
        let browse_ref = path_browse_ref.clone();
        let Some(dialog) = browse_ref.upgrade() else {
            return;
        };
        let protocol = current_protocol(&dialog);
        let shell = dialog.shell.clone();
        browse_dialog::open(&shell, protocol, move |pick: Pick| {
            let Some(dialog) = browse_ref.upgrade() else {
                return;
            };
            apply_pick(&dialog, &pick);
        });
    });

    // Device picker.
    let device_dialog = Rc::downgrade(d);
    d.device_button.connect_clicked(move |_| {
        let Some(dialog) = device_dialog.upgrade() else {
            return;
        };
        open_device_picker(&dialog);
    });

    // Folder pickers (bind source, mount point, ssh key).
    let bind_dialog = Rc::downgrade(d);
    d.bind_button.connect_clicked(move |_| {
        let Some(dialog) = bind_dialog.upgrade() else {
            return;
        };
        let window = dialog.window.clone();
        let bind_row = dialog.bind_row.clone();
        let bind_path = Rc::clone(&dialog.bind_path);
        pick_folder(&window, "Choose the folder to bind", move |path| {
            *bind_path.borrow_mut() = path.clone();
            bind_row.set_subtitle(&path);
        });
    });
    let target_dialog = Rc::downgrade(d);
    d.target_button.connect_clicked(move |_| {
        let Some(dialog) = target_dialog.upgrade() else {
            return;
        };
        let window = dialog.window.clone();
        let target_row = dialog.target_row.clone();
        pick_folder(&window, "Choose the mount point", move |path| {
            target_row.set_text(&path);
        });
    });
    let identity_dialog = Rc::downgrade(d);
    d.identity_button.connect_clicked(move |_| {
        let Some(dialog) = identity_dialog.upgrade() else {
            return;
        };
        let window = dialog.window.clone();
        let identity_row = dialog.identity_row.clone();
        pick_file(&window, "Choose an SSH private key", move |path| {
            identity_row.set_text(&path);
        });
    });
}

fn current_protocol(d: &ShareDialog) -> Protocol {
    Protocol::ALL[d.protocol_row.selected() as usize]
}

fn apply_pick(d: &ShareDialog, pick: &Pick) {
    d.host_row.set_text(&pick.host);
    if !pick.path.is_empty() {
        d.path_row.set_text(&pick.path);
    }
    if let Some(port) = pick.port {
        d.port_row.set_text(&port.to_string());
    }
    // Suggest a mount point and a name from the picked share.
    let request = collect(d);
    if d.target_row.text().trim().is_empty() {
        d.target_row
            .set_text(&request.suggested_mount_point(&platform::Caller::current()));
    }
    if d.name_row.text().trim().is_empty() {
        d.name_row.set_text(&request.display_name());
    }
    d.status
        .set_label(&format!("Selected {} — press Test to check it.", pick.host));
}

// ─────────────────────────────────────────────────────────────────────────────
// actions
// ─────────────────────────────────────────────────────────────────────────────

fn start_test(d: &ShareDialog) {
    let request = collect(d);
    d.status.remove_css_class("error-text");
    d.status.set_label("Testing the connection…");
    d.test_button.set_sensitive(false);
    let engine = d.shell.engine.clone();
    let tx = d.tx.clone();
    spawn(&tx, move || {
        DialogMsg::Tested(engine.test_connection(request))
    });
    spawn(&tx, move || DialogMsg::Busy(true));
}

fn start_mount(d: &ShareDialog) {
    let request = collect(d);
    // Validate on the UI thread first so simple mistakes show up immediately.
    let caller = platform::Caller::current();
    match request.clone().normalize(&caller) {
        Ok(normalized) => {
            d.status.remove_css_class("error-text");
            d.status
                .set_label(&format!("Mounting {}…", normalized.display_source()));
            d.mount_button.set_sensitive(false);
            let save = normalized.remember_password || d.save_row.is_active();
            let engine = d.shell.engine.clone();
            let tx = d.tx.clone();
            spawn(&tx, move || {
                DialogMsg::Mounted(engine.mount(normalized, save))
            });
            spawn(&tx, move || DialogMsg::Busy(true));
        }
        Err(e) => {
            d.status.add_css_class("error-text");
            d.status.set_label(&e.message);
            d.shell.bus.error("Check the share settings", e);
        }
    }
}

fn open_device_picker(d: &ShareDialog) {
    let mut builder = adw::Window::builder()
        .title("Choose a device")
        .modal(true)
        .default_width(560)
        .default_height(460);
    if let Some(app) = d.shell.window.application() {
        builder = builder.application(&app);
    }
    let window = builder.build();
    let header = adw::HeaderBar::builder().build();
    let cancel = gtk::Button::builder().label("Cancel").build();
    header.pack_start(&cancel);
    let list = widgets::list_box();
    let status = gtk::Label::builder()
        .label("Reading block devices…")
        .xalign(0.0)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&status);
    content.append(&widgets::scroller(&list));
    let toolbar = adw::ToolbarView::builder().build();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));

    let (tx, rx) = LocalChannel::<DialogMsg>::new();
    let engine = d.shell.engine.clone();
    spawn(&tx, move || {
        DialogMsg::Devices(engine.devices().unwrap_or_default())
    });

    let device_row = d.device_row.clone();
    let device_path = Rc::clone(&d.device_path);
    let list_for_pump = list.clone();
    let status_for_pump = status.clone();
    let window_for_pump = window.clone();
    attach(rx, move |msg| match msg {
        DialogMsg::Devices(devices) => {
            widgets::clear_list(&list_for_pump);
            status_for_pump.set_label(&format!("{} device(s) — pick one", devices.len()));
            for device in devices {
                let row = browse_dialog::clickable_row(
                    &device.title(),
                    &format!("{} · {}", device.path, device.subtitle()),
                );
                let device_row = device_row.clone();
                let device_path = device_path.clone();
                let window = window_for_pump.clone();
                row.connect_clicked(move |_| {
                    *device_path.borrow_mut() = device.path.clone();
                    device_row.set_subtitle(&device.path);
                    window.close();
                });
                list_for_pump.append(&row);
            }
        }
        _ => {}
    });

    cancel.connect_clicked(glib::clone!(
        #[strong]
        window,
        move |_| window.close(),
    ));
    window.set_transient_for(Some(&d.window));
    window.present();
}

fn pick_folder<W, F>(parent: &W, title: &str, on_pick: F)
where
    W: glib::object::IsA<gtk::Window>,
    F: Fn(String) + 'static,
{
    let chooser = gtk::FileChooserNative::builder()
        .title(title)
        .action(gtk::FileChooserAction::SelectFolder)
        .modal(true)
        .transient_for(parent)
        .accept_label("Choose")
        .cancel_label("Cancel")
        .build();
    let on_pick = Rc::new(RefCell::new(Some(on_pick)));
    chooser.connect_response(glib::clone!(
        #[strong]
        chooser,
        #[strong]
        on_pick,
        move |_, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(file) = chooser.file() {
                    if let Some(path) = file.path() {
                        if let Some(f) = on_pick.borrow_mut().take() {
                            f(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        },
    ));
    chooser.show();
}

fn pick_file<W, F>(parent: &W, title: &str, on_pick: F)
where
    W: glib::object::IsA<gtk::Window>,
    F: Fn(String) + 'static,
{
    let chooser = gtk::FileChooserNative::builder()
        .title(title)
        .action(gtk::FileChooserAction::Open)
        .modal(true)
        .transient_for(parent)
        .accept_label("Use this key")
        .cancel_label("Cancel")
        .build();
    let on_pick = Rc::new(RefCell::new(Some(on_pick)));
    chooser.connect_response(glib::clone!(
        #[strong]
        chooser,
        #[strong]
        on_pick,
        move |_, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(file) = chooser.file() {
                    if let Some(path) = file.path() {
                        if let Some(f) = on_pick.borrow_mut().take() {
                            f(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        },
    ));
    chooser.show();
}

// ─────────────────────────────────────────────────────────────────────────────
// results
// ─────────────────────────────────────────────────────────────────────────────

fn handle(d: &Rc<ShareDialog>, msg: DialogMsg) {
    match msg {
        DialogMsg::Busy(busy) => {
            d.shell.busy(busy);
            if !busy {
                d.test_button.set_sensitive(true);
                d.mount_button.set_sensitive(true);
            }
        }
        DialogMsg::Tested(result) => {
            d.shell.busy(false);
            d.test_button.set_sensitive(true);
            match result {
                Ok(report) => {
                    d.status.remove_css_class("error-text");
                    d.status.add_css_class("ok-text");
                    d.status.set_label(&report);
                }
                Err(e) => {
                    d.status.remove_css_class("ok-text");
                    d.status.add_css_class("error-text");
                    d.status.set_label(&format!(
                        "{}{}",
                        e.message,
                        e.detail.map(|x| format!("\n\n{x}")).unwrap_or_default()
                    ));
                }
            }
        }
        DialogMsg::Mounted(result) => {
            d.shell.busy(false);
            d.mount_button.set_sensitive(true);
            match result {
                Ok(message) => {
                    d.shell.toast(&message);
                    d.shell.bus.send(crate::ui::bus::Msg::Ping);
                    d.window.close();
                }
                Err(e) => {
                    d.status.remove_css_class("ok-text");
                    d.status.add_css_class("error-text");
                    d.status.set_label(&e.message);
                    d.shell.show_error("Could not mount this share", &e);
                    d.shell.bus.send(crate::ui::bus::Msg::Ping);
                }
            }
        }
        DialogMsg::Devices(_) => {}
    }
}

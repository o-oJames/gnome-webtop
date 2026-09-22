//! Preferences + About dialogs.

use crate::config::{AppConfig, EscalationPref};
use crate::ui::bus::Msg;
use crate::ui::shell::Shell;
use crate::ui::widgets;
use adw::prelude::*;
use gtk::glib;
use std::rc::Rc;

pub fn open(shell: &Shell) {
    let mut builder = adw::Window::builder()
        .title("Preferences")
        .modal(true)
        .default_width(620)
        .default_height(720)
        .icon_name("preferences-system-symbolic");
    if let Some(app) = shell.window.application() {
        builder = builder.application(&app);
    }
    let window = builder.build();

    let header = adw::HeaderBar::builder().build();
    let close = gtk::Button::builder().label("Close").build();
    header.pack_start(&close);

    let config = shell.engine.config();

    // ── administrator rights ──────────────────────────────────────────────
    let labels: Vec<String> = EscalationPref::ALL
        .iter()
        .map(|p| p.label().to_string())
        .collect();
    let escalation_row = adw::ComboRow::builder()
        .title("Administrator rights")
        .model(&gtk::StringList::new(
            &labels.iter().map(|s| s.as_str()).collect::<Vec<&str>>(),
        ))
        .selected(
            EscalationPref::ALL
                .iter()
                .position(|p| *p == config.escalation)
                .unwrap_or(0) as u32,
        )
        .build();
    let status_row = adw::ActionRow::builder()
        .title("Current method")
        .subtitle(&shell.engine.escalation_description())
        .build();
    let forget_button = widgets::text_button(
        "Forget",
        "Drop the cached administrator password",
        &["pill"],
    );
    status_row.add_suffix(&forget_button);
    let rights_group = adw::PreferencesGroup::builder()
        .title("Administrator rights")
        .description("Mount Manager only escalates for actions that really need root, and asks once per session.")
        .build();
    rights_group.add(&escalation_row);
    rights_group.add(&status_row);

    // ── passwords ─────────────────────────────────────────────────────────
    let remember_row = adw::SwitchRow::builder()
        .title("Offer to remember share passwords")
        .subtitle(shell.engine.password_backend().label())
        .active(config.remember_passwords)
        .build();
    let keyring_row = adw::ActionRow::builder()
        .title("Password storage")
        .subtitle(shell.engine.password_backend().label())
        .build();
    let password_group = adw::PreferencesGroup::builder().title("Passwords").build();
    password_group.add(&remember_row);
    password_group.add(&keyring_row);

    // ── view ──────────────────────────────────────────────────────────────
    let system_row = adw::SwitchRow::builder()
        .title("Show system mounts")
        .subtitle("proc, sysfs, tmpfs, container bind mounts, …")
        .active(config.show_system_mounts)
        .build();
    let devices_row = adw::SwitchRow::builder()
        .title("Show every block device")
        .subtitle("Including partitions without a filesystem")
        .active(config.show_all_devices)
        .build();
    let refresh_row = adw::SwitchRow::builder()
        .title("Refresh automatically")
        .subtitle(&format!(
            "Re-read the mount table every {} seconds",
            config.refresh_secs
        ))
        .active(config.refresh_secs > 0)
        .build();
    let view_group = adw::PreferencesGroup::builder().title("View").build();
    view_group.add(&system_row);
    view_group.add(&devices_row);
    view_group.add(&refresh_row);

    // ── data & tools ──────────────────────────────────────────────────────
    let config_row = adw::ActionRow::builder()
        .title("Configuration file")
        .subtitle(&AppConfig::path().to_string_lossy().to_string())
        .build();
    config_row.add_suffix(&widgets::icon_button(
        "folder-open-symbolic",
        "Open the folder",
        &[],
    ));
    let restore_row = adw::ActionRow::builder()
        .title("Restore /etc/fstab backup")
        .subtitle("Undo the last change Mount Manager made to /etc/fstab")
        .build();
    let restore_button = widgets::text_button("Restore", "Needs administrator rights", &["pill"]);
    restore_row.add_suffix(&restore_button);
    let helper_row = adw::ActionRow::builder()
        .title("Privileged helper")
        .subtitle(&shell.engine.helper_path().to_string_lossy().to_string())
        .build();
    let data_group = adw::PreferencesGroup::builder().title("Data").build();
    data_group.add(&config_row);
    data_group.add(&restore_row);
    data_group.add(&helper_row);

    // ── about ─────────────────────────────────────────────────────────────
    let about_row = adw::ActionRow::builder()
        .title("About Mount Manager")
        .subtitle(crate::about_line())
        .build();
    let about_button = widgets::text_button("About", "Version, licence and credits", &["pill"]);
    about_row.add_suffix(&about_button);
    let about_group = adw::PreferencesGroup::builder().build();
    about_group.add(&about_row);

    let page = adw::PreferencesPage::builder().build();
    page.add(&rights_group);
    page.add(&password_group);
    page.add(&view_group);
    page.add(&data_group);
    page.add(&about_group);

    let toolbar = adw::ToolbarView::builder().build();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&widgets::scroller(&page)));
    window.set_content(Some(&toolbar));

    // ── wiring ────────────────────────────────────────────────────────────
    let shell_for_actions = Rc::new(shell.clone());

    escalation_row.connect_selected_notify(glib::clone!(
        #[strong(rename_to = s)]
        shell_for_actions,
        move |row| {
            let pref = EscalationPref::ALL[row.selected() as usize];
            let engine = s.engine.clone();
            let bus = s.bus.clone();
            std::thread::spawn(move || {
                let _ = engine.update_config(|cfg| cfg.escalation = pref);
                bus.send(Msg::Config(Box::new(engine.config())));
                bus.send(Msg::Ping);
            });
        },
    ));

    // Read the value on the UI thread: the closure handed to `save_pref` runs
    // on a worker thread and must not capture widgets.
    remember_row.connect_active_notify(glib::clone!(
        #[strong(rename_to = s)]
        shell_for_actions,
        move |row| {
            let on = row.is_active();
            save_pref(&s, move |cfg| cfg.remember_passwords = on);
        },
    ));
    system_row.connect_active_notify(glib::clone!(
        #[strong(rename_to = s)]
        shell_for_actions,
        move |row| {
            let on = row.is_active();
            save_pref(&s, move |cfg| cfg.show_system_mounts = on);
        },
    ));
    devices_row.connect_active_notify(glib::clone!(
        #[strong(rename_to = s)]
        shell_for_actions,
        move |row| {
            let on = row.is_active();
            save_pref(&s, move |cfg| cfg.show_all_devices = on);
        },
    ));
    refresh_row.connect_active_notify(glib::clone!(
        #[strong(rename_to = s)]
        shell_for_actions,
        move |row| {
            let on = row.is_active();
            save_pref(&s, move |cfg| cfg.refresh_secs = if on { 15 } else { 0 });
        },
    ));

    forget_button.connect_clicked(glib::clone!(
        #[strong(rename_to = s)]
        shell_for_actions,
        move |_| {
            s.engine.forget_admin_password();
            s.toast("Administrator password forgotten");
        },
    ));

    restore_button.connect_clicked(glib::clone!(
        #[strong(rename_to = s)]
        shell_for_actions,
        move |_| {
            s.busy(true);
            let engine = s.engine.clone();
            let bus = s.bus.clone();
            std::thread::spawn(move || {
                let result = engine.restore_fstab_backup();
                bus.done("Restore /etc/fstab", result);
                bus.send(Msg::Ping);
            });
        },
    ));

    let config_dir = AppConfig::dir();
    config_row.connect_activated(glib::clone!(
        #[strong(rename_to = s)]
        shell_for_actions,
        move |_| {
            let engine = s.engine.clone();
            let bus = s.bus.clone();
            let dir = config_dir.to_string_lossy().to_string();
            std::thread::spawn(move || {
                if let Err(e) = engine.open(&dir) {
                    bus.error("Could not open the configuration folder", e);
                }
            });
        },
    ));

    about_button.connect_clicked(glib::clone!(
        #[strong(rename_to = s)]
        shell_for_actions,
        move |_| {
            about(&s);
        },
    ));

    let close_window = window.clone();
    close.connect_clicked(move |_| close_window.close());

    window.set_transient_for(Some(&shell.window));
    window.present();
}

fn save_pref(shell: &Shell, f: impl FnOnce(&mut AppConfig) + Send + 'static) {
    let engine = shell.engine.clone();
    let bus = shell.bus.clone();
    std::thread::spawn(move || {
        let _ = engine.update_config(f);
        bus.send(Msg::Config(Box::new(engine.config())));
        bus.send(Msg::Ping);
    });
}

/// The About window.
pub fn about(shell: &Shell) {
    let about = adw::AboutWindow::builder()
        .application_name(crate::APP_NAME)
        .application_icon("mount-manager")
        .version(crate::VERSION)
        .developer_name("Mount Manager contributors")
        .license_type(gtk::License::MitX11)
        .comments("Mount and manage SMB, NFS, SSHFS, WebDAV, FTP shares and external drives.")
        .website("https://github.com/o-oJames/docker_gnome/tree/main/mount-manager")
        .issue_url("https://github.com/o-oJames/docker_gnome/issues")
        .copyright("© 2026 Mount Manager contributors")
        .developers(vec!["Mount Manager contributors"])
        .build();
    about.set_transient_for(Some(&shell.window));
    about.present();
}

//! Main window: header bar, the four pages, the message pump and app actions.

use crate::error::Result;
use crate::model::{BlockDevice, LogEntry, MountEntry, SavedShare, ShareRequest};
use crate::ui::activity_page::ActivityPage;
use crate::ui::bus::{drain, Bus, Msg};
use crate::ui::devices_page::DevicesPage;
use crate::ui::mounts_page::{self, MountsPage};
use crate::ui::password_dialog;
use crate::ui::preferences_dialog;
use crate::ui::share_dialog;
use crate::ui::shares_page::SharesPage;
use crate::ui::shell::{self, Shell};
use crate::ui::widgets;
use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// The assembled window. Kept in an `Rc` for the message pump.
struct Window {
    shell: Shell,
    mounts: MountsPage,
    #[allow(dead_code)]
    devices: DevicesPage,
    #[allow(dead_code)]
    shares: SharesPage,
    #[allow(dead_code)]
    activity: ActivityPage,
}

/// Build and present the main window.
pub fn build(app: &adw::Application, prefilled: Vec<ShareRequest>) {
    let (bus, rx) = Bus::channel();
    let engine = shell::engine(&bus);
    let shell = Shell::new(app, bus.clone(), engine);
    let content = shell.content();

    // ── pages ─────────────────────────────────────────────────────────────
    let mounts = MountsPage::new(shell.clone());
    let devices = DevicesPage::new(shell.clone());
    let shares = SharesPage::new(shell.clone());
    let activity = ActivityPage::new(shell.clone());

    let stack = adw::ViewStack::builder().vexpand(true).build();
    add_page(
        &stack,
        &mounts.widget(),
        "mounts",
        "Mounts",
        "drive-harddisk-system-symbolic",
    );
    add_page(
        &stack,
        &devices.widget(),
        "devices",
        "Devices",
        "drive-removable-media-symbolic",
    );
    add_page(
        &stack,
        &shares.widget(),
        "shares",
        "Shares",
        "folder-remote-symbolic",
    );
    add_page(
        &stack,
        &activity.widget(),
        "activity",
        "Activity",
        "utilities-system-monitor-symbolic",
    );

    // ── header bar ────────────────────────────────────────────────────────
    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();
    let header = adw::HeaderBar::builder().title_widget(&switcher).build();

    let add_button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add a share (Ctrl+N)")
        .css_classes(vec!["suggested-action".to_string()])
        .build();
    let refresh_button = widgets::icon_button("view-refresh-symbolic", "Refresh (Ctrl+R)", &[]);
    header.pack_start(&add_button);
    header.pack_start(&refresh_button);
    header.pack_end(&shell.spinner);

    let menu = gio::Menu::new();
    menu.append(Some("Add Share…"), Some("app.add-share"));
    menu.append(Some("Refresh"), Some("app.refresh"));
    menu.append(Some("Preferences"), Some("app.preferences"));
    menu.append(
        Some("Forget administrator password"),
        Some("app.forget-password"),
    );
    menu.append(Some("About Mount Manager"), Some("app.about"));
    menu.append(Some("Quit"), Some("app.quit"));
    let menu_button = gtk::MenuButton::builder()
        .menu_model(&menu)
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main menu")
        .build();
    header.pack_end(&menu_button);

    content.prepend(&header);
    content.append(&stack);

    // Banner details.
    let banner_shell = shell.clone();
    shell
        .banner
        .connect_button_clicked(move |_| match banner_shell.banner_detail() {
            Some(detail) => {
                crate::ui::alert::info(&banner_shell, "What went wrong", &detail, None);
            }
            None => banner_shell.set_banner("", None),
        });

    // ── actions ───────────────────────────────────────────────────────────
    let action_shell = shell.clone();
    let add_action = gio::SimpleAction::new("add-share", None);
    add_action.connect_activate(move |_, _| share_dialog::open(&action_shell, None));
    app.add_action(&add_action);

    let action_shell = shell.clone();
    let refresh_action = gio::SimpleAction::new("refresh", None);
    refresh_action.connect_activate(move |_, _| refresh_all(&action_shell));
    app.add_action(&refresh_action);

    let action_shell = shell.clone();
    let prefs_action = gio::SimpleAction::new("preferences", None);
    prefs_action.connect_activate(move |_, _| preferences_dialog::open(&action_shell));
    app.add_action(&prefs_action);

    let action_shell = shell.clone();
    let forget_action = gio::SimpleAction::new("forget-password", None);
    forget_action.connect_activate(move |_, _| {
        action_shell.engine.forget_admin_password();
        action_shell.toast("Administrator password forgotten");
    });
    app.add_action(&forget_action);

    let action_shell = shell.clone();
    let about_action = gio::SimpleAction::new("about", None);
    about_action.connect_activate(move |_, _| preferences_dialog::about(&action_shell));
    app.add_action(&about_action);

    let quit_action = gio::SimpleAction::new("quit", None);
    let app_for_quit = app.clone();
    quit_action.connect_activate(move |_, _| app_for_quit.quit());
    app.add_action(&quit_action);

    app.set_accels_for_action("app.add-share", &["<Control>n"]);
    app.set_accels_for_action("app.refresh", &["<Control>r"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);

    // ── buttons ───────────────────────────────────────────────────────────
    let add_shell = shell.clone();
    add_button.connect_clicked(move |_| share_dialog::open(&add_shell, None));
    let refresh_shell = shell.clone();
    refresh_button.connect_clicked(move |_| refresh_all(&refresh_shell));

    // ── window ────────────────────────────────────────────────────────────
    let window = Rc::new(Window {
        shell: shell.clone(),
        mounts,
        devices,
        shares,
        activity,
    });
    let closed = Rc::new(Cell::new(false));
    let closed_for_signal = Rc::clone(&closed);
    shell.window.connect_close_request(move |_| {
        closed_for_signal.set(true);
        glib::Propagation::Proceed
    });

    // Sync the page-level toggle with the stored preference.
    let cfg = shell.engine.config();
    window
        .mounts
        .system_toggle()
        .set_active(cfg.show_system_mounts);

    // Message pump: never blocks the UI thread.
    let pump_window = Rc::clone(&window);
    let pump_closed = Rc::clone(&closed);
    glib::timeout_add_local(Duration::from_millis(110), move || {
        for msg in drain(&rx, Duration::ZERO) {
            handle(&pump_window, msg);
        }
        if pump_closed.get() {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });

    // Automatic refresh (preference driven, cheap: it only reads /proc).
    let auto_shell = shell.clone();
    let ticks = Rc::new(Cell::new(0u32));
    let auto_closed = Rc::clone(&closed);
    glib::timeout_add_local(Duration::from_secs(1), move || {
        if auto_closed.get() {
            return glib::ControlFlow::Break;
        }
        let interval = auto_shell.engine.config().refresh_secs;
        if interval == 0 {
            return glib::ControlFlow::Continue;
        }
        let next = ticks.get() + 1;
        if next >= interval {
            ticks.set(0);
            refresh_all(&auto_shell);
        } else {
            ticks.set(next);
        }
        glib::ControlFlow::Continue
    });

    shell.window.present();
    refresh_all(&shell);

    // A URI handed over by the desktop opens the share dialog right away.
    for request in prefilled {
        let shell_for_dialog = shell.clone();
        glib::idle_add_local(move || {
            share_dialog::open(&shell_for_dialog, Some(request.clone()));
            glib::ControlFlow::Break
        });
    }
}

fn add_page(stack: &adw::ViewStack, child: &gtk::Box, name: &str, title: &str, icon: &str) {
    let page = stack.add_titled(child, Some(name), title);
    page.set_icon_name(Some(icon));
}

/// Kick off the background refresh of every page.
pub fn refresh_all(shell: &Shell) {
    // Mounts (and the fstab state they need).
    let engine = shell.engine.clone();
    let bus = shell.bus.clone();
    std::thread::spawn(move || {
        let result = engine.visible_mounts();
        bus.send(Msg::Mounts(result));
    });

    // Block devices.
    let engine = shell.engine.clone();
    let bus = shell.bus.clone();
    std::thread::spawn(move || {
        let result = engine.devices();
        bus.send(Msg::Devices(result));
    });

    // Saved shares + their mount state.
    let engine = shell.engine.clone();
    let bus = shell.bus.clone();
    std::thread::spawn(move || {
        let shares = engine.share_status();
        bus.send(Msg::Shares(shares));
    });

    // Activity log.
    let engine = shell.engine.clone();
    let bus = shell.bus.clone();
    std::thread::spawn(move || {
        let logs = engine.logs();
        bus.send(Msg::Logs(logs));
    });
}

/// Handle one message from a worker thread on the UI thread.
fn handle(window: &Rc<Window>, msg: Msg) {
    let shell = &window.shell;
    match msg {
        Msg::Mounts(result) => match result {
            Ok(entries) => window.mounts.render(&entries),
            Err(e) => shell.show_error("Could not read the mount table", &e),
        },
        Msg::Devices(result) => match result {
            Ok(devices) => render_devices(window, &devices),
            Err(e) => shell.show_error("Could not list block devices", &e),
        },
        Msg::Shares(shares) => render_shares(window, &shares),
        Msg::Logs(logs) => render_logs(window, &logs),
        Msg::Config(config) => {
            window
                .mounts
                .system_toggle()
                .set_active(config.show_system_mounts);
        }
        Msg::Done { action, result } => {
            shell.busy(false);
            match result {
                Ok(message) => {
                    shell.set_banner("", None);
                    shell.toast(&message);
                }
                Err(e) => {
                    shell.set_banner(&format!("{action} failed: {}", e.message), e.detail.clone());
                    shell.show_error(&format!("{action} failed"), &e);
                }
            }
            refresh_all(shell);
        }
        Msg::Test(result) => {
            shell.busy(false);
            match result {
                Ok(report) => {
                    shell.toast("Connection test finished");
                    crate::ui::alert::info(shell, "Connection test", &report, None);
                }
                Err(e) => shell.show_error("Connection test failed", &e),
            }
            push_logs(window);
        }
        Msg::LazyOffer { target, detail } => {
            shell.busy(false);
            mounts_page::offer_lazy_unmount(shell, &target, &detail);
        }
        Msg::AskPassword { context, reply } => {
            shell.busy(false);
            password_dialog::ask(shell, context, reply);
        }
        Msg::Toast(text) => shell.toast(&text),
        Msg::Error { title, error } => shell.show_error(&title, &error),
        Msg::Busy(busy) => shell.busy(busy),
        Msg::OpenShareDialog(request) => {
            share_dialog::open(shell, request.map(|r| *r));
        }
        Msg::Open(path) => {
            let engine = shell.engine.clone();
            let bus = shell.bus.clone();
            std::thread::spawn(move || {
                if let Err(e) = engine.open(&path) {
                    bus.error("Could not open the folder", e);
                }
            });
        }
        Msg::Discovered { .. } => {
            // Handled inside the browse dialog; nothing to do here.
        }
        Msg::Ping => {
            refresh_all(shell);
            push_logs(window);
        }
    }
}

fn render_devices(window: &Window, devices: &[BlockDevice]) {
    window.devices.render(devices);
}

fn render_shares(window: &Window, shares: &[(SavedShare, Option<MountEntry>)]) {
    window.shares.render(shares);
}

fn render_logs(window: &Window, logs: &[LogEntry]) {
    window.activity.render(logs);
}

/// Pull the latest engine log into the activity page.
fn push_logs(window: &Window) {
    let logs = window.shell.engine.logs();
    window.activity.render(&logs);
}

/// Helper used by tests to keep type inference honest.
#[allow(dead_code)]
fn assert_send_types() {
    fn is_send<T: Send>() {}
    is_send::<Result<String>>();
}

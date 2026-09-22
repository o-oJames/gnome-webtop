//! Command line interface.
//!
//! The GUI is the primary front-end, but every action is also reachable from a
//! terminal — useful for scripts, for `mount-manager auto-mount` at login, and
//! for debugging inside containers where no display is available.
//!
//! ```text
//! mount-manager list --all
//! mount-manager mount smb://nas/data /mnt/nas --user jo --persist
//! mount-manager umount /mnt/nas
//! ```

use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::model::*;
use crate::platform;
use crate::privilege::{Credentials, PromptContext, Prompter};
use crate::{devices, exec, fstab, mounts};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;

/// Flags that take a value.
const VALUE_FLAGS: &[&str] = &[
    "--protocol",
    "-t",
    "--host",
    "--path",
    "--target",
    "--user",
    "-u",
    "--password",
    "-p",
    "--domain",
    "--port",
    "--method",
    "--option",
    "-o",
    "--device",
    "--fstype",
    "--source",
    "--name",
    "--id",
    "--identity",
    "--escalate",
];

/// Run a CLI invocation and return the exit code.
pub fn run(args: &[String]) -> i32 {
    match dispatch(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("mount-manager: {}", e.message);
            if let Some(detail) = e.detail {
                if !detail.trim().is_empty() {
                    eprintln!("\n{}", detail.trim());
                }
            }
            1
        }
    }
}

fn dispatch(args: &[String]) -> Result<i32> {
    let parsed = parse(args);
    let command = parsed
        .positional
        .first()
        .map(|s| s.as_str())
        .unwrap_or("help");

    if parsed.has("--help") || parsed.has("-h") {
        print!("{}", usage());
        return Ok(0);
    }
    if parsed.has("--version") || parsed.has("-V") {
        println!("{}", crate::about_line());
        return Ok(0);
    }

    match command {
        "help" | "--help" | "-h" => {
            print!("{}", usage());
            Ok(0)
        }
        "version" => {
            println!("{}", crate::about_line());
            Ok(0)
        }
        "info" => cmd_info(),
        "list" | "ls" | "mounts" => cmd_list(&parsed),
        "devices" | "dev" => cmd_devices(&parsed),
        "shares" => cmd_shares(&parsed),
        "fstab" => cmd_fstab(&parsed),
        "discover" => cmd_discover(&parsed),
        "mount" => cmd_mount(&parsed, true),
        "add" => cmd_mount(&parsed, false),
        "test" => cmd_test(&parsed),
        "umount" | "unmount" => cmd_umount(&parsed),
        "mount-device" => cmd_mount_device(&parsed),
        "remove" | "rm" => cmd_remove(&parsed),
        "auto-mount" | "automount" => cmd_auto_mount(&parsed),
        "open" => cmd_open(&parsed),
        "mkdir" => cmd_mkdir(&parsed),
        other => {
            Err(Error::new(format!("Unknown command `{other}`")).hint("Run `mount-manager help`."))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// argument parsing
// ─────────────────────────────────────────────────────────────────────────────

struct Parsed {
    positional: Vec<String>,
    values: HashMap<String, Vec<String>>,
    bools: HashSet<String>,
}

impl Parsed {
    fn has(&self, flag: &str) -> bool {
        self.bools.contains(flag)
    }

    /// Value of a flag, accepting aliases.
    fn value(&self, flags: &[&str]) -> Option<String> {
        for f in flags {
            if let Some(v) = self.values.get(*f).and_then(|v| v.first()) {
                return Some(v.clone());
            }
            if self.bools.contains(*f) {
                return Some(String::new());
            }
        }
        None
    }

    fn values_of(&self, flag: &str) -> Vec<String> {
        self.values.get(flag).cloned().unwrap_or_default()
    }

    fn positional_from(&self, index: usize) -> Vec<String> {
        self.positional.iter().skip(index).cloned().collect()
    }
}

fn parse(args: &[String]) -> Parsed {
    let mut out = Parsed {
        positional: Vec::new(),
        values: HashMap::new(),
        bools: HashSet::new(),
    };
    let mut iter = args.iter();
    let mut double_dash = false;
    while let Some(arg) = iter.next() {
        if double_dash {
            out.positional.push(arg.clone());
            continue;
        }
        if arg == "--" {
            double_dash = true;
            continue;
        }
        if arg.starts_with('-') && arg.len() > 1 {
            // Support `--flag=value` as well as `--flag value`.
            if let Some((flag, value)) = arg.split_once('=') {
                out.values
                    .entry(flag.to_string())
                    .or_default()
                    .push(value.to_string());
                continue;
            }
            let flag = arg.as_str();
            if VALUE_FLAGS.contains(&flag) {
                let value = iter.next().cloned().unwrap_or_default();
                out.values.entry(flag.to_string()).or_default().push(value);
            } else {
                out.bools.insert(flag.to_string());
            }
            continue;
        }
        out.positional.push(arg.clone());
    }
    out
}

/// Build a [`ShareRequest`] from CLI flags (and optional URI/positional args).
fn request_from(parsed: &Parsed, positional_offset: usize) -> Result<ShareRequest> {
    let positional = parsed.positional_from(positional_offset);
    let mut req = ShareRequest::default();

    // `mount smb://nas/data /mnt/nas`
    if let Some(first) = positional.first() {
        if first.contains("://") {
            req = parse_uri(first)?;
        } else {
            req.host = first.clone();
        }
    }
    if let Some(second) = positional.get(1) {
        if second.starts_with('/') {
            req.mount_point = second.clone();
        } else if req.remote_path.is_empty() {
            req.remote_path = second.clone();
        }
    }

    if let Some(p) = parsed.value(&["--protocol", "-t"]) {
        req.protocol = protocol_from_str(&p)?;
    }
    if let Some(v) = parsed.value(&["--host"]) {
        if v.contains('/') && !req.protocol.is_network() {
            // --host /dev/sdb1 is accepted for block devices
            req.local_source = v;
        } else {
            req.host = v;
        }
    }
    if let Some(v) = parsed.value(&["--path"]) {
        if req.protocol == Protocol::Bind || req.protocol == Protocol::Block {
            req.local_source = v;
        } else {
            req.remote_path = v;
        }
    }
    if let Some(v) = parsed.value(&["--device"]) {
        req.protocol = Protocol::Block;
        req.local_source = v;
    }
    if let Some(v) = parsed.value(&["--source"]) {
        req.protocol = Protocol::Custom;
        req.custom_source = v;
    }
    if let Some(v) = parsed.value(&["--fstype"]) {
        req.custom_fstype = v;
    }
    if let Some(v) = parsed.value(&["--target"]) {
        req.mount_point = v;
    }
    if let Some(v) = parsed.value(&["--user", "-u"]) {
        req.username = v;
    }
    if let Some(v) = parsed.value(&["--password", "-p"]) {
        req.password = read_password_arg(&v)?;
    }
    if let Some(v) = parsed.value(&["--domain"]) {
        req.domain = v;
    }
    if let Some(v) = parsed.value(&["--port"]) {
        req.port = Some(
            v.parse::<u16>()
                .map_err(|_| Error::new(format!("`{v}` is not a valid port")))?,
        );
    }
    if let Some(v) = parsed.value(&["--method"]) {
        req.method = match v.to_ascii_lowercase().as_str() {
            "auto" => MountMethod::Auto,
            "system" | "root" => MountMethod::System,
            "gvfs" | "user" | "session" => MountMethod::Gvfs,
            "fuse" | "sshfs" => MountMethod::FuseUser,
            other => {
                return Err(Error::new(format!("Unknown mount method `{other}`"))
                    .hint("auto | system | gvfs | fuse"))
            }
        };
    }
    if let Some(v) = parsed.value(&["--name"]) {
        req.name = v;
    }
    if let Some(v) = parsed.value(&["--identity"]) {
        req.identity_file = v;
    }
    for opt in parsed.values_of("--option") {
        req.extra_options.extend(
            opt.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    for opt in parsed.values_of("-o") {
        req.extra_options.extend(
            opt.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    req.read_only = parsed.has("--ro") || parsed.has("--read-only");
    req.persist = parsed.has("--persist") || parsed.has("--fstab");
    req.bookmark = parsed.has("--bookmark");
    req.remember_password = parsed.has("--remember-password");
    req.take_ownership = parsed.has("--take-ownership");
    Ok(req)
}

/// `--password -` reads the password from stdin instead of argv (scripts).
fn read_password_arg(value: &str) -> Result<String> {
    if value == "-" || value.is_empty() {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| {
            Error::with_detail("Cannot read the password from stdin", e.to_string())
        })?;
        let line = line.trim_end_matches(['\n', '\r']).to_string();
        if line.is_empty() {
            return Err(Error::new("No password was provided"));
        }
        return Ok(line);
    }
    Ok(value.to_string())
}

/// Parse a share URI (`smb://user@nas/data`) into a request.
pub fn parse_uri(uri: &str) -> Result<ShareRequest> {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return Err(Error::new(format!("`{uri}` is not a URI")));
    };
    let protocol = match scheme.to_ascii_lowercase().as_str() {
        "smb" | "cifs" => Protocol::Cifs,
        "nfs" | "nfs4" => Protocol::Nfs,
        "sftp" | "ssh" | "sshfs" => Protocol::Sshfs,
        "dav" => Protocol::WebDav,
        "davs" => Protocol::WebDav,
        "webdav" => Protocol::WebDav,
        "ftp" | "ftps" => Protocol::Ftp,
        "file" => Protocol::Bind,
        other => return Err(Error::new(format!("Unsupported URI scheme `{other}`"))),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(i) => (&authority[..i], &authority[i + 1..]),
        None => ("", authority),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.parse::<u16>().is_ok() => {
            (h.to_string(), Some(p.parse::<u16>().unwrap()))
        }
        _ => (hostport.to_string(), None),
    };
    let (username, password) = match userinfo.split_once(':') {
        Some((u, p)) => (u.to_string(), p.to_string()),
        None => (userinfo.to_string(), String::new()),
    };
    Ok(ShareRequest {
        protocol,
        host,
        remote_path: path.clone(),
        username: percent_decode(&username),
        password: percent_decode(&password),
        port,
        local_source: if protocol == Protocol::Bind {
            percent_decode(&format!("{authority}{}", path))
        } else {
            String::new()
        },
        ..Default::default()
    })
}

fn percent_decode(value: &str) -> String {
    mounts::uri_unescape(value)
}

/// Parse a protocol name.
pub fn protocol_from_str(value: &str) -> Result<Protocol> {
    Ok(match value.to_ascii_lowercase().as_str() {
        "smb" | "cifs" | "samba" | "windows" => Protocol::Cifs,
        "nfs" | "nfs4" => Protocol::Nfs,
        "sshfs" | "sftp" | "ssh" => Protocol::Sshfs,
        "webdav" | "dav" | "davs" | "http" => Protocol::WebDav,
        "ftp" | "ftps" | "curlftpfs" => Protocol::Ftp,
        "block" | "device" | "usb" | "disk" => Protocol::Block,
        "bind" => Protocol::Bind,
        "custom" | "auto" | "other" => Protocol::Custom,
        other => {
            return Err(Error::new(format!("Unknown protocol `{other}`"))
                .hint("smb | nfs | sshfs | webdav | ftp | block | bind | custom"))
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// commands
// ─────────────────────────────────────────────────────────────────────────────

fn engine(parsed: &Parsed) -> Arc<Engine> {
    let escalation =
        parsed
            .value(&["--escalate"])
            .and_then(|v| match v.to_ascii_lowercase().as_str() {
                "auto" => Some(crate::config::EscalationPref::Auto),
                "pkexec" | "polkit" => Some(crate::config::EscalationPref::Pkexec),
                "sudo" => Some(crate::config::EscalationPref::Sudo),
                "never" | "none" => Some(crate::config::EscalationPref::Never),
                _ => None,
            });
    let e = Engine::new(Arc::new(CliPrompter));
    if let Some(pref) = escalation {
        let _ = e.update_config(|cfg| cfg.escalation = pref);
    }
    e
}

fn cmd_info() -> Result<i32> {
    let e = Engine::new(Arc::new(CliPrompter));
    println!("{} {}", crate::APP_NAME, crate::VERSION);
    println!("  helper         : {}", e.helper_path().display());
    println!(
        "  escalation     : {}",
        e.escalation_description()
            .replace('\n', "\n                   ")
    );
    println!(
        "  config         : {}",
        crate::config::AppConfig::path().display()
    );
    println!(
        "  user           : {} (uid {}, gid {})",
        e.caller().name,
        e.caller().uid,
        e.caller().gid
    );
    println!(
        "  default target : {}",
        platform::default_mount_root().display()
    );
    println!("  password store : {}", e.password_backend().label());
    println!("  tools          :");
    for tool in [
        "mount",
        "umount",
        "lsblk",
        "gio",
        "smbclient",
        "showmount",
        "sshfs",
        "mount.cifs",
        "mount.nfs",
        "mount.davfs",
        "curlftpfs",
        "pkexec",
        "sudo",
        "secret-tool",
        "avahi-browse",
    ] {
        let present = exec::exists_any(tool);
        println!(
            "    {:<14} {}",
            tool,
            if present { "yes" } else { "MISSING" }
        );
    }
    println!("  saved shares   : {}", e.shares().len());
    Ok(0)
}

fn cmd_list(parsed: &Parsed) -> Result<i32> {
    let e = Engine::new(Arc::new(CliPrompter));
    let all = parsed.has("--all") || parsed.has("-a");
    let list = if all {
        e.all_mounts()?
    } else {
        e.visible_mounts()?
    };
    if parsed.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).unwrap_or_default()
        );
        return Ok(0);
    }
    if list.is_empty() {
        println!(
            "No mounts to show{}.",
            if all { "" } else { " (try --all)" }
        );
        return Ok(0);
    }
    println!(
        "{:<44} {:<28} {:<12} {:<10} {:<9} {}",
        "SOURCE", "TARGET", "TYPE", "KIND", "FREE", "FLAGS"
    );
    for m in &list {
        println!(
            "{:<44} {:<28} {:<12} {:<10} {:<9} {}",
            truncate(&m.source, 44),
            truncate(&m.target, 28),
            truncate(&m.fstype, 12),
            m.kind.label(),
            m.avail
                .map(platform::human_size)
                .unwrap_or_else(|| "-".to_string()),
            if m.writable { "" } else { "ro" },
        );
    }
    Ok(0)
}

fn cmd_devices(parsed: &Parsed) -> Result<i32> {
    let e = Engine::new(Arc::new(CliPrompter));
    let list = e.devices()?;
    if parsed.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).unwrap_or_default()
        );
        return Ok(0);
    }
    if list.is_empty() {
        println!("No block devices found.");
        return Ok(0);
    }
    println!(
        "{:<16} {:<18} {:<10} {:<10} {:<28} {}",
        "DEVICE", "LABEL", "FSTYPE", "SIZE", "MOUNTED AT", "FLAGS"
    );
    for d in &list {
        println!(
            "{:<16} {:<18} {:<10} {:<10} {:<28} {}",
            truncate(&d.path, 16),
            truncate(&d.label, 18),
            truncate(&d.fstype, 10),
            if d.size > 0 {
                platform::human_size(d.size)
            } else {
                "-".to_string()
            },
            truncate(&d.mountpoints.join(","), 28),
            if d.removable { "removable" } else { "" },
        );
    }
    Ok(0)
}

fn cmd_shares(parsed: &Parsed) -> Result<i32> {
    let e = Engine::new(Arc::new(CliPrompter));
    let shares = e.share_status();
    if parsed.has("--json") {
        let json = shares
            .iter()
            .map(|(s, m)| {
                serde_json::json!({
                    "id": s.id,
                    "name": s.request.display_name(),
                    "protocol": s.request.protocol.short(),
                    "source": s.request.display_source(),
                    "target": s.request.mount_point,
                    "method": s.request.method.short(),
                    "persist": s.request.persist,
                    "auto_mount": s.auto_mount,
                    "mounted": m.is_some(),
                    "last_used": s.last_used,
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
        return Ok(0);
    }
    if shares.is_empty() {
        println!("No saved shares. Add one with:\n  mount-manager add --protocol smb --host nas --path data --target /mnt/nas --save");
        return Ok(0);
    }
    println!(
        "{:<12} {:<20} {:<8} {:<34} {:<24} {}",
        "ID", "NAME", "TYPE", "SOURCE", "TARGET", "STATE"
    );
    for (s, mounted) in &shares {
        println!(
            "{:<12} {:<20} {:<8} {:<34} {:<24} {}",
            truncate(&s.id, 12),
            truncate(&s.request.display_name(), 20),
            s.request.protocol.short(),
            truncate(&s.request.display_source(), 34),
            truncate(&s.request.mount_point, 24),
            match mounted {
                Some(_) => "mounted",
                None if s.request.persist => "saved (fstab)",
                None => "saved",
            },
        );
    }
    Ok(0)
}

fn cmd_mount(parsed: &Parsed, do_mount: bool) -> Result<i32> {
    let e = engine(parsed);
    let req = request_from(parsed, 1)?;
    if req.host.is_empty() && req.local_source.is_empty() && req.custom_source.is_empty() {
        return Err(Error::new("Nothing to mount — give a source").hint(
            "mount-manager mount smb://nas/data /mnt/nas --user jo\nmount-manager mount --device /dev/sdb1",
        ));
    }
    let save = parsed.has("--save") || parsed.has("--remember-password") || !do_mount;
    if do_mount {
        let message = e.mount(req, save)?;
        println!("{message}");
    } else {
        let id = e.save_share(&req)?;
        println!("Saved share {id}");
    }
    Ok(0)
}

fn cmd_test(parsed: &Parsed) -> Result<i32> {
    let e = Engine::new(Arc::new(CliPrompter));
    let req = request_from(parsed, 1)?;
    let report = e.test_connection(req)?;
    println!("{report}");
    Ok(0)
}

fn cmd_umount(parsed: &Parsed) -> Result<i32> {
    let e = engine(parsed);
    let targets = parsed.positional_from(1);
    if targets.is_empty() {
        return Err(Error::new("Nothing to unmount — give a mount point")
            .hint("mount-manager umount /mnt/nas"));
    }
    let lazy = parsed.has("--lazy") || parsed.has("-l");
    let live = e.all_mounts()?;
    let mut failures = 0;
    for target in targets {
        match mounts::find_by_target(&live, &target) {
            Some(entry) => match e.unmount(&entry, lazy) {
                Ok(msg) => println!("{msg}"),
                Err(err) => {
                    failures += 1;
                    eprintln!("mount-manager: {}", err.message);
                    if let Some(d) = err.detail {
                        eprintln!("{}", d);
                    }
                }
            },
            None => {
                failures += 1;
                eprintln!("mount-manager: `{target}` is not mounted");
            }
        }
    }
    Ok(if failures > 0 { 1 } else { 0 })
}

fn cmd_mount_device(parsed: &Parsed) -> Result<i32> {
    let e = engine(parsed);
    let path = parsed
        .positional
        .get(1)
        .or(parsed.value(&["--device"]).as_ref())
        .cloned()
        .ok_or_else(|| Error::new("Give a device, e.g. `mount-manager mount-device /dev/sdb1`"))?;
    let device = devices::find_by_path(&path)
        .ok_or_else(|| Error::new(format!("`{path}` is not a block device")))?;
    let target = parsed.value(&["--target"]);
    println!("{}", e.mount_device(&device, target)?);
    Ok(0)
}

fn cmd_remove(parsed: &Parsed) -> Result<i32> {
    let e = engine(parsed);
    let needle = parsed
        .positional
        .get(1)
        .cloned()
        .or_else(|| parsed.value(&["--id"]))
        .ok_or_else(|| Error::new("Give the id or name of a saved share"))?;
    let id = match e
        .shares()
        .iter()
        .find(|s| s.id == needle || s.request.display_name() == needle)
    {
        Some(s) => s.id.clone(),
        None => return Err(Error::new(format!("No saved share matches `{needle}`"))),
    };
    println!("Removed {}", e.delete_share(&id)?);
    Ok(0)
}

fn cmd_auto_mount(parsed: &Parsed) -> Result<i32> {
    let e = engine(parsed);
    let results = e.auto_mount();
    if results.is_empty() {
        println!("Nothing is configured to mount automatically.");
        return Ok(0);
    }
    let mut failures = 0;
    let quiet = parsed.has("--quiet");
    for (name, res) in results {
        match res {
            Ok(msg) => println!("{name}: {msg}"),
            Err(err) => {
                failures += 1;
                if !quiet {
                    eprintln!("{name}: {}", err.message);
                }
            }
        }
    }
    Ok(if failures > 0 { 1 } else { 0 })
}

fn cmd_discover(parsed: &Parsed) -> Result<i32> {
    let e = Engine::new(Arc::new(CliPrompter));
    let protocol = parsed
        .value(&["--protocol", "-t"])
        .map(|p| protocol_from_str(&p))
        .transpose()?
        .unwrap_or(Protocol::Cifs);
    let host = parsed.value(&["--host"]);

    let services = e.discover(protocol);
    if services.is_empty() {
        println!(
            "No {} servers were announced on the local network (mDNS).",
            protocol.short()
        );
    } else {
        println!("Servers ({}):", protocol.short());
        for s in &services {
            println!(
                "  {:<28} {:<24} {:<16} :{}",
                s.name, s.host, s.address, s.port
            );
        }
    }

    if let Some(host) = host {
        println!("\n{} offers:", host);
        match protocol {
            Protocol::Cifs => match e.smb_shares(
                &host,
                &parsed.value(&["--user", "-u"]).unwrap_or_default(),
                &parsed.value(&["--password", "-p"]).unwrap_or_default(),
            ) {
                Ok(shares) => {
                    for s in shares {
                        println!("  {:<24} {:<8} {}", s.name, s.kind, s.comment);
                    }
                }
                Err(err) => eprintln!("  {}", err.message),
            },
            Protocol::Nfs => match e.nfs_exports(&host) {
                Ok(exports) => {
                    for x in exports {
                        println!("  {:<24} {}", x.path, x.allowed);
                    }
                }
                Err(err) => eprintln!("  {}", err.message),
            },
            other => println!("  (no share listing implemented for {})", other.short()),
        }
    }
    Ok(0)
}

fn cmd_fstab(parsed: &Parsed) -> Result<i32> {
    let e = engine(parsed);
    if parsed.has("--restore") {
        println!("{}", e.restore_fstab_backup()?);
        return Ok(0);
    }
    if parsed.has("--verify") {
        match mounts::verify_fstab() {
            Some(report) => println!("{report}"),
            None => println!("findmnt is not available to verify /etc/fstab"),
        }
        return Ok(0);
    }
    let entries = fstab::read()?;
    if parsed.has("--json") {
        let json: Vec<_> = entries
            .iter()
            .map(|f| {
                serde_json::json!({
                    "source": f.source, "target": f.target, "fstype": f.fstype,
                    "options": f.options, "managed": f.marker.is_some(), "line": f.line,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
        return Ok(0);
    }
    if entries.is_empty() {
        println!("No entries in /etc/fstab.");
        return Ok(0);
    }
    println!(
        "{:<34} {:<24} {:<10} {:<34} {}",
        "SOURCE", "TARGET", "TYPE", "OPTIONS", "MANAGED"
    );
    for f in &entries {
        println!(
            "{:<34} {:<24} {:<10} {:<34} {}",
            truncate(&f.source, 34),
            truncate(&f.target, 24),
            truncate(&f.fstype, 10),
            truncate(&f.options.join(","), 34),
            if f.marker.is_some() {
                "mount-manager"
            } else {
                ""
            },
        );
    }
    Ok(0)
}

fn cmd_open(parsed: &Parsed) -> Result<i32> {
    let e = Engine::new(Arc::new(CliPrompter));
    let target = parsed
        .positional
        .get(1)
        .cloned()
        .ok_or_else(|| Error::new("Give a path to open"))?;
    e.open(&target)?;
    Ok(0)
}

fn cmd_mkdir(parsed: &Parsed) -> Result<i32> {
    let e = engine(parsed);
    let paths = parsed.positional_from(1);
    if paths.is_empty() {
        return Err(Error::new("Give one or more directories to create"));
    }
    for path in paths {
        e.ensure_mount_point(&path)?;
        println!("created {path}");
    }
    Ok(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────────────

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        value.to_string()
    } else {
        let cut: String = value.chars().take(width.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Read a password from the controlling terminal with echo disabled.
pub fn prompt_password_hidden(prompt: &str) -> Option<String> {
    use std::io::Read;
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let _ = write!(tty, "{prompt}");
    let _ = tty.flush();
    set_echo(&tty, false);
    let mut buf = [0u8; 1024];
    let n = tty.read(&mut buf).ok()?;
    set_echo(&tty, true);
    let _ = writeln!(tty);
    let line = String::from_utf8_lossy(&buf[..n])
        .trim_end_matches(['\n', '\r'])
        .to_string();
    (!line.is_empty()).then_some(line)
}

fn set_echo(tty: &std::fs::File, on: bool) {
    if !exec::exists("stty") {
        return;
    }
    let mut child = std::process::Command::new("stty");
    if on {
        child.arg("echo");
    } else {
        child.arg("-echo");
    }
    child.stdin(tty.try_clone().unwrap());
    let _ = child.output();
}

/// CLI password prompter (used for both admin and share passwords).
pub struct CliPrompter;

impl Prompter for CliPrompter {
    fn password(&self, ctx: &PromptContext) -> Option<Credentials> {
        if !std::path::Path::new("/dev/tty").exists() {
            eprintln!(
                "mount-manager: {} — but no terminal is available to ask for a password",
                ctx.reason
            );
            eprintln!("  Re-run with --password, or use the graphical app.");
            return None;
        }
        let label = if ctx.username.is_empty() {
            String::new()
        } else {
            format!(" for {}", ctx.username)
        };
        let prompt = format!("{reason}{label}\nPassword: ", reason = ctx.reason);
        if let Some(prev) = &ctx.previous_error {
            eprintln!("{prev}");
        }
        prompt_password_hidden(&prompt).map(Credentials::cached)
    }

    fn status(&self, message: &str) {
        if std::env::var("MOUNT_MANAGER_QUIET").is_err() {
            eprintln!("… {message}");
        }
    }
}

/// Long help text.
pub fn usage() -> String {
    format!(
        r#"{name} {version} — manage mounted external paths

USAGE
  mount-manager                      open the graphical app
  mount-manager <command> [options]  run a command (see below)

COMMANDS
  list [--all] [--json]        show mounted filesystems
  devices [--all] [--json]     show block devices (USB drives, disks)
  shares [--json]              show saved shares and whether they are mounted
  fstab [--json|--restore|--verify]
                               inspect /etc/fstab, restore the backup
  discover [--protocol P] [--host H]
                               find servers and shares on the network
  mount <uri|host> [target] [options]
                               mount a share
  add <uri|host> [target] [options]
                               save a share without mounting it
  test <uri|host> [target] [options]
                               check a share without mounting it
  mount-device /dev/sdX1 [--target DIR]
                               mount a block device
  umount <target>... [--lazy]  unmount
  remove <id|name>             delete a saved share
  auto-mount [--quiet]         mount every share marked "mount at login"
  open <path>                  open a path in the file manager
  mkdir <path>...              create mount points (escalates if needed)
  info                         show escalation method, tools and paths
  version | help

OPTIONS FOR mount/add/test
  --protocol P        smb | nfs | sshfs | webdav | ftp | block | bind | custom
  --host H            server name or IP
  --path P            share name / export / remote directory
  --device /dev/sdX1  block device (implies --protocol block)
  --source S          raw source for --protocol custom
  --fstype F          filesystem type for block/custom
  --target T          mount point (default: /media/$USER/<name>)
  --user U            user name            --password P   ("-" reads stdin)
  --domain D          SMB domain/workgroup --port N       port
  --identity FILE     SSH private key      --option K=V   extra -o option (repeatable)
  --method M          auto | system | gvfs | fuse
  --ro                mount read-only      --persist      add an /etc/fstab entry
  --bookmark          add to the Files sidebar
  --remember-password store the password (keyring when available)
  --take-ownership    chown the mount point to you after mounting
  --save              also store this share for later

GLOBAL OPTIONS
  --escalate M        auto | pkexec | sudo | never
  --json              machine readable output where supported
  --version, --help

EXAMPLES
  mount-manager mount smb://nas/backups /mnt/backups --user jo --persist --save
  mount-manager mount --protocol nfs --host 10.0.0.5 --path /srv/data --target /mnt/data
  mount-manager mount --device /dev/sdb1
  mount-manager umount /mnt/data --lazy
  mount-manager list --json

Root rights are obtained through polkit (pkexec) or sudo; the password is asked
inside the app when sudo needs one. Nothing is ever put on a command line.
"#,
        name = crate::APP_NAME,
        version = crate::VERSION
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Parsed {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_flags_and_positionals() {
        let a = p(&[
            "mount",
            "smb://nas/data",
            "/mnt/nas",
            "--user",
            "jo",
            "--persist",
            "--option",
            "vers=1.0",
        ]);
        assert_eq!(a.positional, vec!["mount", "smb://nas/data", "/mnt/nas"]);
        assert!(a.has("--persist"));
        assert_eq!(a.value(&["--user", "-u"]).as_deref(), Some("jo"));
        assert_eq!(a.values_of("--option"), vec!["vers=1.0"]);
    }

    #[test]
    fn parses_equals_syntax_and_double_dash() {
        let a = p(&["list", "--all", "--", "--not-a-flag"]);
        assert!(a.has("--all"));
        assert_eq!(a.positional, vec!["list", "--not-a-flag"]);
    }

    #[test]
    fn uri_to_request() {
        let r = request_from(&p(&["mount", "smb://jo@nas/data"]), 1).unwrap();
        assert_eq!(r.protocol, Protocol::Cifs);
        assert_eq!(r.host, "nas");
        assert_eq!(r.username, "jo");
        assert_eq!(r.remote_path, "/data");

        let r = request_from(&p(&["mount", "sftp://jo@example.com:2222/home/jo"]), 1).unwrap();
        assert_eq!(r.protocol, Protocol::Sshfs);
        assert_eq!(r.port, Some(2222));
        assert_eq!(r.remote_path, "/home/jo");

        let r = request_from(&p(&["mount", "nfs://10.0.0.5/srv/data", "/mnt/nfs"]), 1).unwrap();
        assert_eq!(r.protocol, Protocol::Nfs);
        assert_eq!(r.host, "10.0.0.5");
        assert_eq!(r.mount_point, "/mnt/nfs");
    }

    #[test]
    fn uri_with_password_is_percent_decoded() {
        let r = request_from(&p(&["mount", "smb://jo:p%40ss@nas/data"]), 1).unwrap();
        assert_eq!(r.password, "p@ss");
    }

    #[test]
    fn flags_build_a_request() {
        let a = p(&[
            "mount",
            "--protocol",
            "nfs",
            "--host",
            "nas",
            "--path",
            "/srv",
            "--target",
            "/mnt/nfs",
            "--ro",
            "--persist",
            "--bookmark",
            "--option",
            "vers=3",
            "--option",
            "nolock",
            "--method",
            "system",
        ]);
        let r = request_from(&a, 1).unwrap();
        assert_eq!(r.protocol, Protocol::Nfs);
        assert_eq!(r.source(), "nas:/srv");
        assert_eq!(r.mount_point, "/mnt/nfs");
        assert!(r.read_only && r.persist && r.bookmark);
        assert_eq!(r.extra_options, vec!["vers=3", "nolock"]);
        assert_eq!(r.method, MountMethod::System);
    }

    #[test]
    fn device_flag_switches_protocol() {
        let r = request_from(
            &p(&["mount", "--device", "/dev/sdb1", "--target", "/mnt/stick"]),
            1,
        )
        .unwrap();
        assert_eq!(r.protocol, Protocol::Block);
        assert_eq!(r.local_source, "/dev/sdb1");
    }

    #[test]
    fn protocol_parsing() {
        assert_eq!(protocol_from_str("SMB").unwrap(), Protocol::Cifs);
        assert_eq!(protocol_from_str("sftp").unwrap(), Protocol::Sshfs);
        assert_eq!(protocol_from_str("dav").unwrap(), Protocol::WebDav);
        assert!(protocol_from_str("smbfs-xyz").is_err());
    }

    #[test]
    fn truncate_shortens() {
        assert_eq!(truncate("abcdefgh", 4), "abc…");
        assert_eq!(truncate("ab", 4), "ab");
    }

    #[test]
    fn usage_mentions_every_command() {
        let u = usage();
        for cmd in [
            "list",
            "devices",
            "shares",
            "fstab",
            "discover",
            "mount",
            "umount",
            "auto-mount",
            "info",
        ] {
            assert!(u.contains(cmd), "usage missing {cmd}");
        }
    }
}

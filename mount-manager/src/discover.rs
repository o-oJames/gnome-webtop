//! Network discovery and connection testing.
//!
//! Every probe is best effort: when a helper tool (`smbclient`, `showmount`,
//! `avahi-browse`, `curl`, `ssh`) is missing we return a helpful "install X"
//! error instead of failing silently.

use crate::error::{Error, Result};
use crate::exec::{self, Cmd};
use crate::model::{Protocol, ShareRequest};
use crate::platform;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A service found on the local network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub host: String,
    pub address: String,
    pub port: u16,
    pub protocol: Protocol,
}

/// One SMB share offered by a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmbShare {
    pub name: String,
    pub kind: String,
    pub comment: String,
}

/// One NFS export offered by a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NfsExport {
    pub path: String,
    pub allowed: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Low level reachability
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve `host` to its first address.
pub fn resolve(host: &str) -> Result<SocketAddr> {
    let hint_port = 1;
    let mut addrs = (host, hint_port)
        .to_socket_addrs()
        .map_err(|e| Error::with_detail(format!("Cannot resolve `{host}`"), e.to_string()))?;
    addrs
        .next()
        .ok_or_else(|| Error::new(format!("`{host}` did not resolve to any address")))
}

/// TCP connect probe — the cheapest way to tell "server reachable" from
/// "credentials wrong".
pub fn tcp_probe(host: &str, port: u16, timeout: Duration) -> Result<()> {
    let mut addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::with_detail(format!("Cannot resolve `{host}`"), e.to_string()))?;
    let mut last_err = Error::new(format!("`{host}:{port}` did not resolve"));
    for addr in addrs.by_ref() {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err =
                    Error::with_detail(format!("Cannot reach `{host}:{port}`"), e.to_string());
            }
        }
    }
    Err(last_err.hint("Is the server switched on, and is the port open in its firewall?"))
}

/// TCP connect probe using the protocol default port.
pub fn probe_request(req: &ShareRequest) -> Result<()> {
    let port = req
        .port
        .or_else(|| req.protocol.default_port())
        .unwrap_or(445);
    tcp_probe(&req.host, port, Duration::from_secs(5))
}

// ─────────────────────────────────────────────────────────────────────────────
// SMB
// ─────────────────────────────────────────────────────────────────────────────

/// List the shares offered by an SMB server using `smbclient -L`.
pub fn smb_shares(
    host: &str,
    user: &str,
    password: &str,
    timeout: Duration,
) -> Result<Vec<SmbShare>> {
    let mut cmd = Cmd::new("smbclient")
        .arg("-L")
        .arg(format!("//{host}"))
        .timeout(timeout);
    if user.is_empty() {
        cmd = cmd.arg("-N");
    } else {
        // Passing the password via -U keeps it out of argv only partially, but
        // smbclient has no stdin credential mode; the process is short lived.
        cmd = cmd.arg("-U").arg(format!("{user}%{password}"));
    }
    if !exec::exists("smbclient") {
        return Err(Error::new("smbclient is not installed").hint("sudo apt install smbclient"));
    }
    let out = exec::run(&cmd)?;
    let shares = parse_smbclient_list(&out.stdout);
    if !shares.is_empty() {
        return Ok(shares);
    }
    let text = out.combined();
    if text.contains("NT_STATUS_LOGON_FAILURE") {
        Err(
            Error::with_detail("The server rejected these credentials", text)
                .hint("Check the user name, password and domain/workgroup."),
        )
    } else if text.contains("NT_STATUS_ACCESS_DENIED") {
        Err(Error::with_detail("Access denied while listing shares", text)
            .hint("Some servers hide the share list from anonymous users — type the share name manually."))
    } else if text.contains("NT_STATUS_CONNECTION_REFUSED") || text.contains("NT_STATUS_IO_TIMEOUT")
    {
        Err(Error::with_detail("The server refused or ignored the SMB connection", text)
            .hint("SMB may be disabled, or the server only speaks SMB1 (enable it in Advanced options with `vers=1.0`)."))
    } else {
        Err(Error::with_detail(
            format!("Could not list shares on `{host}`"),
            text,
        ))
    }
}

/// Parse `smbclient -L` output.
pub fn parse_smbclient_list(text: &str) -> Vec<SmbShare> {
    let mut shares = Vec::new();
    let mut in_table = false;
    for line in text.lines() {
        let l = line.trim_end();
        let t = l.trim();
        if t.starts_with("Sharename") {
            in_table = true;
            continue;
        }
        if in_table {
            if t.starts_with("----") || t.is_empty() {
                if t.is_empty() && !shares.is_empty() {
                    break;
                }
                continue;
            }
            // Table rows are indented; anything else ("SMB1 disabled …",
            // "Reconnecting with SMB1 …") is smbclient chatter.
            if !l.starts_with(' ') && !l.starts_with('\t') {
                break;
            }
            // "public          Disk      Public files"
            let (name, rest) = match t.split_once(char::is_whitespace) {
                Some(pair) => pair,
                None => continue,
            };
            let rest = rest.trim_start();
            let (kind, comment) = match rest.split_once(char::is_whitespace) {
                Some((k, c)) => (k.to_string(), c.trim().to_string()),
                None => (rest.to_string(), String::new()),
            };
            let name = name.trim().to_string();
            if name.is_empty() || kind == "Type" {
                continue;
            }
            shares.push(SmbShare {
                name,
                kind,
                comment,
            });
        }
    }
    shares
}

// ─────────────────────────────────────────────────────────────────────────────
// NFS
// ─────────────────────────────────────────────────────────────────────────────

/// List NFS exports with `showmount -e`.
pub fn nfs_exports(host: &str, timeout: Duration) -> Result<Vec<NfsExport>> {
    if !exec::exists("showmount") {
        return Err(Error::new("showmount is not installed").hint("sudo apt install nfs-common"));
    }
    let out = exec::run(
        &Cmd::new("showmount")
            .args(["-e", "--no-headers", host])
            .timeout(timeout),
    )?;
    if !out.ok() {
        return Err(Error::with_detail(
            format!("Could not list NFS exports on `{host}`"),
            out.combined(),
        )
        .hint("The server must run rpc.mountd and allow your IP in /etc/exports."));
    }
    let exports = parse_showmount(&out.stdout);
    if exports.is_empty() {
        return Err(Error::new(format!(
            "`{host}` exports nothing (or hides the export list)"
        )));
    }
    Ok(exports)
}

/// Parse `showmount -e --no-headers` output (`/export   *(rw,sync)`).
pub fn parse_showmount(text: &str) -> Vec<NfsExport> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let path = f.next()?.to_string();
            if !path.starts_with('/') {
                return None;
            }
            Some(NfsExport {
                path,
                allowed: f.collect::<Vec<_>>().join(" "),
            })
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// mDNS / Avahi
// ─────────────────────────────────────────────────────────────────────────────

/// Browse the local network for a service type (`_smb._tcp`, `_sftp-ssh._tcp`).
pub fn mdns(service: &str, timeout: Duration) -> Vec<Service> {
    if !exec::exists("avahi-browse") {
        return Vec::new();
    }
    let out = exec::run(
        &Cmd::new("avahi-browse")
            .args(["-r", "-t", "-p", service])
            .timeout(timeout),
    );
    let Ok(out) = out else { return Vec::new() };
    parse_avahi(&out.stdout, protocol_for_mdns(service))
}

fn protocol_for_mdns(service: &str) -> Protocol {
    if service.contains("smb") {
        Protocol::Cifs
    } else if service.contains("sftp") || service.contains("ssh") {
        Protocol::Sshfs
    } else if service.contains("webdav") || service.contains("dav") {
        Protocol::WebDav
    } else if service.contains("ftp") {
        Protocol::Ftp
    } else if service.contains("nfs") {
        Protocol::Nfs
    } else {
        Protocol::Custom
    }
}

/// Parse `avahi-browse -r -t -p` output.
///
/// `=;eth0;IPv4;NAS\032SMB;_smb._tcp;local;nas.local;192.168.1.5;445;...`
pub fn parse_avahi(text: &str, protocol: Protocol) -> Vec<Service> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(';').collect();
        if f.len() < 9 || (f[0] != "=" && f[0] != "+") {
            continue;
        }
        let name = unescape_avahi(f[3]);
        let host = unescape_avahi(f[6]);
        let address = f[7].trim().to_string();
        let port: u16 = f[8].trim().parse().unwrap_or(0);
        if host.is_empty() && address.is_empty() {
            continue;
        }
        if out
            .iter()
            .any(|s: &Service| s.name == name && s.address == address)
        {
            continue;
        }
        out.push(Service {
            name,
            host: if host.is_empty() {
                address.clone()
            } else {
                host
            },
            address,
            port,
            protocol,
        });
    }
    out
}

/// Avahi escapes bytes that are not printable ASCII as `\ddd` — three
/// **decimal** digits, e.g. a space becomes `\032`.
pub fn unescape_avahi(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Ok(b) = value[i + 1..i + 4].parse::<u16>() {
                if b <= 255 {
                    out.push(b as u8);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// WebDAV / FTP / SSH probes
// ─────────────────────────────────────────────────────────────────────────────

/// `curl` probe for WebDAV/FTP URLs; returns a human readable status line.
pub fn http_probe(url: &str, user: &str, password: &str, timeout: Duration) -> Result<String> {
    if !exec::exists("curl") {
        return Err(Error::new("curl is not installed").hint("sudo apt install curl"));
    }
    let mut cmd = Cmd::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code} %{url_effective}",
            "--max-time",
            &timeout.as_secs().to_string(),
            "--location",
            "--insecure",
        ])
        .timeout(timeout + Duration::from_secs(3));
    if !user.is_empty() {
        cmd = cmd.args(["-u", &format!("{user}:{password}")]);
    }
    cmd = cmd.arg(url);
    let out = exec::run(&cmd)?;
    if !out.ok() {
        return Err(Error::with_detail(
            format!("Could not reach `{url}`"),
            out.combined(),
        ));
    }
    let code = out
        .stdout
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("000")
        .to_string();
    match code.as_str() {
        "200" | "207" | "301" | "302" | "405" => Ok(format!(
            "Server answered with HTTP {code} — looks like WebDAV."
        )),
        "401" => Ok(format!(
            "Server answered HTTP 401 — it is reachable but wants different credentials."
        )),
        "404" => Ok(format!(
            "Server answered HTTP 404 — reachable, but this path does not exist."
        )),
        "000" => Err(Error::new(format!("No answer from `{url}`"))
            .hint("Check the URL, the port and TLS settings.")),
        other => Ok(format!("Server answered with HTTP {other}.")),
    }
}

/// Non-interactive SSH probe (works with key authentication).
pub fn ssh_probe(
    host: &str,
    port: u16,
    user: &str,
    identity: &str,
    timeout: Duration,
) -> Result<String> {
    if !exec::exists("ssh") {
        return Err(
            Error::new("openssh-client is not installed").hint("sudo apt install openssh-client")
        );
    }
    let target = if user.is_empty() {
        host.to_string()
    } else {
        format!("{user}@{host}")
    };
    let mut cmd = Cmd::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            &format!("ConnectTimeout={}", timeout.as_secs().max(2)),
            "-p",
            &port.to_string(),
        ])
        .timeout(timeout + Duration::from_secs(5));
    if !identity.trim().is_empty() {
        cmd = cmd.args(["-i", identity.trim()]);
    }
    cmd = cmd.arg(target.as_str()).arg("exit");
    let out = exec::run(&cmd)?;
    if out.ok() {
        Ok(format!(
            "SSH login as `{target}` succeeded (key authentication works)."
        ))
    } else {
        let text = out.combined();
        if text.contains("Permission denied") {
            Ok("SSH server is reachable, but key authentication failed — you will be asked for a password when mounting.".to_string())
        } else if text.contains("Connection refused") {
            Err(Error::with_detail(
                format!("SSH refused the connection on `{host}:{port}`"),
                text,
            ))
        } else {
            Err(Error::with_detail(
                format!("SSH probe of `{target}` failed"),
                text,
            ))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// High level "Test connection"
// ─────────────────────────────────────────────────────────────────────────────

/// Everything we can learn about a share request without mounting it.
pub fn test_request(req: &ShareRequest) -> Result<String> {
    let mut report = Vec::new();
    let timeout = Duration::from_secs(6);

    match req.protocol {
        Protocol::Cifs => {
            let port = req.port.unwrap_or(445);
            tcp_probe(&req.host, port, timeout)?;
            report.push(format!(
                "{host}:{port} is reachable (SMB).",
                host = req.host
            ));
            match smb_shares(
                &req.host,
                &req.username,
                &req.password,
                timeout + Duration::from_secs(4),
            ) {
                Ok(shares) => {
                    let wanted = req.remote_path.trim_matches('/');
                    match shares.iter().find(|s| s.name.eq_ignore_ascii_case(wanted)) {
                        Some(s) => {
                            let note = if s.comment.is_empty() {
                                s.kind.clone()
                            } else {
                                s.comment.clone()
                            };
                            report.push(format!("Share `{}` found ({note}).", s.name));
                        }
                        None => report.push(format!(
                            "Share `{wanted}` was NOT in the server list: {}",
                            shares
                                .iter()
                                .map(|s| s.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )),
                    }
                }
                Err(e) => report.push(format!("Share list unavailable: {}", e.message)),
            }
        }
        Protocol::Nfs => {
            let port = req.port.unwrap_or(2049);
            tcp_probe(&req.host, port, timeout)?;
            report.push(format!(
                "{host}:{port} is reachable (NFS).",
                host = req.host
            ));
            match nfs_exports(&req.host, timeout + Duration::from_secs(4)) {
                Ok(exports) => {
                    let wanted = req.remote_path.trim();
                    if exports.iter().any(|e| e.path == wanted) {
                        report.push(format!("Export `{wanted}` is offered by the server."));
                    } else {
                        report.push(format!(
                            "Export `{wanted}` not in the server list: {}",
                            exports
                                .iter()
                                .map(|e| e.path.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
                Err(e) => report.push(format!("Export list unavailable: {}", e.message)),
            }
        }
        Protocol::Sshfs => {
            let port = req.port.unwrap_or(22);
            tcp_probe(&req.host, port, timeout)?;
            report.push(format!(
                "{host}:{port} is reachable (SSH).",
                host = req.host
            ));
            match ssh_probe(&req.host, port, &req.username, &req.identity_file, timeout) {
                Ok(msg) => report.push(msg),
                Err(e) => report.push(format!("SSH probe: {}", e.message)),
            }
        }
        Protocol::WebDav => {
            let url = req.source();
            report.push(http_probe(&url, &req.username, &req.password, timeout)?);
        }
        Protocol::Ftp => {
            let port = req.port.unwrap_or(21);
            tcp_probe(&req.host, port, timeout)?;
            report.push(format!(
                "{host}:{port} is reachable (FTP).",
                host = req.host
            ));
        }
        Protocol::Block => {
            let dev = std::path::Path::new(&req.local_source);
            if !dev.exists() {
                return Err(Error::new(format!(
                    "Device `{}` does not exist",
                    req.local_source
                )));
            }
            let fstype = crate::devices::find_by_path(&req.local_source)
                .map(|d| d.fstype)
                .unwrap_or_default();
            report.push(format!(
                "`{}` exists{}",
                req.local_source,
                if fstype.is_empty() {
                    ", but no filesystem was detected on it".to_string()
                } else {
                    format!(" ({fstype})")
                }
            ));
            if fstype.is_empty() {
                return Err(Error::new(format!(
                    "`{}` has no filesystem — it needs to be formatted first (try GNOME Disks).",
                    req.local_source
                )));
            }
        }
        Protocol::Bind => {
            let src = std::path::Path::new(&req.local_source);
            if !src.is_dir() {
                return Err(Error::new(format!(
                    "`{}` is not a directory",
                    req.local_source
                )));
            }
            report.push(format!(
                "`{}` is a directory and can be bind mounted.",
                req.local_source
            ));
        }
        Protocol::Custom => {
            report.push(format!(
                "Custom source `{}` (type `{}`) — no automatic checks.",
                req.source(),
                req.fstype().unwrap_or_else(|| "auto".to_string())
            ));
        }
    }

    // Mount point sanity.
    let target = std::path::Path::new(&req.mount_point);
    report.push(if target.is_dir() {
        let empty = target.read_dir().map(|mut d| d.next().is_none()).unwrap_or(false);
        if empty {
            format!("Mount point `{}` exists and is empty.", req.mount_point)
        } else {
            format!("Mount point `{}` exists but is NOT empty — its contents will be hidden while mounted.", req.mount_point)
        }
    } else if target.exists() {
        format!("Mount point `{}` exists but is not a directory!", req.mount_point)
    } else {
        format!("Mount point `{}` will be created.", req.mount_point)
    });

    // Warn about missing client tools before the user hits "Mount".
    for pkg in missing_tool_for(req.protocol) {
        report.push(format!(
            "Missing tool: {pkg} — install with `sudo apt install {pkg}`."
        ));
    }

    Ok(report.join("\n"))
}

/// Client packages whose binaries are missing for this protocol.
pub fn missing_tool_for(protocol: Protocol) -> Vec<&'static str> {
    let mut missing = Vec::new();
    match protocol {
        Protocol::Cifs => {
            if !exec::exists_any("mount.cifs") {
                missing.push("cifs-utils");
            }
        }
        Protocol::Nfs => {
            if !exec::exists_any("mount.nfs") && !exec::exists_any("mount.nfs4") {
                missing.push("nfs-common");
            }
        }
        Protocol::Sshfs => {
            if !exec::exists("sshfs") {
                missing.push("sshfs");
            }
        }
        Protocol::WebDav => {
            if !exec::exists_any("mount.davfs") && !exec::exists("gio") {
                missing.push("davfs2");
            }
        }
        Protocol::Ftp => {
            // curlftpfs is not packaged on every release; GVFS covers ftp://.
            if !exec::exists_any("curlftpfs") && !exec::exists("gio") {
                missing.push("gvfs-backends (or curlftpfs)");
            }
        }
        _ => {}
    }
    missing
}

/// Discover servers for a protocol (mDNS first, then protocol specific listing).
pub fn discover_servers(protocol: Protocol) -> Vec<Service> {
    let mut services = protocol
        .mdns_service()
        .map(|s| mdns(s, Duration::from_secs(4)))
        .unwrap_or_default();
    // De-duplicate by address+port.
    let mut seen = Vec::new();
    services.retain(|s| {
        let key = (s.address.clone(), s.port);
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    services
}

/// Absolute path of the user's SSH directory (for the key picker).
pub fn ssh_dir() -> String {
    format!("{}/.ssh", platform::home_dir().display())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMBCLIENT: &str = r#"
Anonymous login successful

	Sharename       Type      Comment
	---------       ----      -------
	public          Disk      Public files
	backups         Disk      Nightly backups
	IPC$            IPC       IPC Service (Samba)
Anonymous login successful
Smb1 disabled -- no workgroup available
"#;

    #[test]
    fn parses_smbclient_share_table() {
        let shares = parse_smbclient_list(SMBCLIENT);
        assert_eq!(shares.len(), 3);
        assert_eq!(shares[0].name, "public");
        assert_eq!(shares[0].kind, "Disk");
        assert_eq!(shares[0].comment, "Public files");
        assert_eq!(shares[2].name, "IPC$");
    }

    #[test]
    fn ignores_smbclient_chatter_after_the_table() {
        let text = "\n\tSharename       Type      Comment\n\t---------       ----      -------\n\tmmshare         Disk      \nSMB1 disabled -- no workgroup available\n";
        let shares = parse_smbclient_list(text);
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].name, "mmshare");
        assert_eq!(shares[0].kind, "Disk");
    }

    #[test]
    fn parses_showmount() {
        let text = "/srv/nfs       *(rw,sync,no_subtree_check)\n/export/home 192.168.1.0/24(rw)\n";
        let e = parse_showmount(text);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].path, "/srv/nfs");
        assert_eq!(e[0].allowed, "*(rw,sync,no_subtree_check)");
        assert_eq!(e[1].path, "/export/home");
    }

    #[test]
    fn parses_avahi_records() {
        let text = "+;eth0;IPv4;NAS\\032SMB;_smb._tcp;local;\n=;eth0;IPv4;NAS\\032SMB;_smb._tcp;local;nas.local;192.168.1.5;445;\n";
        let s = parse_avahi(text, Protocol::Cifs);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "NAS SMB");
        assert_eq!(s[0].host, "nas.local");
        assert_eq!(s[0].address, "192.168.1.5");
        assert_eq!(s[0].port, 445);
    }

    #[test]
    fn avahi_unescape() {
        assert_eq!(unescape_avahi("NAS\\032SMB"), "NAS SMB");
        assert_eq!(unescape_avahi("plain"), "plain");
    }

    #[test]
    fn mdns_protocol_mapping() {
        assert_eq!(protocol_for_mdns("_smb._tcp"), Protocol::Cifs);
        assert_eq!(protocol_for_mdns("_sftp-ssh._tcp"), Protocol::Sshfs);
        assert_eq!(protocol_for_mdns("_webdavs._tcp"), Protocol::WebDav);
        assert_eq!(protocol_for_mdns("_nfs._tcp"), Protocol::Nfs);
    }

    #[test]
    fn tcp_probe_rejects_closed_port() {
        // Port 1 on localhost is closed in any sane environment.
        assert!(tcp_probe("127.0.0.1", 1, Duration::from_millis(400)).is_err());
    }

    #[test]
    fn resolve_localhost() {
        assert!(resolve("localhost").is_ok());
        assert!(resolve("no-such-host.invalid").is_err());
    }
}

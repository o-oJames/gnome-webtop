# Mount Manager

A small GNOME application, written in Rust (GTK 4 + libadwaita), that keeps
every **mounted external path** in one place: mount it, unmount it, see why it
failed, and make it come back after a reboot — without touching a terminal.

It ships as a **Debian package** with a `.desktop` entry, an icon, AppStream
metadata and a polkit action, so after installing it the app simply shows up in
the GNOME app grid as **Mount Manager**.

```
┌──────────────────────────────────────────────────────────────────────────┐
│  [+ Add Share] [⟳]        Mounts │ Devices │ Shares │ Activity      [☰] │
├──────────────────────────────────────────────────────────────────────────┤
│  🗀  NAS data        [SMB] [fstab ✓]                    [Open] [Unmount] │
│     on /mnt/nas · 412.6 GB free of 2.0 TB                                │
│     Network · cifs                                                       │
│  🖴  SanDisk Ultra   [exfat] [removable] [mounted]      [Unmount] [Open] │
│     /dev/sdb1 · exfat · 29.8 GB                                          │
│     mounted on /media/joe/SanDisk Ultra                                  │
└──────────────────────────────────────────────────────────────────────────┘
```

## Features

| Area | What you get |
|------|--------------|
| **Mounts page** | Everything currently mounted (network, removable, bind, GVFS), with size/free space, read-only and `/etc/fstab` badges. Unmount, unmount-lazily when busy, open in Files. |
| **Devices page** | `lsblk` view of USB sticks, SD cards, external disks and partitions — one click to mount under `/media/$USER/<label>`. |
| **Shares page** | Saved shares, their mount state, and Mount / Unmount / Edit / Delete. |
| **Activity page** | A log of every action with the exact command that was run and its output — passwords masked. |
| **Add Share dialog** | Protocol picker, network browsing, per-protocol fields, mount point, options, “mount at login”, Files sidebar bookmark, connection test. |
| **Privileges** | polkit (`pkexec`) first, `sudo -n` when passwordless, and an **in-app password dialog** as the fallback — the password is piped into `sudo -S`, never into argv. |
| **CLI** | Every action is scriptable (`mount-manager list`, `mount`, `umount`, `auto-mount`, …). |

## Supported protocols

| Protocol | System mount (`mount(8)`) | User session (no admin rights) | Discovery | Client package |
|----------|---------------------------|--------------------------------|-----------|----------------|
| **SMB / CIFS** (Windows share, Samba, NAS) | `mount.cifs` with a root-only credentials file | `gio mount smb://…` | mDNS + `smbclient -L` share list | `cifs-utils`, `smbclient` |
| **NFS** (v3/v4, automatic fallback) | `mount.nfs` | — | mDNS + `showmount -e` export list | `nfs-common` |
| **SSHFS / SFTP** | `mount -t fuse.sshfs` | `sshfs` as the user | mDNS + non-interactive SSH probe | `sshfs` |
| **WebDAV / HTTP(S)** (Nextcloud, ownCloud) | `mount.davfs` with `/etc/davfs2/secrets` | `gio mount davs://…` | mDNS + `curl` probe | `davfs2`, `gvfs-backends` |
| **FTP / FTPS** | `curlftpfs` (where packaged) | `gio mount ftp://…` (default) | mDNS + TCP probe | `gvfs-backends`, `libglib2.0-bin` |
| **Block devices** (USB, SD, disk images) | `mount /dev/sdXN` with fstype detection and retries (`ntfs3` → `ntfs-3g`) | — | `lsblk` | `util-linux` |
| **Bind mounts** | `mount --bind` (+ `remount,ro`) | — | — | `util-linux` |
| **Anything else** | “Custom” mode: your own `-t` and source | — | — | — |

## Install

### From the package in this repository (recommended)

```bash
cd mount-manager
docker build -f packaging/Dockerfile.build --target package -t mount-manager-pkg .
docker create --name mm-tmp mount-manager-pkg
docker cp mm-tmp:/src/mount-manager/dist ./dist && docker rm mm-tmp
```

Now `dist/` contains both the `.deb` and a ready-made **local APT repository**:

```bash
# 1. publish the repository (so apt knows the package)
sudo packaging/install-apt-repo.sh ./dist/repo

# 2. install through apt, like any other package
sudo apt-get update
sudo apt-get install mount-manager
```

Or install the single file directly (apt resolves the dependencies):

```bash
sudo apt-get install ./dist/mount-manager_1.0.0_arm64.deb
```

After that, press <kbd>Super</kbd> and type **Mount Manager** — it is in the app
grid, and it can be pinned to the dash like any other app.

### In the GNOME webtop container in this repository

The container `Dockerfile` builds the package in a separate stage and installs
it through the local repository, so the app is part of the image:

```bash
docker compose up -d --build
# open http://localhost:6901/webtop.html → Super → "Mount Manager"
```

### From source

```bash
sudo apt-get build-dep ./debian/control      # or: libgtk-4-dev libadwaita-1-dev cargo
cargo build --release
sudo ./packaging/build-deb.sh                # builds dist/*.deb
# development, without packaging:
cargo run                                    # GUI
cargo run -- list --all                      # CLI
sudo dpkg-buildpackage -us -uc -b            # the "official" Debian way
```

Requirements: Rust ≥ 1.80, GTK 4 ≥ 4.10 and libadwaita ≥ 1.4 (Ubuntu 24.04 and
newer). Build the privileged helper without GTK using
`cargo build --release --no-default-features --bin mount-manager-helper`.

## Usage

### Graphical

1. **Add Share** → pick the protocol.
2. Click the **browse** icon to find servers on the network (mDNS) and list the
   shares (`smbclient -L`) or exports (`showmount -e`) they offer.
3. **Test** checks reachability, credentials, the existence of the share and the
   mount point — and tells you which package is missing, if any.
4. **Mount**. If root rights are needed you get the polkit dialog, or — when
   polkit has no agent, as in containers — the app's own password prompt.
5. Tick **Mount at login** to write an `/etc/fstab` entry. Mount Manager writes
   it atomically, keeps a backup at `/etc/fstab.mount-manager.bak`, uses
   `nofail,_netdev` for network shares and stores credentials in a root-only
   file (`/etc/mount-manager/credentials/*.cred`) instead of in `fstab`.

### Command line

```bash
mount-manager list --all                       # what is mounted
mount-manager devices                          # USB drives, disks
mount-manager mount smb://nas/backups /mnt/backups --user joe --persist --save
mount-manager mount --protocol nfs --host 10.0.0.5 --path /srv/data
mount-manager mount --device /dev/sdb1
mount-manager umount /mnt/backups [--lazy]
mount-manager shares --json
mount-manager discover --protocol smb --host nas
mount-manager test --protocol sshfs --host build.example.org --user ci
mount-manager auto-mount                       # everything marked "mount at login"
mount-manager fstab --verify
mount-manager info                             # escalation method, tools, paths
```

`mount-manager help` prints the full list. Every command exits non-zero on
failure and can print `--json`, so it is usable from scripts and provisioning.

## How privilege escalation works

```
UI / CLI (unprivileged)
   │  Op as JSON on stdin           ← passwords never appear in argv
   ▼
pkexec  ──polkit action io.github.mount_manager.manage──►  /usr/lib/mount-manager/mount-manager-helper (root)
sudo -n ──passwordless sudo────────────────────────────►  the same helper
sudo -S ──password from the app's own dialog───────────►  the same helper
```

* The helper is a **separate binary** that does not link GTK. It reads exactly
  one JSON operation from stdin, re-validates it and executes it.
* Validation is not delegated to the UI: the helper refuses protected targets
  (`/`, `/usr`, `/etc/…`, `/var/lib/…`, …), refuses to write credential files
  outside `/etc/mount-manager`, `/run/mount-manager` and `/etc/davfs2`, only
  removes mount points it may have created (`/media`, `/mnt`, `/run/media`) and
  never touches an `/etc/fstab` entry that it did not write itself.
* `pkexec` is preferred; if polkit cannot authorise (no agent, container, …) the
  app falls back to `sudo -S` and asks for the password **inside the window**.
* Passwordless sudo is detected first, so containers need no prompt at all.
* The password is cached in memory for the session (toggle: “Remember for this
  session”) and can be dropped at any time from the menu
  (**Forget administrator password**).

### No admin rights at all?

Choose **Mount as → user session (GVFS)** or **user FUSE mount**: SMB, SFTP,
WebDAV and FTP can then be mounted by your own user, appearing under
`/run/user/$UID/gvfs` and in the Files sidebar. The app picks this
automatically when it cannot escalate.

## Security notes

* Passwords for **system** mounts go into a root-owned `0600` file
  (`/etc/mount-manager/credentials/<id>.cred`) or `/etc/davfs2/secrets`; they are
  deleted again for temporary mounts and never placed on a command line.
* **Remembered** share passwords use the Secret Service (GNOME Keyring) through
  `secret-tool` when available. Otherwise they are stored base64-obfuscated in
  `~/.config/mount-manager/config.json` (mode `0600`) — the app says so in the
  dialog, because obfuscation is not encryption.
* GVFS URIs (`gio mount smb://user:pass@host/share`) do expose the password in
  the argument list of a short-lived process; prefer the system mount method
  when the same share is reachable both ways.
* Every `/etc/fstab` write creates `/etc/fstab.mount-manager.bak` first, and
  `mount-manager fstab --restore` puts it back.

## Files installed by the package

| Path | Purpose |
|------|---------|
| `/usr/bin/mount-manager` | GTK app + CLI |
| `/usr/lib/mount-manager/mount-manager-helper` | privileged helper (pkexec/sudo) |
| `/usr/share/applications/io.github.mount_manager.desktop` | app grid / dash entry |
| `/usr/share/icons/hicolor/scalable/apps/mount-manager.svg` | app icon (+ symbolic) |
| `/usr/share/metainfo/io.github.mount_manager.metainfo.xml` | GNOME Software metadata |
| `/usr/share/polkit-1/actions/io.github.mount_manager.policy` | polkit action |
| `/usr/share/bash-completion/completions/mount-manager` | CLI completion |
| `/etc/mount-manager/credentials/` | root-only credential files (created by postinst) |

## Project layout

```
mount-manager/
├── Cargo.toml              # lib + 2 binaries, GUI behind the "gui" feature
├── src/
│   ├── lib.rs              # module map
│   ├── main.rs             # GUI by default, CLI when arguments are given
│   ├── bin/mount-manager-helper.rs
│   ├── cli.rs              # argument parsing, tables, TTY password prompt
│   ├── engine.rs           # high-level API used by GUI *and* CLI
│   ├── model.rs            # protocols, share requests, mount entries
│   ├── mounts.rs           # /proc/self/mountinfo parsing + classification
│   ├── devices.rs          # lsblk (with /proc/partitions fallback)
│   ├── fstab.rs            # atomic, backed-up /etc/fstab editing
│   ├── discover.rs         # mDNS, smbclient, showmount, TCP/SSH/HTTP probes
│   ├── ops.rs              # JSON protocol between app and helper
│   ├── executor.rs         # the only code that runs mount/umount as root
│   ├── privilege.rs        # pkexec / sudo -n / sudo -S, password caching
│   ├── secret.rs           # keyring + obfuscated fallback
│   ├── config.rs           # ~/.config/mount-manager/config.json
│   ├── platform.rs         # users, paths, statvfs, protected paths
│   ├── exec.rs             # subprocesses with timeouts, no shell
│   └── ui/                 # GTK4 + libadwaita front-end
├── data/                   # .desktop, icons, AppStream, polkit policy
├── debian/                 # control, changelog, rules, postinst, …
└── packaging/              # build-deb.sh, make-apt-repo.sh, Dockerfile.build
```

## Tests

```bash
cargo test --lib                       # 82 unit tests (parsers, validation, safety)
cargo test --no-default-features       # core only, no GTK needed
./packaging/dev-check.sh test          # run them inside the Linux container
```

The tests cover the parts that matter most: `mountinfo` parsing and mount
classification, `fstab` parsing/escaping/rewriting, `lsblk`/`smbclient`/
`showmount`/`avahi` output parsing, mount option ladders, and the safety rules
(protected targets, credential file locations, passwords never in `fstab`).

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| “The `cifs-utils` package is required” | `sudo apt-get install cifs-utils` (the app tells you the exact package for every protocol) |
| FTP: `curlftpfs` cannot be installed | Ubuntu 24.04 dropped that package — use **Mount as → user session (GVFS)**, which is the default for FTP |
| GVFS mounts unavailable (`gio` missing) | `sudo apt-get install libglib2.0-bin gvfs-backends` |
| polkit dialog never appears (container) | Expected: the app falls back to `sudo`. See Preferences → Administrator rights. |
| `mount error(112): Host is down` | Server only speaks SMB1 — add `vers=1.0` under “Extra mount options”. |
| NFS “access denied by server” | The server's `/etc/exports` does not allow your address. |
| Unmount says “target is busy” | The Activity page lists the processes; use “Unmount anyway (lazy)”. |
| Share is mounted but Files shows nothing | It was mounted as GVFS: look under `/run/user/$UID/gvfs` or use the Open button. |
| App does not appear in the grid | `sudo update-desktop-database && sudo gtk-update-icon-cache -f /usr/share/icons/hicolor` |

## License

MIT — see [LICENSE](LICENSE).

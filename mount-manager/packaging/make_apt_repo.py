#!/usr/bin/env python3
"""Generate APT repository metadata without apt-utils.

`make-apt-repo.sh` prefers `apt-ftparchive` (from the apt-utils package); this
script is the fallback so the repository can also be produced on a minimal
image. It implements just enough of the format for `apt-get update` to accept
an unsigned local repository:

    dists/<suite>/main/binary-<arch>/Packages[.gz]
    dists/<suite>/Release

Usage:
    make_apt_repo.py packages <repo_dir> <out_file> <arch>
    make_apt_repo.py release  <suite_dir> <origin> <suite> <component> <archs...>
"""

from __future__ import annotations

import gzip
import hashlib
import os
import subprocess
import sys
from datetime import datetime, timezone

FIELDS = [
    "Package", "Version", "Architecture", "Maintainer", "Installed-Size",
    "Depends", "Recommends", "Suggests", "Conflicts", "Breaks", "Provides",
    "Section", "Priority", "Homepage", "Description",
]


def deb_field(deb: str, field: str) -> str:
    out = subprocess.run(["dpkg-deb", "-f", deb, field],
                         capture_output=True, text=True, check=False)
    return out.stdout.strip()


def checksums(path: str) -> tuple[str, str, str, int]:
    with open(path, "rb") as fh:
        data = fh.read()
    return (hashlib.md5(data).hexdigest(),
            hashlib.sha1(data).hexdigest(),
            hashlib.sha256(data).hexdigest(),
            len(data))


def cmd_packages(repo_dir: str, out_file: str, arch: str) -> int:
    pool = os.path.join(repo_dir, "pool")
    stanzas: list[str] = []
    for root, _dirs, files in os.walk(pool):
        for name in sorted(files):
            if not name.endswith(".deb"):
                continue
            deb = os.path.join(root, name)
            deb_arch = deb_field(deb, "Architecture")
            if deb_arch not in (arch, "all"):
                continue
            filename = os.path.relpath(deb, repo_dir)
            md5, sha1, sha256, size = checksums(deb)
            fields = {f: deb_field(deb, f) for f in FIELDS if deb_field(deb, f)}
            fields.setdefault("Section", "admin")
            fields.setdefault("Priority", "optional")
            fields["Filename"] = filename
            fields["Size"] = str(size)
            fields["MD5sum"] = md5
            fields["SHA1"] = sha1
            fields["SHA256"] = sha256
            lines = []
            for key in ["Package", "Version", "Architecture", "Maintainer",
                        "Installed-Size", "Depends", "Recommends", "Suggests",
                        "Conflicts", "Breaks", "Provides", "Section", "Priority",
                        "Homepage", "Filename", "Size", "MD5sum", "SHA1", "SHA256"]:
                if fields.get(key):
                    lines.append(f"{key}: {fields[key]}")
            description = fields.get("Description", "")
            lines.append("Description: " + (description.split("\n", 1)[0] or filename))
            for extra in description.split("\n")[1:]:
                lines.append(" " + extra if extra.strip() else " .")
            stanzas.append("\n".join(lines))
    os.makedirs(os.path.dirname(out_file), exist_ok=True)
    with open(out_file, "w", encoding="utf-8") as fh:
        if stanzas:
            fh.write("\n\n".join(stanzas) + "\n")
    return 0


def cmd_release(suite_dir: str, origin: str, suite: str, component: str, archs: list[str]) -> int:
    entries: list[tuple[str, str, str, str, int]] = []  # md5, sha1, sha256, relpath, size
    for root, _dirs, files in os.walk(suite_dir):
        for name in sorted(files):
            if name == "Release":
                continue
            path = os.path.join(root, name)
            md5, sha1, sha256, size = checksums(path)
            entries.append((md5, sha1, sha256, os.path.relpath(path, suite_dir), size))
    entries.sort(key=lambda e: e[3])

    now = datetime.now(timezone.utc).strftime("%a, %d %b %Y %H:%M:%S +0000")
    out = [
        f"Origin: {origin}",
        f"Label: {origin}",
        f"Suite: {suite}",
        f"Codename: {suite}",
        f"Date: {now}",
        f"Components: {component}",
        f"Architectures: {' '.join(archs)}",
        f"Description: Local repository for {origin}",
        "Acquire-By-Hash: no",
        "MD5Sum:",
    ]
    out += [f" {e[0]} {e[4]:>8} {e[3]}" for e in entries]
    out.append("SHA1:")
    out += [f" {e[1]} {e[4]:>8} {e[3]}" for e in entries]
    out.append("SHA256:")
    out += [f" {e[2]} {e[4]:>8} {e[3]}" for e in entries]
    sys.stdout.write("\n".join(out) + "\n")
    return 0


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    if argv[1] == "packages" and len(argv) == 5:
        return cmd_packages(argv[2], argv[3], argv[4])
    if argv[1] == "release" and len(argv) >= 7:
        return cmd_release(argv[2], argv[3], argv[4], argv[5], argv[6:])
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))

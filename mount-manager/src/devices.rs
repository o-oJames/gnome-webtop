//! Block device enumeration (`lsblk`), with a `/proc/partitions` fallback.

use crate::error::{Error, Result};
use crate::exec::{self, Cmd};
use crate::model::BlockDevice;
use std::time::Duration;

/// Columns we ask `lsblk` for. Missing columns are tolerated (older util-linux).
const COLUMNS: &str = "NAME,PATH,KNAME,LABEL,FSTYPE,SIZE,MOUNTPOINT,MOUNTPOINTS,TYPE,RM,RO,MODEL,VENDOR,TRAN,UUID,PTTYPE";

/// Every block device on the system, flattened (children included).
pub fn list() -> Result<Vec<BlockDevice>> {
    match lsblk() {
        Ok(devs) if !devs.is_empty() => Ok(devs),
        Ok(_) | Err(_) => Ok(proc_partitions_fallback()),
    }
}

/// Devices worth showing in the UI (real filesystems, removable media).
pub fn list_interesting() -> Result<Vec<BlockDevice>> {
    Ok(list()?.into_iter().filter(|d| d.interesting()).collect())
}

fn lsblk() -> Result<Vec<BlockDevice>> {
    let out = exec::run(
        &Cmd::new("lsblk")
            .args(["-J", "-b", "-o", COLUMNS])
            .timeout(Duration::from_secs(10)),
    )?;
    if !out.ok() {
        return Err(Error::with_detail("lsblk failed", out.combined()));
    }
    Ok(parse_lsblk_json(&out.stdout))
}

/// Flatten the `lsblk --json` tree into a list, remembering each parent's name.
pub fn parse_lsblk_json(text: &str) -> Vec<BlockDevice> {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let root = value
        .get("blockdevices")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    walk(&root, "", &mut out);
    out
}

fn walk(node: &serde_json::Value, parent: &str, out: &mut Vec<BlockDevice>) {
    if let Some(arr) = node.as_array() {
        for item in arr {
            walk(item, parent, out);
        }
        return;
    }
    let Some(obj) = node.as_object() else { return };

    let str_field = |key: &str| -> String {
        match obj.get(key) {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    };
    let bool_field = |key: &str| -> bool {
        match obj.get(key) {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
            Some(serde_json::Value::Number(n)) => n.as_u64() == Some(1),
            _ => false,
        }
    };

    let name = str_field("name");
    let path = {
        let p = str_field("path");
        if p.is_empty() {
            format!("/dev/{name}")
        } else {
            p
        }
    };
    let size = match obj.get("size") {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse::<u64>().unwrap_or(0),
        _ => 0,
    };
    let mut mountpoints: Vec<String> = Vec::new();
    if let Some(serde_json::Value::Array(arr)) = obj.get("mountpoints") {
        for v in arr {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    mountpoints.push(s.to_string());
                }
            }
        }
    } else {
        let mp = str_field("mountpoint");
        if !mp.is_empty() {
            mountpoints.push(mp);
        }
    }

    out.push(BlockDevice {
        name: name.clone(),
        path,
        label: str_field("label"),
        fstype: str_field("fstype"),
        size,
        mountpoints,
        removable: bool_field("rm"),
        model: str_field("model"),
        vendor: str_field("vendor"),
        kind: str_field("type"),
        transport: str_field("tran"),
        uuid: str_field("uuid"),
        parent: parent.to_string(),
    });

    if let Some(children) = obj.get("children") {
        walk(children, &name, out);
    }
}

/// Minimal device list when `lsblk` is unavailable (busybox-ish containers).
pub fn proc_partitions_fallback() -> Vec<BlockDevice> {
    let Ok(text) = std::fs::read_to_string("/proc/partitions") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines().skip(2) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let name = f[3].to_string();
        let size = f[1].parse::<u64>().unwrap_or(0) * 1024;
        let path = format!("/dev/{name}");
        let removable = std::fs::read_to_string(format!("/sys/block/{name}/removable"))
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        out.push(BlockDevice {
            name,
            path,
            removable,
            size,
            kind: "part".to_string(),
            ..Default::default()
        });
    }
    out
}

/// Look up a single device by path (`/dev/sdb1`).
pub fn find_by_path(path: &str) -> Option<BlockDevice> {
    list().ok()?.into_iter().find(|d| d.path == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{
  "blockdevices": [
    {"name":"sda","path":"/dev/sda","label":null,"fstype":null,"size":"256060514304",
     "mountpoints":[],"rm":false,"type":"disk","model":"NVMe SSD","tran":"sata","uuid":null,
     "children":[
       {"name":"sda1","path":"/dev/sda1","label":null,"fstype":"vfat","size":"536870912",
        "mountpoints":["/boot/efi"],"rm":false,"type":"part","model":null,"tran":null,"uuid":"1234-ABCD"},
       {"name":"sda2","path":"/dev/sda2","label":"ubuntu","fstype":"ext4","size":"255522504704",
        "mountpoints":["/"],"rm":false,"type":"part","model":null,"tran":null,"uuid":"abcd"}
     ]},
    {"name":"sdb","path":"/dev/sdb","label":"STICK","fstype":"exfat","size":"32000000000",
     "mountpoints":[],"rm":true,"type":"disk","model":"Flash","tran":"usb","uuid":"XYZ"},
    {"name":"loop0","path":"/dev/loop0","fstype":"squashfs","size":65536,"mountpoint":"/snap/x",
     "rm":false,"type":"loop"}
  ]
}"#;

    #[test]
    fn flattens_children_and_reads_both_mountpoint_shapes() {
        let v = parse_lsblk_json(JSON);
        // sda + sda1 + sda2 + sdb + loop0
        assert_eq!(v.len(), 5);
        let sda1 = v.iter().find(|d| d.name == "sda1").unwrap();
        assert_eq!(sda1.parent, "sda");
        assert_eq!(sda1.mountpoints, vec!["/boot/efi".to_string()]);
        assert_eq!(sda1.size, 536870912);
        let loop0 = v.iter().find(|d| d.name == "loop0").unwrap();
        assert_eq!(loop0.mountpoints, vec!["/snap/x".to_string()]);
    }

    #[test]
    fn interesting_filters_loops_and_empty_disks() {
        let v = parse_lsblk_json(JSON);
        let i: Vec<&str> = v
            .iter()
            .filter(|d| d.interesting())
            .map(|d| d.name.as_str())
            .collect();
        assert!(i.contains(&"sdb"), "{i:?}");
        assert!(i.contains(&"sda1"), "{i:?}");
        assert!(!i.contains(&"loop0"), "{i:?}");
        assert!(!i.contains(&"sda"), "{i:?}");
    }

    #[test]
    fn titles_and_icons() {
        let v = parse_lsblk_json(JSON);
        let stick = v.iter().find(|d| d.name == "sdb").unwrap();
        assert_eq!(stick.title(), "STICK");
        assert_eq!(stick.icon(), "drive-removable-media-symbolic");
        assert!(stick.subtitle().contains("exfat"));
        assert!(stick.subtitle().contains("GB"), "{}", stick.subtitle());
        let sda = v.iter().find(|d| d.name == "sda").unwrap();
        assert_eq!(sda.title(), "NVMe SSD");
        assert_eq!(sda.icon(), "drive-harddisk-symbolic");
    }

    #[test]
    fn invalid_json_is_empty() {
        assert!(parse_lsblk_json("not json").is_empty());
        assert!(parse_lsblk_json("{}").is_empty());
    }
}

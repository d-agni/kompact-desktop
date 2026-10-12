//! 6m-b: the logo menu's system side. "About kompact" facts (read from /etc, /proc and /sys,
//! parsed by pure, tested functions) and the Restart/Shut Down commands.

use serde::Serialize;

/// What "About kompact" shows.
#[derive(Debug, Serialize, PartialEq)]
pub struct SystemInfo {
    pub version: String,
    pub os: String,
    pub kernel: String,
    pub cpu: String,
    pub gpu: String,
    pub memory: String,
}

pub fn read() -> SystemInfo {
    let file = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    SystemInfo {
        version: format!("kompact-shell {}", env!("CARGO_PKG_VERSION")),
        os: os_name(&file("/etc/os-release")).unwrap_or_else(|| "Linux".into()),
        kernel: file("/proc/sys/kernel/osrelease").trim().to_string(),
        cpu: cpu_name(&file("/proc/cpuinfo")).unwrap_or_default(),
        gpu: gpu_name(file("/sys/class/drm/card0/device/vendor").trim(), &driver_name("/sys/class/drm/card0/device/driver")),
        memory: memory_total(&file("/proc/meminfo")).unwrap_or_default(),
    }
}

fn driver_name(link: &str) -> String {
    std::fs::read_link(link)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// `PRETTY_NAME` from os-release, unquoted.
pub fn os_name(os_release: &str) -> Option<String> {
    let v = os_release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME="))?;
    let v = v.trim().trim_matches('"').trim_matches('\'');
    (!v.is_empty()).then(|| v.to_string())
}

/// The first `model name` in /proc/cpuinfo, with runs of spaces squeezed.
pub fn cpu_name(cpuinfo: &str) -> Option<String> {
    let line = cpuinfo.lines().find(|l| l.starts_with("model name"))?;
    let name = line.split_once(':')?.1.split_whitespace().collect::<Vec<_>>().join(" ");
    (!name.is_empty()).then_some(name)
}

/// "NVIDIA (nouveau)" from the PCI vendor id and the kernel driver's name.
pub fn gpu_name(vendor: &str, driver: &str) -> String {
    let maker = match vendor.to_ascii_lowercase().as_str() {
        "0x10de" => "NVIDIA",
        "0x1002" => "AMD",
        "0x8086" => "Intel",
        "" => "",
        _ => "Graphics",
    };
    match (maker, driver) {
        ("", "") => String::new(),
        ("", d) => d.to_string(),
        (m, "") => m.to_string(),
        (m, d) => format!("{m} ({d})"),
    }
}

/// `MemTotal` as "16 GB" (rounded to whole GB, or MB below 1 GB).
pub fn memory_total(meminfo: &str) -> Option<String> {
    let kib: u64 = meminfo.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.split_whitespace().next()?.parse().ok()?;
    let gib = kib as f64 / (1024.0 * 1024.0);
    Some(if gib >= 1.0 { format!("{} GB", gib.round() as u64) } else { format!("{} MB", kib / 1024) })
}

/// 12j: System Settings → About: what "About kompact" shows, and more.
#[derive(Debug, Serialize)]
pub struct About {
    #[serde(flatten)]
    pub info: SystemInfo,
    /// The pretty host name (`/etc/machine-info`), else the host name.
    pub name: String,
    pub model: String,
    /// "GK208B [GeForce GT 710]" from pci.ids, or empty.
    pub gpu_model: String,
    pub disk: String,
    pub uptime: String,
}

#[tauri::command]
pub fn about_state() -> About {
    let file = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let dmi = |f: &str| file(&format!("/sys/class/dmi/id/{f}"));
    let host = file("/proc/sys/kernel/hostname");
    let ids = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"].iter().map(|p| file(p)).find(|t| !t.is_empty()).unwrap_or_default();
    let dev = |f: &str| file(&format!("/sys/class/drm/card0/device/{f}"));
    About {
        info: read(),
        name: pretty_host(&file("/etc/machine-info")).unwrap_or_else(|| host.trim().to_string()),
        model: model_name(&dmi("sys_vendor"), &dmi("product_name"), &dmi("board_vendor"), &dmi("board_name")),
        gpu_model: pci_device(&ids, &dev("vendor"), &dev("device")).unwrap_or_default(),
        disk: disk_space("/").map(|(total, free)| disk_text(total, free)).unwrap_or_default(),
        uptime: file("/proc/uptime").split_whitespace().next().and_then(|s| s.parse::<f64>().ok()).map(|s| uptime_text(s as u64)).unwrap_or_default(),
    }
}

/// `PRETTY_HOSTNAME` from /etc/machine-info (hostnamed's), unquoted.
pub fn pretty_host(machine_info: &str) -> Option<String> {
    let v = machine_info.lines().find_map(|l| l.strip_prefix("PRETTY_HOSTNAME="))?;
    let v = v.trim().trim_matches('"').trim_matches('\'');
    (!v.is_empty()).then(|| v.to_string())
}

/// Firmware placeholders that name nothing.
fn placeholder(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.is_empty() || ["to be filled", "o.e.m", "default string", "system product name", "system manufacturer", "not applicable", "not specified"].iter().any(|p| l.contains(p))
}

/// "Gigabyte B450M S2H": the system's vendor and product, else the board's; the vendor's
/// "Technology Co., Ltd." and the like dropped, and not repeated when the product has it.
pub fn model_name(vendor: &str, product: &str, board_vendor: &str, board_name: &str) -> String {
    let (vendor, product) = if placeholder(product.trim()) { (board_vendor.trim(), board_name.trim()) } else { (vendor.trim(), product.trim()) };
    if placeholder(product) {
        return String::new();
    }
    let vendor = if placeholder(vendor) { "" } else { vendor.split([' ', ',']).next().unwrap_or("") };
    if vendor.is_empty() || product.to_lowercase().starts_with(&vendor.to_lowercase()) {
        product.to_string()
    } else {
        format!("{vendor} {product}")
    }
}

/// The device's name in pci.ids (`vendor`/`device` as sysfs has them, "0x10de").
pub fn pci_device(ids: &str, vendor: &str, device: &str) -> Option<String> {
    let v = vendor.trim().trim_start_matches("0x").to_ascii_lowercase();
    let d = device.trim().trim_start_matches("0x").to_ascii_lowercase();
    if v.len() != 4 || d.len() != 4 {
        return None;
    }
    let mut in_vendor = false;
    for line in ids.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            in_vendor = line.get(..4).is_some_and(|id| id.eq_ignore_ascii_case(&v));
        } else if in_vendor && !line.starts_with("\t\t") {
            let rest = &line[1..];
            if rest.get(..4).is_some_and(|id| id.eq_ignore_ascii_case(&d)) {
                let name = rest[4..].trim();
                return (!name.is_empty()).then(|| name.to_string());
            }
        }
    }
    None
}

/// Total and available bytes of the file system holding `path`.
fn disk_space(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    // SAFETY: a valid C string and an out-pointer to a zeroed struct of the right type.
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            return None;
        }
        let unit = s.f_frsize as u64;
        Some((s.f_blocks as u64 * unit, s.f_bavail as u64 * unit))
    }
}

/// "120 GB available of 234 GB" (decimal units, as macOS counts).
pub fn disk_text(total: u64, free: u64) -> String {
    let size = |b: u64| {
        let gb = b as f64 / 1e9;
        if gb >= 1000.0 {
            format!("{:.1} TB", gb / 1000.0)
        } else if gb >= 10.0 {
            format!("{} GB", gb.round() as u64)
        } else {
            format!("{gb:.1} GB")
        }
    };
    format!("{} available of {}", size(free), size(total))
}

/// "3 days, 4 hours", "2 hours, 5 minutes", "7 minutes".
pub fn uptime_text(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60);
    let unit = |n: u64, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    match (d, h) {
        (0, 0) => unit(m, "minute"),
        (0, _) => format!("{}, {}", unit(h, "hour"), unit(m, "minute")),
        _ => format!("{}, {}", unit(d, "day"), unit(h, "hour")),
    }
}

/// The logo menu's power items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Restart,
    ShutDown,
    /// 12i-2: only with `KOMPACT_SLEEP=on` (`power.rs`).
    Sleep,
}

impl Power {
    pub fn parse(s: &str) -> Option<Power> {
        match s {
            "restart" => Some(Power::Restart),
            "shutdown" => Some(Power::ShutDown),
            "sleep" => Some(Power::Sleep),
            _ => None,
        }
    }

    /// Through logind: allowed without a password for the user of the active local session.
    /// Suspend only on request (`KOMPACT_SLEEP=on`): suspend/resume on the GT 710 (nouveau) is
    /// untested with kompact.
    pub fn command(self) -> [&'static str; 2] {
        match self {
            Power::Restart => ["systemctl", "reboot"],
            Power::ShutDown => ["systemctl", "poweroff"],
            Power::Sleep => ["systemctl", "suspend"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_actions() {
        assert_eq!(Power::parse("sleep").map(Power::command), Some(["systemctl", "suspend"]));
        assert_eq!(Power::parse("restart").map(Power::command), Some(["systemctl", "reboot"]));
        assert_eq!(Power::parse("hibernate"), None);
    }

    #[test]
    fn os_release() {
        assert_eq!(os_name("NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 26.04.1 LTS\"\n").as_deref(), Some("Ubuntu 26.04.1 LTS"));
        assert_eq!(os_name("PRETTY_NAME=Arch Linux\n").as_deref(), Some("Arch Linux"));
        assert_eq!(os_name("NAME=x\n"), None);
        assert_eq!(os_name("PRETTY_NAME=\"\"\n"), None);
    }

    #[test]
    fn cpu() {
        let info = "processor\t: 0\nmodel name\t: AMD Ryzen 3 3100 4-Core Processor\nprocessor\t: 1\nmodel name\t: other\n";
        assert_eq!(cpu_name(info).as_deref(), Some("AMD Ryzen 3 3100 4-Core Processor"));
        assert_eq!(cpu_name("model name\t:   Intel(R)   Core(TM)  i5\n").as_deref(), Some("Intel(R) Core(TM) i5"));
        assert_eq!(cpu_name("processor: 0\n"), None);
    }

    #[test]
    fn gpu() {
        assert_eq!(gpu_name("0x10de", "nouveau"), "NVIDIA (nouveau)");
        assert_eq!(gpu_name("0x8086", ""), "Intel");
        assert_eq!(gpu_name("", "virtio-pci"), "virtio-pci");
        assert_eq!(gpu_name("", ""), "");
        assert_eq!(gpu_name("0x1234", "x"), "Graphics (x)");
    }

    #[test]
    fn memory() {
        assert_eq!(memory_total("MemTotal:       16303412 kB\nMemFree: 1 kB\n").as_deref(), Some("16 GB"));
        assert_eq!(memory_total("MemTotal: 524288 kB\n").as_deref(), Some("512 MB"));
        assert_eq!(memory_total("MemFree: 1 kB\n"), None);
    }

    #[test]
    fn about_facts() {
        assert_eq!(model_name("Gigabyte Technology Co., Ltd.\n", "B450M S2H\n", "x", "y"), "Gigabyte B450M S2H");
        assert_eq!(model_name("LENOVO", "LENOVO ThinkPad T480", "", ""), "LENOVO ThinkPad T480");
        assert_eq!(model_name("System manufacturer", "System Product Name", "ASUSTeK COMPUTER INC.", "PRIME B450M-A"), "ASUSTeK PRIME B450M-A");
        assert_eq!(model_name("", "To Be Filled By O.E.M.", "", "Default string"), "");
        assert_eq!(model_name("QEMU", "Standard PC (Q35 + ICH9, 2009)", "", ""), "QEMU Standard PC (Q35 + ICH9, 2009)");
        let ids = "# comment\n1002  AMD\n\t128b  Not this\n10de  NVIDIA Corporation\n\t1280  GK208 [GeForce GT 635]\n\t128b  GK208B [GeForce GT 710]\n\t\t1043 8576  GT710-SL-1GD5\n8086  Intel\n";
        assert_eq!(pci_device(ids, "0x10de\n", "0x128b\n").as_deref(), Some("GK208B [GeForce GT 710]"));
        assert_eq!(pci_device(ids, "0x10de", "0x9999"), None);
        assert_eq!(pci_device(ids, "", ""), None);
        assert_eq!(pretty_host("PRETTY_HOSTNAME=\"Office PC\"\nCHASSIS=desktop\n").as_deref(), Some("Office PC"));
        assert_eq!(pretty_host("CHASSIS=desktop\n"), None);
        assert_eq!(disk_text(234_000_000_000, 120_400_000_000), "120 GB available of 234 GB");
        assert_eq!(disk_text(2_000_000_000_000, 5_250_000_000), "5.2 GB available of 2.0 TB");
        assert_eq!(uptime_text(7 * 60), "7 minutes");
        assert_eq!(uptime_text(3600 + 60), "1 hour, 1 minute");
        assert_eq!(uptime_text(3 * 86400 + 4 * 3600 + 59), "3 days, 4 hours");
    }

    #[test]
    fn power() {
        assert_eq!(Power::parse("restart").map(Power::command), Some(["systemctl", "reboot"]));
        assert_eq!(Power::parse("shutdown").map(Power::command), Some(["systemctl", "poweroff"]));
        assert_eq!(Power::parse("suspend"), None);
    }
}

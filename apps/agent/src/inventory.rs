//! What this machine is: OS, hardware, disks, network, software and security state.
//! Every probe fails soft: what it can't find stays `None` or empty.

use deskhand_proto::{Cpu, Disk, Hardware, Inventory, Memory, Nic, OsInfo};
use sysinfo::{Disks, Networks, Product, System};

/// Collects on a thread of its own (Windows COM wants one it can initialize) and waits.
pub async fn collect() -> Inventory {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(collect_blocking());
    });
    rx.await.unwrap_or_default()
}

fn clean(s: impl Into<String>) -> Option<String> {
    let s = s.into().trim().to_owned();
    // Firmware fills unset fields with placeholders.
    let junk = ["", "To be filled by O.E.M.", "To Be Filled By O.E.M.", "Default string", "System Product Name", "System manufacturer", "Not Specified", "None", "0"];
    (!junk.contains(&s.as_str())).then_some(s)
}

fn collect_blocking() -> Inventory {
    let mut inv = Inventory {
        collected_at: crate::now(),
        hostname: System::host_name().ok().and_then(clean),
        os: OsInfo {
            name: System::name().ok().and_then(clean),
            version: System::os_version().ok().and_then(clean),
            build: None,
            kernel: System::kernel_version().ok().and_then(clean),
            arch: clean(System::cpu_arch()),
        },
        uptime_secs: System::uptime().ok(),
        last_boot: System::boot_time().ok().map(|t| t as i64),
        hardware: Hardware {
            manufacturer: Product::vendor_name().ok().and_then(clean),
            model: Product::name().ok().and_then(clean),
            serial: Product::serial_number().ok().and_then(clean),
        },
        ..Default::default()
    };

    if let Ok(mut sys) = System::new() {
        sys.refresh_memory();
        sys.refresh_cpu_all();
        let cpus = sys.cpus();
        inv.cpu = Cpu {
            model: cpus.first().and_then(|c| clean(c.brand())),
            cores: System::physical_core_count().ok().map(|n| n as u32),
            threads: (!cpus.is_empty()).then_some(cpus.len() as u32),
        };
        inv.memory = Memory {
            total: Some(sys.total_memory()).filter(|&n| n > 0),
            used: Some(sys.used_memory()).filter(|_| sys.total_memory() > 0),
        };
    }

    if let Ok(disks) = Disks::new_with_refreshed_list() {
        for d in disks.iter() {
            let fs = d.file_system().to_string_lossy().into_owned();
            let mount = d.mount_point().to_string_lossy().into_owned();
            // Snap images, overlays and RAM disks aren't storage anyone asks about.
            if ["squashfs", "tmpfs", "overlay", "devtmpfs", "ramfs"].contains(&fs.as_str())
                || mount.starts_with("/snap/")
                || d.total_space() == 0
            {
                continue;
            }
            inv.disks.push(Disk { mount, fs: clean(fs), total: d.total_space(), free: d.available_space() });
        }
        inv.disks.sort_by(|a, b| a.mount.cmp(&b.mount));
        inv.disks.dedup_by(|a, b| a.mount == b.mount);
    }

    if let Ok(nets) = Networks::new_with_refreshed_list() {
        for (name, data) in nets.iter() {
            let ips: Vec<String> = data
                .ip_networks()
                .iter()
                .filter(|n| !n.addr.is_loopback())
                .map(|n| format!("{}/{}", n.addr, n.prefix))
                .collect();
            let mac = data.mac_address().to_string();
            let name = name.to_string_lossy().into_owned();
            if ips.is_empty() && name == "lo" {
                continue;
            }
            inv.network.push(Nic {
                name,
                mac: (mac != "00:00:00:00:00:00").then_some(mac),
                ips,
            });
        }
        inv.network.sort_by(|a, b| b.ips.len().cmp(&a.ips.len()).then(a.name.cmp(&b.name)));
    }

    platform::fill(&mut inv);
    inv.software.sort_by_key(|s| s.name.to_lowercase());
    inv
}

#[cfg(target_os = "linux")]
mod platform {
    use super::clean;
    use deskhand_proto::{Encryption, Firewall, Inventory, Software};
    use std::path::Path;
    use std::process::Command;

    pub fn fill(inv: &mut Inventory) {
        inv.logged_in_users = utmp_users();
        inv.domain = fqdn_domain().map(|d| format!("domain: {d}"));
        // On Linux "version" is the distro release and "build" the kernel.
        inv.os.build = inv.os.kernel.clone();
        if inv.hardware.serial.is_none() {
            inv.hardware.serial = read("/sys/class/dmi/id/product_serial").and_then(clean);
        }
        inv.software = dpkg().or_else(rpm).unwrap_or_default();
        inv.security.encryption = Some(luks());
        inv.security.firewall = firewall();
        inv.security.pending_reboot = if Path::new("/var/run/reboot-required").exists() {
            Some(true)
        } else if Path::new("/var/lib/dpkg").exists() {
            // Debian and Ubuntu flag it with that file; elsewhere we can't tell.
            Some(false)
        } else {
            None
        };
    }

    fn read(p: &str) -> Option<String> {
        std::fs::read_to_string(p).ok()
    }

    fn run(cmd: &str, args: &[&str]) -> Option<String> {
        let out = Command::new(cmd).args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Users with a login session, from utmp (glibc's x86_64/aarch64 layout: 384-byte
    /// records, `ut_type` at 0, `ut_user` at 44).
    fn utmp_users() -> Vec<String> {
        let Ok(b) = std::fs::read("/run/utmp").or_else(|_| std::fs::read("/var/run/utmp")) else {
            return Vec::new();
        };
        let mut users: Vec<String> = b
            .chunks_exact(384)
            .filter(|r| i16::from_ne_bytes([r[0], r[1]]) == 7) // USER_PROCESS
            .filter_map(|r| {
                let u = &r[44..76];
                let end = u.iter().position(|&c| c == 0).unwrap_or(u.len());
                clean(String::from_utf8_lossy(&u[..end]).into_owned())
            })
            .collect();
        users.sort();
        users.dedup();
        users
    }

    fn fqdn_domain() -> Option<String> {
        let fqdn = run("hostname", &["-f"])?;
        let (_, domain) = fqdn.trim().split_once('.')?;
        clean(domain.to_owned())
    }

    /// Installed packages from dpkg's status file.
    fn dpkg() -> Option<Vec<Software>> {
        let status = read("/var/lib/dpkg/status")?;
        let mut out = Vec::new();
        for para in status.split("\n\n") {
            let mut sw = Software::default();
            let mut installed = false;
            for line in para.lines() {
                if let Some((k, v)) = line.split_once(": ") {
                    match k {
                        "Package" => sw.name = v.to_owned(),
                        "Version" => sw.version = clean(v),
                        "Maintainer" => sw.publisher = clean(v),
                        "Status" => installed = v.ends_with(" installed"),
                        _ => {}
                    }
                }
            }
            if installed && !sw.name.is_empty() {
                out.push(sw);
            }
        }
        Some(out)
    }

    fn rpm() -> Option<Vec<Software>> {
        let list = run("rpm", &["-qa", "--queryformat", "%{NAME}\\t%{VERSION}-%{RELEASE}\\t%{VENDOR}\\t%{INSTALLTIME}\\n"])?;
        Some(
            list.lines()
                .filter_map(|l| {
                    let mut f = l.split('\t');
                    let name = f.next()?.to_owned();
                    Some(Software {
                        name,
                        version: f.next().and_then(clean),
                        publisher: f.next().filter(|v| *v != "(none)").and_then(clean),
                        install_date: f.next().and_then(|t| t.parse::<i64>().ok()).map(crate::iso_date),
                    })
                })
                .collect(),
        )
    }

    /// Device-mapper targets set up by cryptsetup (their uuid starts with CRYPT-).
    fn luks() -> Vec<Encryption> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir("/sys/block") else {
            return out;
        };
        for e in dir.flatten() {
            let p = e.path();
            let uuid = std::fs::read_to_string(p.join("dm/uuid")).unwrap_or_default();
            if let Some(kind) = uuid.strip_prefix("CRYPT-") {
                let name = std::fs::read_to_string(p.join("dm/name")).unwrap_or_default();
                let kind = kind.split('-').next().unwrap_or("crypt").to_ascii_lowercase();
                out.push(Encryption { volume: name.trim().to_owned(), status: kind });
            }
        }
        out.sort_by(|a, b| a.volume.cmp(&b.volume));
        out
    }

    fn firewall() -> Vec<Firewall> {
        let mut out = Vec::new();
        if let Some(conf) = read("/etc/ufw/ufw.conf") {
            let on = conf.lines().any(|l| l.trim().eq_ignore_ascii_case("ENABLED=yes"));
            out.push(Firewall { profile: "ufw".into(), enabled: Some(on) });
        }
        if Path::new("/usr/lib/systemd/system/firewalld.service").exists()
            || Path::new("/lib/systemd/system/firewalld.service").exists()
        {
            let on = run("systemctl", &["is-active", "firewalld"]).is_some_and(|s| s.trim() == "active");
            out.push(Firewall { profile: "firewalld".into(), enabled: Some(on) });
        }
        out
    }
}

#[cfg(windows)]
mod platform {
    use super::clean;
    use deskhand_proto::{Antivirus, Encryption, Firewall, Inventory, Software};
    use serde::Deserialize;
    use winreg::enums::{HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
    use winreg::RegKey;
    use wmi::WMIConnection;

    pub fn fill(inv: &mut Inventory) {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        os(&hklm, inv);
        inv.logged_in_users = sessions();
        inv.software = software();
        inv.security.firewall = firewall(&hklm);
        inv.security.pending_reboot = Some(pending_reboot(&hklm));
        if let Ok(wmi) = WMIConnection::new() {
            computer_system(&wmi, inv);
        }
        inv.security.encryption = bitlocker();
        inv.security.antivirus = defender();
    }

    /// Edition, display version and build from the registry: sysinfo's name is just
    /// "Windows".
    fn os(hklm: &RegKey, inv: &mut Inventory) {
        let Ok(k) = hklm.open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", KEY_READ | KEY_WOW64_64KEY) else {
            return;
        };
        let get = |n: &str| k.get_value::<String, _>(n).ok().and_then(clean);
        let build = get("CurrentBuildNumber").or_else(|| get("CurrentBuild"));
        let ubr = k.get_value::<u32, _>("UBR").ok();
        if let Some(mut name) = get("ProductName") {
            // Windows 11 still says "Windows 10" in ProductName; the build tells them apart.
            if build.as_deref().and_then(|b| b.parse::<u32>().ok()).is_some_and(|b| b >= 22000) {
                name = name.replacen("Windows 10", "Windows 11", 1);
            }
            inv.os.name = Some(name);
        }
        inv.os.version = get("DisplayVersion").or_else(|| get("ReleaseId")).or(inv.os.version.take());
        inv.os.build = build.map(|b| match ubr {
            Some(u) => format!("{b}.{u}"),
            None => b,
        });
    }

    /// User names of the active and disconnected sessions.
    fn sessions() -> Vec<String> {
        use windows::Win32::System::RemoteDesktop::*;
        let mut users = Vec::new();
        unsafe {
            let mut list: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
            let mut count = 0u32;
            if WTSEnumerateSessionsW(Some(WTS_CURRENT_SERVER_HANDLE), 0, 1, &mut list, &mut count).is_err() {
                return users;
            }
            for s in std::slice::from_raw_parts(list, count as usize) {
                let mut buf = windows::core::PWSTR::null();
                let mut len = 0u32;
                if WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), s.SessionId, WTSUserName, &mut buf, &mut len).is_ok() {
                    if let Some(u) = buf.to_string().ok().and_then(clean) {
                        users.push(u);
                    }
                    WTSFreeMemory(buf.0 as _);
                }
            }
            WTSFreeMemory(list as _);
        }
        users.sort();
        users.dedup();
        users
    }

    /// Programs from the Uninstall keys: machine-wide (64 and 32-bit) and each loaded user.
    fn software() -> Vec<Software> {
        const PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
        let mut roots = Vec::new();
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        for flag in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            if let Ok(k) = hklm.open_subkey_with_flags(PATH, KEY_READ | flag) {
                roots.push(k);
            }
        }
        let hku = RegKey::predef(HKEY_USERS);
        for sid in hku.enum_keys().flatten() {
            if sid.ends_with("_Classes") {
                continue;
            }
            if let Ok(k) = hku.open_subkey_with_flags(format!(r"{sid}\{PATH}"), KEY_READ) {
                roots.push(k);
            }
        }
        let mut out: Vec<Software> = Vec::new();
        for root in roots {
            for name in root.enum_keys().flatten() {
                let Ok(k) = root.open_subkey_with_flags(&name, KEY_READ) else { continue };
                let get = |n: &str| k.get_value::<String, _>(n).ok().and_then(clean);
                let Some(display) = get("DisplayName") else { continue };
                // Updates and components that Programs and Features hides.
                if k.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 || get("ParentKeyName").is_some() {
                    continue;
                }
                let sw = Software {
                    name: display,
                    version: get("DisplayVersion"),
                    publisher: get("Publisher"),
                    install_date: get("InstallDate"),
                };
                if !out.iter().any(|o| o.name == sw.name && o.version == sw.version) {
                    out.push(sw);
                }
            }
        }
        out
    }

    fn firewall(hklm: &RegKey) -> Vec<deskhand_proto::Firewall> {
        const BASE: &str = r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy";
        [("Domain", "DomainProfile"), ("Private", "StandardProfile"), ("Public", "PublicProfile")]
            .into_iter()
            .map(|(label, key)| Firewall {
                profile: label.into(),
                enabled: hklm
                    .open_subkey_with_flags(format!(r"{BASE}\{key}"), KEY_READ)
                    .and_then(|k| k.get_value::<u32, _>("EnableFirewall"))
                    .ok()
                    .map(|v| v != 0),
            })
            .collect()
    }

    fn pending_reboot(hklm: &RegKey) -> bool {
        let exists = |p: &str| hklm.open_subkey_with_flags(p, KEY_READ | KEY_WOW64_64KEY).is_ok();
        exists(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending")
            || exists(r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired")
            || hklm
                .open_subkey_with_flags(r"SYSTEM\CurrentControlSet\Control\Session Manager", KEY_READ)
                .and_then(|k| k.get_raw_value("PendingFileRenameOperations"))
                .is_ok_and(|v| v.bytes.len() > 4)
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct ComputerSystem {
        manufacturer: Option<String>,
        model: Option<String>,
        domain: Option<String>,
        part_of_domain: Option<bool>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Bios {
        serial_number: Option<String>,
    }

    fn computer_system(wmi: &WMIConnection, inv: &mut Inventory) {
        if let Ok(cs) = wmi.raw_query::<ComputerSystem>("SELECT Manufacturer, Model, Domain, PartOfDomain FROM Win32_ComputerSystem") {
            if let Some(cs) = cs.into_iter().next() {
                if inv.hardware.manufacturer.is_none() {
                    inv.hardware.manufacturer = cs.manufacturer.and_then(clean);
                }
                if inv.hardware.model.is_none() {
                    inv.hardware.model = cs.model.and_then(clean);
                }
                inv.domain = cs.domain.and_then(clean).map(|d| match cs.part_of_domain {
                    Some(true) => format!("domain: {d}"),
                    _ => format!("workgroup: {d}"),
                });
            }
        }
        if inv.hardware.serial.is_none() {
            if let Ok(b) = wmi.raw_query::<Bios>("SELECT SerialNumber FROM Win32_BIOS") {
                inv.hardware.serial = b.into_iter().next().and_then(|b| b.serial_number).and_then(clean);
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct EncryptableVolume {
        drive_letter: Option<String>,
        protection_status: Option<u32>,
    }

    /// `None` without admin: the BitLocker WMI namespace refuses everyone else.
    fn bitlocker() -> Option<Vec<Encryption>> {
        let wmi = WMIConnection::with_namespace_path(r"ROOT\CIMV2\Security\MicrosoftVolumeEncryption").ok()?;
        let vols = wmi
            .raw_query::<EncryptableVolume>("SELECT DriveLetter, ProtectionStatus FROM Win32_EncryptableVolume")
            .ok()?;
        Some(
            vols.into_iter()
                .filter_map(|v| {
                    Some(Encryption {
                        volume: v.drive_letter.and_then(clean)?,
                        status: match v.protection_status {
                            Some(1) => "on",
                            Some(0) => "off",
                            _ => "unknown",
                        }
                        .into(),
                    })
                })
                .collect(),
        )
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct MpStatus {
        antivirus_enabled: Option<bool>,
        real_time_protection_enabled: Option<bool>,
        antivirus_signature_age: Option<u32>,
    }

    fn defender() -> Option<Antivirus> {
        let wmi = WMIConnection::with_namespace_path(r"ROOT\Microsoft\Windows\Defender").ok()?;
        let s = wmi
            .raw_query::<MpStatus>("SELECT AntivirusEnabled, RealTimeProtectionEnabled, AntivirusSignatureAge FROM MSFT_MpComputerStatus")
            .ok()?
            .into_iter()
            .next()?;
        Some(Antivirus {
            name: "Microsoft Defender".into(),
            enabled: s.antivirus_enabled,
            realtime: s.real_time_protection_enabled,
            signature_age_days: s.antivirus_signature_age,
        })
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    pub fn fill(_: &mut deskhand_proto::Inventory) {}
}

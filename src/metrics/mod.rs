// SPDX-License-Identifier: GPL-3.0-only
//
// Sammelt alle Metriken ausschließlich aus /proc und /sys — niemals über einen
// Subprozess. Zustand (vorherige CPU-/Netz-Samples) lebt im `Collector`, damit
// Deltas/Raten berechnet werden können.

pub mod gpu;

use crate::hw;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

/// Momentaufnahme aller Werte (wird vom UI gerendert).
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub cpu_pct: f32,
    pub per_core: Vec<f32>,
    pub mem_used_kb: u64,
    pub mem_total_kb: u64,
    pub cpu_temp_c: Option<f32>,
    pub ram_temp_c: Option<f32>,
    pub fans_rpm: Vec<u32>,
    pub net_down_bps: f64,
    pub net_up_bps: f64,
    pub net_iface: Option<String>,
    /// Typ der aktiven Schnittstelle (WLAN/LAN/VPN) — lesbarer als der rohe Iface-Name.
    pub net_kind: Option<&'static str>,
    pub gpu: gpu::GpuInfo,
}

#[derive(Debug, Clone, Copy)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

/// Hält Zustand zwischen den Ticks.
pub struct Collector {
    prev_cpu: Option<CpuTimes>,
    prev_core: Vec<CpuTimes>,
    prev_net: Option<(u64, u64, Instant)>,
    iface: Option<String>,
    gpu: gpu::GpuReader,
    /// Wird beim Suspend gesetzt (Reserve für logind-Integration); pausiert GPU-Reads.
    pub paused: bool,
}

impl Collector {
    pub fn new() -> Self {
        Self {
            prev_cpu: None,
            prev_core: Vec::new(),
            prev_net: None,
            iface: None,
            gpu: gpu::GpuReader::new(),
            paused: false,
        }
    }

    /// `gpu_live`: ob GPU-Live-Werte (NVML) gelesen werden sollen — nur bei offenem Popup.
    /// Sonst bleibt die dGPU unangetastet (kein Pin, kein Wecken).
    pub fn refresh(&mut self, gpu_live: bool) -> Metrics {
        let mut m = Metrics::default();

        // --- CPU ---
        if let Some((agg, cores)) = read_cpu_times() {
            if let Some(prev) = self.prev_cpu {
                m.cpu_pct = cpu_usage(prev, agg);
            }
            if self.prev_core.len() == cores.len() {
                m.per_core = cores
                    .iter()
                    .zip(&self.prev_core)
                    .map(|(c, p)| cpu_usage(*p, *c))
                    .collect();
            }
            self.prev_cpu = Some(agg);
            self.prev_core = cores;
        }

        // --- RAM ---
        if let Some((total, avail)) = read_meminfo() {
            m.mem_total_kb = total;
            m.mem_used_kb = total.saturating_sub(avail);
        }

        // --- Temperaturen / Lüfter ---
        m.cpu_temp_c = cpu_temp();
        m.ram_temp_c =
            hwmon_by_name(hw::RAM_TEMP_CHIP).and_then(|p| read_milli_c(&p.join("temp1_input")));
        m.fans_rpm = read_fans();

        // --- Netz ---
        // Default-Interface bei jedem Tick neu bestimmen (WLAN↔LAN↔VPN-Wechsel);
        // bei Wechsel das vorherige Sample verwerfen, damit keine Delta-Sprünge
        // zwischen zwei verschiedenen Interfaces entstehen.
        let cur_iface = default_iface();
        if cur_iface != self.iface {
            self.prev_net = None;
            self.iface = cur_iface;
        }
        m.net_iface = self.iface.clone();
        m.net_kind = self.iface.as_deref().map(iface_kind);
        if let Some(iface) = &self.iface {
            if let Some((rx, tx)) = read_net_bytes(iface) {
                let now = Instant::now();
                if let Some((prx, ptx, pt)) = self.prev_net {
                    let dt = now.duration_since(pt).as_secs_f64();
                    if dt > 0.0 {
                        m.net_down_bps = rx.saturating_sub(prx) as f64 / dt;
                        m.net_up_bps = tx.saturating_sub(ptx) as f64 / dt;
                    }
                }
                self.prev_net = Some((rx, tx, now));
            }
        }

        // --- GPU (NVML nur wenn dGPU wach, Live gewünscht & nicht pausiert) ---
        m.gpu = self.gpu.read(gpu_live && !self.paused);

        m
    }
}

/// Typ der Schnittstelle anhand sysfs/Namenskonvention — pin-/wake-frei.
/// WLAN (`/sys/class/net/<if>/wireless`), VPN (tun/tap/wg/ppp), sonst LAN.
fn iface_kind(name: &str) -> &'static str {
    if std::path::Path::new(&format!("{}/{name}/wireless", hw::SYS_CLASS_NET)).exists() {
        "WLAN"
    } else if hw::VPN_IFACE_PREFIXES.iter().any(|p| name.starts_with(p)) {
        "VPN"
    } else {
        "LAN"
    }
}

fn cpu_usage(prev: CpuTimes, cur: CpuTimes) -> f32 {
    let dt = cur.total.saturating_sub(prev.total);
    let di = cur.idle.saturating_sub(prev.idle);
    if dt == 0 {
        0.0
    } else {
        ((dt.saturating_sub(di)) as f32 / dt as f32) * 100.0
    }
}

fn read_cpu_times() -> Option<(CpuTimes, Vec<CpuTimes>)> {
    let data = fs::read_to_string(hw::PROC_STAT).ok()?;
    let mut agg = None;
    let mut cores = Vec::new();
    for line in data.lines() {
        if !line.starts_with("cpu") {
            continue;
        }
        let mut it = line.split_whitespace();
        let tag = it.next()?;
        let nums: Vec<u64> = it.filter_map(|x| x.parse::<u64>().ok()).collect();
        if nums.len() < 5 {
            continue;
        }
        // Nur die ersten 8 Felder (user..steal) summieren: guest/guest_nice (Feld 9/10)
        // sind laut Kernel bereits in user/nice enthalten → sonst Doppelzählung.
        let total: u64 = nums.iter().take(8).sum();
        let idle = nums[3] + nums[4]; // idle + iowait
        let t = CpuTimes { total, idle };
        if tag == "cpu" {
            agg = Some(t);
        } else {
            cores.push(t);
        }
    }
    Some((agg?, cores))
}

fn read_meminfo() -> Option<(u64, u64)> {
    let data = fs::read_to_string(hw::PROC_MEMINFO).ok()?;
    let mut total = None;
    let mut avail = None;
    for line in data.lines() {
        if let Some(v) = line.strip_prefix("MemTotal:") {
            total = v.split_whitespace().next().and_then(|x| x.parse().ok());
        } else if let Some(v) = line.strip_prefix("MemAvailable:") {
            avail = v.split_whitespace().next().and_then(|x| x.parse().ok());
        }
    }
    Some((total?, avail?))
}

/// Interface der Default-Route aus /proc/net/route (Destination == 00000000).
fn default_iface() -> Option<String> {
    let data = fs::read_to_string(hw::PROC_NET_ROUTE).ok()?;
    for line in data.lines().skip(1) {
        let mut f = line.split_whitespace();
        let iface = f.next()?;
        let dest = f.next()?;
        if dest == "00000000" {
            return Some(iface.to_string());
        }
    }
    None
}

fn read_net_bytes(iface: &str) -> Option<(u64, u64)> {
    let base = PathBuf::from(hw::SYS_CLASS_NET).join(iface).join("statistics");
    let rx = read_u64(&base.join("rx_bytes"))?;
    let tx = read_u64(&base.join("tx_bytes"))?;
    Some((rx, tx))
}

// ---- hwmon-Helfer ----

fn hwmon_by_name(name: &str) -> Option<PathBuf> {
    for e in fs::read_dir(hw::SYS_CLASS_HWMON).ok()?.flatten() {
        let p = e.path();
        if let Ok(n) = fs::read_to_string(p.join("name")) {
            if n.trim() == name {
                return Some(p);
            }
        }
    }
    None
}

fn cpu_temp() -> Option<f32> {
    // Bekannte (Chip, Label)-Paare der Reihe nach versuchen (Intel coretemp, AMD k10temp, …).
    for (chip, label) in hw::CPU_TEMP_LABELED {
        if let Some(base) = hwmon_by_name(chip) {
            if let Some(t) = label_input(&base, label) {
                return Some(t);
            }
        }
    }
    // Fallback-Chip (dessen temp1_input).
    if let Some(base) = hwmon_by_name(hw::CPU_TEMP_FALLBACK_CHIP) {
        if let Some(t) = read_milli_c(&base.join("temp1_input")) {
            return Some(t);
        }
    }
    None
}

/// Sucht in einem hwmon-Verzeichnis das temp*_label == `want` und liest dessen _input.
fn label_input(base: &PathBuf, want: &str) -> Option<f32> {
    for e in fs::read_dir(base).ok()?.flatten() {
        let p = e.path();
        let fname = p.file_name()?.to_str()?.to_string();
        if fname.ends_with("_label") {
            if let Ok(lbl) = fs::read_to_string(&p) {
                if lbl.trim() == want {
                    let input = base.join(fname.replace("_label", "_input"));
                    return read_milli_c(&input);
                }
            }
        }
    }
    None
}

fn read_fans() -> Vec<u32> {
    let mut out = Vec::new();
    if let Some(base) = hwmon_by_name(hw::FAN_CHIP) {
        for n in 1..=hw::FAN_MAX_INDEX {
            if let Some(rpm) = read_u64(&base.join(format!("fan{n}_input"))) {
                if rpm > 0 {
                    out.push(rpm as u32);
                }
            }
        }
    }
    out
}

fn read_u64(p: &PathBuf) -> Option<u64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn read_milli_c(p: &PathBuf) -> Option<f32> {
    read_u64(p).map(|v| v as f32 / 1000.0)
}

// SPDX-License-Identifier: GPL-3.0-only
//
// Sammelt alle Metriken ausschließlich aus /proc und /sys — niemals über einen
// Subprozess. Zustand (vorherige CPU-/Netz-Samples) lebt im `Collector`, damit
// Deltas/Raten berechnet werden können.
//
// Sparsamkeit pro Tick: hwmon-/Akku-Pfade werden einmal aufgelöst und gecacht
// (`SensorPaths`, `PowerReader::bats`); Momentanwerte, die nur im Popup sichtbar
// sind (Temperaturen, Lüfter, per-Core-Prozente, PRIME-Modus), werden bei
// geschlossenem Popup gar nicht erst erhoben. Delta-Metriken (CPU, Netz, RAPL)
// laufen immer, sonst gäbe es Sprünge beim Popup-Öffnen.

pub mod gpu;
pub mod power;

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
    /// Kumulierte RX/TX-Bytes der aktiven Schnittstelle seit Boot (aus den
    /// ohnehin gelesenen Zählern — kein zusätzlicher Read).
    pub net_total_rx: u64,
    pub net_total_tx: u64,
    /// Swap belegt/gesamt in kB (0/0 = kein Swap eingerichtet).
    pub swap_used_kb: u64,
    pub swap_total_kb: u64,
    /// Load Average 1/5/15 min (nur bei offenem Popup gelesen).
    pub loadavg: Option<(f32, f32, f32)>,
    /// Uptime in Sekunden (nur bei offenem Popup gelesen).
    pub uptime_s: Option<u64>,
    /// Disk-Lese-/Schreibrate in Bytes/s, summiert über physische Laufwerke.
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub gpu: gpu::GpuInfo,
    pub power: power::PowerInfo,
}

#[derive(Debug, Clone, Copy)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

/// Einmal aufgelöste hwmon-Pfade (Verzeichnis-Scans sind teuer und liefen
/// früher mehrfach pro Tick). Lesefehler invalidieren den jeweiligen Pfad;
/// nicht gefundene Chips werden erst beim nächsten Popup-Öffnen erneut gesucht.
struct SensorPaths {
    /// Fertig aufgelöster CPU-Temp-`_input`-Pfad (inkl. Label-Suche).
    cpu_temp_input: Option<PathBuf>,
    ram_temp_input: Option<PathBuf>,
    /// hwmon-Basisverzeichnis des Lüfter-Chips.
    fan_base: Option<PathBuf>,
    resolved: bool,
}

/// Hält Zustand zwischen den Ticks.
pub struct Collector {
    prev_cpu: Option<CpuTimes>,
    prev_core: Vec<CpuTimes>,
    prev_net: Option<(u64, u64, Instant)>,
    /// Voriges Disk-Sample (Sektoren gelesen/geschrieben) für die Delta-Rate.
    prev_disk: Option<(u64, u64, Instant)>,
    iface: Option<String>,
    sensors: SensorPaths,
    /// `live` des vorherigen Ticks — für den Sensor-Retry bei Popup-Öffnung.
    was_live: bool,
    gpu: gpu::GpuReader,
    power: power::PowerReader,
    /// Wird beim Suspend gesetzt (Reserve für logind-Integration); pausiert GPU-Reads.
    pub paused: bool,
}

impl Collector {
    pub fn new() -> Self {
        Self {
            prev_cpu: None,
            prev_core: Vec::new(),
            prev_net: None,
            prev_disk: None,
            iface: None,
            sensors: SensorPaths {
                cpu_temp_input: None,
                ram_temp_input: None,
                fan_base: None,
                resolved: false,
            },
            was_live: false,
            gpu: gpu::GpuReader::new(),
            power: power::PowerReader::new(),
            paused: false,
        }
    }

    /// `live`: Popup offen. Gated GPU-Live-Werte (NVML — dGPU bleibt sonst
    /// unangetastet, kein Pin, kein Wecken) und die nur im Popup sichtbaren
    /// Momentanwerte (Temperaturen, Lüfter). Delta-Metriken (CPU, Netz, RAPL)
    /// laufen immer, damit beim Öffnen keine Sprünge entstehen.
    pub fn refresh(&mut self, live: bool) -> Metrics {
        let mut m = Metrics::default();

        // --- CPU ---
        if let Some((agg, cores)) = read_cpu_times() {
            if let Some(prev) = self.prev_cpu {
                m.cpu_pct = cpu_usage(prev, agg);
            }
            // Prozente nur bei offenem Popup berechnen; `prev_core` bleibt
            // immer aktuell, damit die Deltas beim Öffnen sofort stimmen.
            if live && self.prev_core.len() == cores.len() {
                m.per_core = cores
                    .iter()
                    .zip(&self.prev_core)
                    .map(|(c, p)| cpu_usage(*p, *c))
                    .collect();
            }
            self.prev_cpu = Some(agg);
            self.prev_core = cores;
        }

        // --- RAM / Swap ---
        if let Some(mi) = read_meminfo() {
            m.mem_total_kb = mi.total;
            m.mem_used_kb = mi.total.saturating_sub(mi.avail);
            m.swap_total_kb = mi.swap_total;
            m.swap_used_kb = mi.swap_total.saturating_sub(mi.swap_free);
        }

        // --- Temperaturen / Lüfter (nur im Popup sichtbar) ---
        if live {
            // Beim Popup-Öffnen fehlende Sensoren erneut suchen (Modul-Nachladen).
            if !self.was_live {
                self.sensors.resolved = false;
            }
            if !self.sensors.resolved {
                self.sensors.resolve();
            }
            m.cpu_temp_c = self.sensors.cpu_temp();
            m.ram_temp_c = self.sensors.ram_temp();
            m.fans_rpm = self.sensors.fans();
            // Momentanwerte ohne Delta-Zustand — nur fürs Popup nötig.
            m.loadavg = read_loadavg();
            m.uptime_s = read_uptime_s();
        }
        self.was_live = live;

        // --- Disk-I/O (Delta wie Netz: läuft immer, sonst Sprung beim Öffnen) ---
        if let Some((rd, wr)) = read_disk_sectors() {
            let now = Instant::now();
            if let Some((prd, pwr, pt)) = self.prev_disk {
                let dt = now.duration_since(pt).as_secs_f64();
                if dt > 0.0 {
                    m.disk_read_bps =
                        rd.saturating_sub(prd) as f64 * hw::DISK_SECTOR_BYTES as f64 / dt;
                    m.disk_write_bps =
                        wr.saturating_sub(pwr) as f64 * hw::DISK_SECTOR_BYTES as f64 / dt;
                }
            }
            self.prev_disk = Some((rd, wr, now));
        }

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
                // Kumulierte Totals seit Boot — dieselben Zähler wie für die Rate.
                m.net_total_rx = rx;
                m.net_total_tx = tx;
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

        // --- Leistung (RAPL + Akku) ---
        // Nach Suspend Zählerzustand verwerfen, damit kein Delta über die Schlafphase entsteht.
        if self.paused {
            self.power.reset();
            // Auch Netz-/Disk-Samples: deren Deltas haben keine Sanity-Klammer wie RAPL.
            self.prev_net = None;
            self.prev_disk = None;
        }
        m.power = self.power.read();

        // --- GPU (NVML nur wenn dGPU wach, Live gewünscht & nicht pausiert) ---
        m.gpu = self.gpu.read(live && !self.paused);

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
        // Felder direkt aufsummieren statt in ein Vec zu sammeln (läuft jeden Tick).
        // Nur die ersten 8 Felder (user..steal) summieren: guest/guest_nice (Feld 9/10)
        // sind laut Kernel bereits in user/nice enthalten → sonst Doppelzählung.
        let mut total: u64 = 0;
        let mut idle: u64 = 0;
        let mut n = 0usize;
        for x in it.filter_map(|x| x.parse::<u64>().ok()).take(8) {
            if n == 3 || n == 4 {
                idle += x; // idle + iowait
            }
            total += x;
            n += 1;
        }
        if n < 5 {
            continue;
        }
        let t = CpuTimes { total, idle };
        if tag == "cpu" {
            agg = Some(t);
        } else {
            cores.push(t);
        }
    }
    Some((agg?, cores))
}

/// RAM- und Swap-Zahlen aus /proc/meminfo (alle in kB).
struct MemInfo {
    total: u64,
    avail: u64,
    swap_total: u64,
    swap_free: u64,
}

fn read_meminfo() -> Option<MemInfo> {
    let data = fs::read_to_string(hw::PROC_MEMINFO).ok()?;
    let mut total = None;
    let mut avail = None;
    let mut swap_total = 0;
    let mut swap_free = 0;
    let kb = |v: &str| v.split_whitespace().next().and_then(|x| x.parse().ok());
    for line in data.lines() {
        if let Some(v) = line.strip_prefix("MemTotal:") {
            total = kb(v);
        } else if let Some(v) = line.strip_prefix("MemAvailable:") {
            avail = kb(v);
        } else if let Some(v) = line.strip_prefix("SwapTotal:") {
            swap_total = kb(v).unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("SwapFree:") {
            swap_free = kb(v).unwrap_or(0);
        }
    }
    Some(MemInfo {
        total: total?,
        avail: avail?,
        swap_total,
        swap_free,
    })
}

/// Load Average 1/5/15 min aus /proc/loadavg.
fn read_loadavg() -> Option<(f32, f32, f32)> {
    let data = fs::read_to_string(hw::PROC_LOADAVG).ok()?;
    let mut it = data.split_whitespace();
    Some((
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    ))
}

/// Uptime in ganzen Sekunden aus /proc/uptime.
fn read_uptime_s() -> Option<u64> {
    let data = fs::read_to_string(hw::PROC_UPTIME).ok()?;
    data.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|s| s as u64)
}

/// Summe (Sektoren gelesen, Sektoren geschrieben) über physische Laufwerke aus
/// /proc/diskstats. Partitionen werden übersprungen, sonst zählt alles doppelt.
fn read_disk_sectors() -> Option<(u64, u64)> {
    let data = fs::read_to_string(hw::PROC_DISKSTATS).ok()?;
    let mut rd = 0u64;
    let mut wr = 0u64;
    for line in data.lines() {
        let mut f = line.split_whitespace();
        // Felder: major minor name reads reads_merged sectors_read ms_read
        //         writes writes_merged sectors_written …
        let name = f.nth(2)?;
        // Virtuelle/gestapelte Devices (loop, ram, zram, device-mapper, RAID)
        // überspringen: deren I/O erscheint bereits auf den darunterliegenden
        // physischen Laufwerken — mitzählen wäre Doppelzählung.
        if is_partition(name)
            || name.starts_with("loop")
            || name.starts_with("ram")
            || name.starts_with("zram")
            || name.starts_with("dm-")
            || name.starts_with("md")
        {
            continue;
        }
        let sectors_read: u64 = f.nth(2).and_then(|x| x.parse().ok())?;
        let sectors_written: u64 = f.nth(3).and_then(|x| x.parse().ok())?;
        rd += sectors_read;
        wr += sectors_written;
    }
    Some((rd, wr))
}

/// Partition erkennen: `sda1`, `nvme0n1p2`, `mmcblk0p1` — ganze Laufwerke
/// (`sda`, `nvme0n1`, `mmcblk0`) zählen, ihre Partitionen nicht.
fn is_partition(name: &str) -> bool {
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        // nvme0n1p2 / mmcblk0p1 → enthält 'p' nach dem Grundnamen.
        name.contains('p') && name.rsplit('p').next().is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty())
    } else {
        // sda1, vdb2, … → endet auf Ziffer.
        name.ends_with(|c: char| c.is_ascii_digit())
    }
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

// ---- hwmon-Helfer (Pfade werden in `SensorPaths` gecacht) ----

impl SensorPaths {
    /// Löst alle Sensor-Pfade in EINEM `/sys/class/hwmon`-Scan auf (früher
    /// 3–5 Scans pro Tick). Läuft nur beim Popup-Öffnen bzw. nach Invalidierung.
    fn resolve(&mut self) {
        self.cpu_temp_input = None;
        self.ram_temp_input = None;
        self.fan_base = None;
        let mut cpu_fallback: Option<PathBuf> = None;
        // Bester Treffer gewinnt: kleinster Index in CPU_TEMP_LABELED.
        let mut cpu_rank = usize::MAX;

        if let Ok(dir) = fs::read_dir(hw::SYS_CLASS_HWMON) {
            for e in dir.flatten() {
                let p = e.path();
                let Ok(name) = fs::read_to_string(p.join("name")) else {
                    continue;
                };
                let name = name.trim();
                for (rank, (chip, label)) in hw::CPU_TEMP_LABELED.iter().enumerate() {
                    if name == *chip && rank < cpu_rank {
                        if let Some(input) = label_input_path(&p, label) {
                            self.cpu_temp_input = Some(input);
                            cpu_rank = rank;
                        }
                    }
                }
                if name == hw::CPU_TEMP_FALLBACK_CHIP {
                    cpu_fallback = Some(p.join("temp1_input"));
                }
                if name == hw::RAM_TEMP_CHIP {
                    self.ram_temp_input = Some(p.join("temp1_input"));
                }
                if name == hw::FAN_CHIP {
                    self.fan_base = Some(p.clone());
                }
            }
        }
        if self.cpu_temp_input.is_none() {
            self.cpu_temp_input = cpu_fallback;
        }
        self.resolved = true;
    }

    fn cpu_temp(&mut self) -> Option<f32> {
        let t = self.cpu_temp_input.as_ref().and_then(|p| read_milli_c(p));
        if t.is_none() && self.cpu_temp_input.is_some() {
            // Pfad tot (Modul entladen / Index verschoben) → beim nächsten
            // Popup-Öffnen neu auflösen.
            self.cpu_temp_input = None;
        }
        t
    }

    fn ram_temp(&mut self) -> Option<f32> {
        let t = self.ram_temp_input.as_ref().and_then(|p| read_milli_c(p));
        if t.is_none() && self.ram_temp_input.is_some() {
            self.ram_temp_input = None;
        }
        t
    }

    fn fans(&mut self) -> Vec<u32> {
        let mut out = Vec::new();
        if let Some(base) = &self.fan_base {
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
}

/// Sucht in einem hwmon-Verzeichnis das temp*_label == `want` und gibt den
/// zugehörigen `_input`-Pfad zurück (nur bei lesbarem Wert).
fn label_input_path(base: &PathBuf, want: &str) -> Option<PathBuf> {
    for e in fs::read_dir(base).ok()?.flatten() {
        let p = e.path();
        let fname = p.file_name()?.to_str()?.to_string();
        if fname.ends_with("_label") {
            if let Ok(lbl) = fs::read_to_string(&p) {
                if lbl.trim() == want {
                    let input = base.join(fname.replace("_label", "_input"));
                    if read_milli_c(&input).is_some() {
                        return Some(input);
                    }
                }
            }
        }
    }
    None
}

fn read_u64(p: &PathBuf) -> Option<u64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn read_milli_c(p: &PathBuf) -> Option<f32> {
    read_u64(p).map(|v| v as f32 / 1000.0)
}

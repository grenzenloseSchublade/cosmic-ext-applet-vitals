// SPDX-License-Identifier: GPL-3.0-only
//
// Leistungsaufnahme: RAPL (Intel "Running Average Power Limit") + Akku.
//
// RAPL liefert Energie-Zähler (µJ) unter /sys/class/powercap — Leistung ist das
// Zählerdelta pro Zeit. `energy_uj` ist seit Kernel 5.10 nur für root lesbar
// (Mitigation für CVE-2020-8694/PLATYPUS); ohne die optionale udev-Regel
// (siehe README, `just install-rapl-rule`) bleiben die RAPL-Zonen einfach leer.
// Die Lesbarkeit wird EINMAL beim Start geprobt — nach Installation der Regel
// muss das Applet neu gestartet werden.
//
// Der Akku (power_now/voltage_now, frei lesbar) dient als Fallback: im
// Akkubetrieb entspricht die Entladeleistung dem Gesamtverbrauch des Systems.
// Am Netz misst er nur die Laderate — dann wird bewusst nichts als
// Gesamtwert ausgegeben (statt eines irreführenden Werts).

use crate::hw;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

/// Ladezustand laut `BAT*/status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BatStatus {
    Charging,
    Discharging,
    Full,
    #[default]
    Unknown,
}

/// Momentaufnahme der Leistungs-/Spannungswerte (Teil von `Metrics`).
#[derive(Debug, Clone, Default)]
pub struct PowerInfo {
    /// Gesamtverbrauch des Systems in W: RAPL-`psys` wenn lesbar, sonst
    /// Akku-Entladeleistung (nur im Akkubetrieb aussagekräftig).
    pub sys_w: Option<f32>,
    /// `sys_w` stammt aus dem Akku-Fallback (kein psys): am Netz ist dann kein
    /// Gesamtwert verfügbar → UI zeigt „– · Netz".
    pub sys_from_battery: bool,
    /// CPU-Paket-Leistung (RAPL `package-0`, inkl. iGPU) in W.
    pub cpu_pkg_w: Option<f32>,
    /// Akku-Spannung in V.
    pub bat_voltage_v: Option<f32>,
    /// Akku-Lade-/Entladeleistung in W (Betrag; Richtung steckt in `bat_status`).
    pub bat_power_w: Option<f32>,
    pub bat_status: BatStatus,
    /// Näherung „Zug am Netzteil" in W: psys + Ladeleistung (nur beim Laden
    /// und mit lesbarem psys; Wandlerverluste nicht enthalten).
    pub charger_w: Option<f32>,
}

/// Eine RAPL-Zone mit Zählerzustand für die Delta-Bildung.
struct RaplZone {
    energy_path: PathBuf,
    /// Wrap-Grenze des Zählers (`max_energy_range_uj`).
    max_range_uj: u64,
    prev: Option<(u64, Instant)>,
}

impl RaplZone {
    /// Leistung seit dem letzten Aufruf in W. Erste Probe → `None`.
    /// Zähler-Wrap wird über `max_range_uj` behandelt; unplausible Werte
    /// (z. B. Resume-Spike: `Instant` friert im Suspend ein, der Zähler nicht)
    /// werden verworfen und der Zustand neu synchronisiert.
    fn read_watts(&mut self) -> Option<f32> {
        let cur = fs::read_to_string(&self.energy_path)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()?;
        let now = Instant::now();
        let prev = self.prev.replace((cur, now));
        let (p, pt) = prev?;
        let dt = now.duration_since(pt).as_secs_f64();
        if dt <= 0.0 {
            return None;
        }
        let delta_uj = if cur >= p {
            cur - p
        } else if self.max_range_uj > 0 {
            // Zähler-Überlauf: Rest bis zur Wrap-Grenze + neuer Stand.
            self.max_range_uj - p + cur
        } else {
            return None;
        };
        let watts = delta_uj as f64 / dt / 1e6;
        (watts.is_finite() && watts <= f64::from(hw::POWER_SANITY_MAX_W)).then(|| watts as f32)
    }
}

/// Hält die entdeckten RAPL-Zonen und die gecachten Akku-Pfade.
pub struct PowerReader {
    /// CPU-Paket (`package-0`).
    pkg: Option<RaplZone>,
    /// Ganze Plattform (`psys`) — nicht auf jeder Hardware vorhanden.
    psys: Option<RaplZone>,
    /// Gecachte Akku-Pfade. `None` = (noch) nicht gescannt;
    /// `Some(vec![])` = kein Akku vorhanden (gültiger Cache, z. B. Desktop).
    bats: Option<Vec<PathBuf>>,
    /// Countdown bis zum nächsten Verzeichnis-Rescan (Hot-Swap/Zweit-Akku).
    bat_rescan: u32,
}

impl PowerReader {
    pub fn new() -> Self {
        Self {
            pkg: find_rapl_zone(hw::RAPL_PKG_ZONE),
            psys: find_rapl_zone(hw::RAPL_PSYS_ZONE),
            bats: None,
            bat_rescan: 0,
        }
    }

    /// Zählerzustand verwerfen (nach Suspend), damit kein Delta über die
    /// Schlafphase gebildet wird.
    pub fn reset(&mut self) {
        if let Some(z) = &mut self.pkg {
            z.prev = None;
        }
        if let Some(z) = &mut self.psys {
            z.prev = None;
        }
    }

    pub fn read(&mut self) -> PowerInfo {
        let mut info = PowerInfo::default();

        info.cpu_pkg_w = self.pkg.as_mut().and_then(RaplZone::read_watts);
        let psys_w = self.psys.as_mut().and_then(RaplZone::read_watts);

        let bat = self.read_batteries();
        info.bat_voltage_v = bat.voltage_v;
        info.bat_power_w = bat.power_w;
        info.bat_status = bat.status;

        if psys_w.is_some() {
            info.sys_w = psys_w;
            // Beim Laden fließt zusätzlich die Ladeleistung aus dem Netzteil.
            if bat.status == BatStatus::Charging {
                if let (Some(s), Some(b)) = (psys_w, bat.power_w) {
                    info.charger_w = Some(s + b);
                }
            }
        } else if self.psys.is_none() && bat.voltage_v.is_some() {
            // Kein psys lesbar, aber ein Akku vorhanden → Fallback.
            info.sys_from_battery = true;
            if bat.status == BatStatus::Discharging {
                info.sys_w = bat.power_w;
            }
        }
        info
    }
}

/// Zwischenergebnis des Akku-Scans.
#[derive(Default)]
struct BatteryReading {
    voltage_v: Option<f32>,
    power_w: Option<f32>,
    status: BatStatus,
}

/// Sucht die RAPL-Zone mit dem gegebenen `name` (z. B. `package-0`, `psys`) und
/// probt dabei die Lesbarkeit von `energy_uj` — schlägt sie fehl (EACCES ohne
/// udev-Regel), bleibt die Zone `None` und das UI degradiert still.
fn find_rapl_zone(want: &str) -> Option<RaplZone> {
    for e in fs::read_dir(hw::POWERCAP_DIR).ok()?.flatten() {
        let p = e.path();
        let Ok(name) = fs::read_to_string(p.join("name")) else {
            continue;
        };
        if name.trim() != want {
            continue;
        }
        let energy_path = p.join(hw::RAPL_ENERGY_FILE);
        // Lesbarkeits-Probe: ohne Rechte gibt es diese Zone für uns nicht.
        if fs::read_to_string(&energy_path).is_err() {
            return None;
        }
        let max_range_uj = fs::read_to_string(p.join(hw::RAPL_MAX_ENERGY_FILE))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        return Some(RaplZone {
            energy_path,
            max_range_uj,
            prev: None,
        });
    }
    None
}

impl PowerReader {
    /// Liest alle Akkus über die gecachten Pfade: Leistung wird über alle
    /// summiert (Zweit-Akku), Spannung und Status kommen vom ersten
    /// (alphabetisch, d. h. BAT0). Das Verzeichnis wird nur beim ersten Mal
    /// und danach alle `BAT_RESCAN_TICKS` neu gescannt (Hot-Swap); ein
    /// Lesefehler auf einem gecachten Pfad (Akku entfernt) verwirft den Cache.
    fn read_batteries(&mut self) -> BatteryReading {
        if self.bats.is_none() || self.bat_rescan == 0 {
            self.bats = Some(scan_batteries());
            self.bat_rescan = hw::BAT_RESCAN_TICKS;
        }
        self.bat_rescan -= 1;

        let mut out = BatteryReading::default();
        let bats = self.bats.as_deref().unwrap_or(&[]);
        let mut power_sum_uw: Option<u64> = None;
        for (i, bat) in bats.iter().enumerate() {
            if i == 0 {
                out.voltage_v = read_u64(&bat.join("voltage_now")).map(|uv| uv as f32 / 1e6);
                match fs::read_to_string(bat.join("status")) {
                    Ok(s) => {
                        out.status = match s.trim() {
                            "Charging" => BatStatus::Charging,
                            "Discharging" => BatStatus::Discharging,
                            "Full" => BatStatus::Full,
                            _ => BatStatus::Unknown,
                        }
                    }
                    // Akku weg → Cache verwerfen, nächster Tick scannt neu.
                    Err(_) => {
                        self.bats = None;
                        return BatteryReading::default();
                    }
                }
            }
            // Manche Akkus exportieren nur current_now — dann bleibt die Leistung leer.
            if let Some(uw) = read_u64(&bat.join("power_now")) {
                power_sum_uw = Some(power_sum_uw.unwrap_or(0) + uw);
            }
        }
        out.power_w = power_sum_uw.map(|uw| uw as f32 / 1e6);
        out
    }
}

/// Voller Verzeichnis-Scan nach Akkus (`type == Battery`), sortiert (BAT0 zuerst).
fn scan_batteries() -> Vec<PathBuf> {
    let Ok(dir) = fs::read_dir(hw::POWER_SUPPLY_DIR) else {
        return Vec::new();
    };
    let mut bats: Vec<PathBuf> = dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            fs::read_to_string(p.join("type")).is_ok_and(|t| t.trim() == "Battery")
        })
        .collect();
    bats.sort();
    bats
}

fn read_u64(p: &PathBuf) -> Option<u64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

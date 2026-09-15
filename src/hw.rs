// SPDX-License-Identifier: GPL-3.0-only
//
// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  HARDWARE-ANPASSUNG — die EINZIGE Datei, die man für andere Hardware       ║
// ║  anfassen muss.                                                            ║
// ╚══════════════════════════════════════════════════════════════════════════╝
//
// Alle board-/treiber-spezifischen Pfade und Sensor-Bezeichner sind hier
// gesammelt. Auf anderer Hardware weichen vor allem die **hwmon-Chipnamen**
// (CPU-/RAM-Temp, Lüfter) und ggf. die GPU-Erkennung ab.
//
// Chipnamen des eigenen Systems herausfinden:
//
//     for f in /sys/class/hwmon/hwmon*/name; do echo "$f -> $(cat "$f")"; done
//
// Temperatur-Labels eines Chips:
//
//     grep . /sys/class/hwmon/hwmon*/temp*_label
//
// Es wird ausschließlich **gelesen** — siehe Design-Philosophie „only read,
// never blocking": keine Subprozesse, keine Netzwerkzugriffe, kein Schreiben.

// ---- hwmon (am ehesten anzupassen) ----

/// CPU-Paket-Temperatur: Liste von (hwmon-Chipname, gesuchtes `temp*_label`).
/// Wird der Reihe nach versucht; der erste Treffer gewinnt.
/// Intel: `coretemp`/„Package id 0". AMD: `k10temp`/„Tctl". Weitere hier ergänzen.
pub const CPU_TEMP_LABELED: &[(&str, &str)] =
    &[("coretemp", "Package id 0"), ("k10temp", "Tctl")];

/// Fallback-Chip für die CPU-Temp (dessen `temp1_input`), falls oben nichts passt.
pub const CPU_TEMP_FALLBACK_CHIP: &str = "acpitz";

/// hwmon-Chip für die RAM-Temperatur (`temp1_input`). Oft `spd5118` (DDR5).
/// Fehlt der Sensor (z. B. geblacklistet), wird die RAM-Temp einfach ausgeblendet.
pub const RAM_TEMP_CHIP: &str = "spd5118";

/// hwmon-Chip für die Lüfterdrehzahlen (`fan1_input` … `fanN_input`).
/// ThinkPad: `thinkpad`. Andere Notebooks/Desktops nutzen z. B. `dell_smm`, `nct6...`.
pub const FAN_CHIP: &str = "thinkpad";
/// Höchster geprüfter Lüfter-Index (`fan1_input` … `fan{N}_input`).
pub const FAN_MAX_INDEX: u32 = 4;

// ---- GPU (NVIDIA / PRIME) ----

/// Quelle des PRIME-Modus (Inhalt: `on-demand` | `on` | `off`).
pub const PRIME_DISCRETE_PATH: &str = "/etc/prime-discrete";
/// Verzeichnis der PCI-Geräte (zur dGPU-Erkennung + RTD3-Zustand).
pub const PCI_DEVICES_DIR: &str = "/sys/bus/pci/devices";
/// PCI-Vendor-ID von NVIDIA.
pub const NVIDIA_VENDOR_ID: &str = "0x10de";
/// PCI-Klassenpräfix „Display controller" (0x03xxxx).
pub const DISPLAY_CLASS_PREFIX: &str = "0x03";
/// Relativer Pfad zum RTD3-Laufzeitzustand eines PCI-Geräts (`active` | `suspended`).
pub const RUNTIME_STATUS_REL: &str = "power/runtime_status";

// ---- Energie / RAPL / Akku ----

/// Basis der RAPL-Zonen (Intel/AMD Energie-Zähler).
pub const POWERCAP_DIR: &str = "/sys/class/powercap";
/// Zonen-`name` des CPU-Pakets (inkl. iGPU). Über den Namen matchen, nicht über
/// `intel-rapl:N` — die Nummerierung variiert je nach System.
pub const RAPL_PKG_ZONE: &str = "package-0";
/// Zonen-`name` der Gesamt-Plattform (nicht auf jeder Hardware vorhanden).
pub const RAPL_PSYS_ZONE: &str = "psys";
/// Energie-Zähler in µJ (root-only ohne udev-Regel — siehe README).
pub const RAPL_ENERGY_FILE: &str = "energy_uj";
/// Wrap-Grenze des Zählers.
pub const RAPL_MAX_ENERGY_FILE: &str = "max_energy_range_uj";
/// Basis der Netzteile/Akkus.
pub const POWER_SUPPLY_DIR: &str = "/sys/class/power_supply";
/// Ticks zwischen zwei Akku-Verzeichnis-Rescans (Hot-Swap/Zweit-Akku);
/// ~60 s beim Default-Intervall von 1,5 s.
pub const BAT_RESCAN_TICKS: u32 = 40;
/// Unplausibel hohe Leistungswerte verwerfen (z. B. Spike nach Resume).
pub const POWER_SANITY_MAX_W: f32 = 1000.0;

// ---- Netz ----

/// Basis der Netzwerk-Schnittstellen.
pub const SYS_CLASS_NET: &str = "/sys/class/net";
/// Iface-Namenspräfixe, die als VPN gelten (sonst LAN; WLAN via `…/<if>/wireless`).
pub const VPN_IFACE_PREFIXES: &[&str] = &["tun", "tap", "wg", "ppp"];

// ---- Standardpfade (selten zu ändern) ----

pub const PROC_STAT: &str = "/proc/stat";
pub const PROC_MEMINFO: &str = "/proc/meminfo";
pub const PROC_NET_ROUTE: &str = "/proc/net/route";
pub const SYS_CLASS_HWMON: &str = "/sys/class/hwmon";

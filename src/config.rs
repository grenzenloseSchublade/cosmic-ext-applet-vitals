// SPDX-License-Identifier: GPL-3.0-only

use cosmic::cosmic_config::{self, cosmic_config_derive::CosmicConfigEntry, CosmicConfigEntry};

/// Persistente Konfiguration (über cosmic-config, live aktualisierbar).
/// Nur primitive Typen → keine zusätzliche serde-Abhängigkeit nötig.
#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    /// Aktualisierungsintervall in Millisekunden.
    pub interval_ms: u64,
    /// Welche Metrik-Zeilen im Popup erscheinen.
    pub show_cpu: bool,
    pub show_cpu_temp: bool,
    pub show_mem: bool,
    pub show_net: bool,
    pub show_gpu: bool,
    pub show_fans: bool,
    /// Leistungsaufnahme (System/CPU/GPU in Watt) im Popup zeigen.
    pub show_power: bool,
    /// Watt-Zeile zusätzlich nach CPU/GPU aufschlüsseln (sonst nur Gesamtwert).
    pub power_breakdown: bool,
    /// Akku-Zeile (Spannung, Ladezustand) im Popup zeigen.
    pub show_battery: bool,
    /// Disk-I/O-Rate (Lesen/Schreiben) im Popup zeigen.
    pub show_disk: bool,
    /// Swap-Zeile im Popup zeigen (erscheint nur, wenn Swap eingerichtet ist).
    pub show_swap: bool,
    /// Load Average (1/5/15 min) im Popup zeigen.
    pub show_load: bool,
    /// Uptime-Zeile im Popup zeigen.
    pub show_uptime: bool,
    /// Kumulierter Netz-Verbrauch (RX/TX seit Boot) im Popup zeigen.
    pub show_net_total: bool,
    /// Reihenfolge der Metriken im Popup als IDs (siehe `MetricKind` in app.rs).
    /// 0=CPU, 1=RAM, 2=Netz, 3=GPU, 4=Lüfter, 5=Kerne, 6=Watt, 7=Akku,
    /// 8=Disk, 9=Swap, 10=Load, 11=Uptime, 12=NetzΣ.
    pub metric_order: Vec<u8>,
    /// Temperatur in °F statt °C.
    pub fahrenheit: bool,
    /// Netz-Einheit: 0 = SI (MB/s), 1 = binär (MiB/s), 2 = Bit (Mbit/s).
    pub net_unit: u8,
    /// Monospace-Font im Panel (ruhigeres Bild).
    pub mono_font: bool,
    /// dGPU im Schlaf ausblenden statt „schläft" zeigen.
    pub hide_gpu_when_asleep: bool,
    /// Schwellen für Warn-/Kritisch-Farben im Popup (°C). Bewusst ohne
    /// UI-Schalter — nur über die cosmic-config-Dateien änderbar.
    pub warn_temp_c: u32,
    pub crit_temp_c: u32,
    /// CPU-Auslastung pro Kern im Popup zeigen.
    pub per_core: bool,
    /// Grafische Auslastungsbalken im Popup (statt nur Text) für CPU/RAM/GPU.
    pub graphical: bool,
    /// Verlaufs-Graphen (Sparklines) im Popup — Master-Schalter.
    pub show_graphs: bool,
    /// Sparkline je Metrik (wirkt nur bei aktivem Master-Schalter).
    pub graph_cpu: bool,
    pub graph_mem: bool,
    pub graph_net: bool,
    pub graph_power: bool,
    /// Metrik-Beschriftungen im Popup in der System-Akzentfarbe (dezente
    /// Struktur; Warnfarben Orange/Rot bleiben den Schwellen vorbehalten).
    pub accent_labels: bool,
    /// Einen kompakten Wert direkt neben dem Panel-Icon anzeigen (nur horizontale Leiste).
    pub panel_text: bool,
    /// Welche Metrik im Panel-Text steht (0=CPU, 1=RAM, 2=Netz, 3=GPU, 6=Watt).
    pub panel_metric: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval_ms: 1500,
            show_cpu: true,
            show_cpu_temp: true,
            show_mem: true,
            show_net: true,
            show_gpu: true,
            show_fans: true,
            show_power: true,
            power_breakdown: true,
            show_battery: false,
            show_disk: false,
            show_swap: false,
            show_load: false,
            show_uptime: false,
            show_net_total: false,
            metric_order: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
            fahrenheit: false,
            net_unit: 0,
            mono_font: true,
            hide_gpu_when_asleep: false,
            warn_temp_c: 80,
            crit_temp_c: 90,
            per_core: true,
            graphical: false,
            show_graphs: true,
            graph_cpu: true,
            graph_mem: true,
            graph_net: true,
            graph_power: true,
            accent_labels: true,
            panel_text: false,
            panel_metric: 0,
        }
    }
}

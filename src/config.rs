// SPDX-License-Identifier: GPL-3.0-only

use cosmic::cosmic_config::{self, cosmic_config_derive::CosmicConfigEntry, CosmicConfigEntry};

/// Persistente Konfiguration (über cosmic-config, live aktualisierbar).
/// Nur primitive Typen → keine zusätzliche serde-Abhängigkeit nötig.
#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    /// Aktualisierungsintervall in Millisekunden.
    pub interval_ms: u64,
    /// Welche Werte inline im Panel erscheinen.
    pub show_cpu: bool,
    pub show_cpu_temp: bool,
    pub show_mem: bool,
    pub show_net: bool,
    pub show_gpu: bool,
    pub show_fans: bool,
    /// Reihenfolge der Metriken im Popup als IDs (siehe `MetricKind` in app.rs).
    /// 0=CPU, 1=RAM, 2=Netz, 3=GPU, 4=Lüfter, 5=Kerne.
    pub metric_order: Vec<u8>,
    /// Temperatur in °F statt °C.
    pub fahrenheit: bool,
    /// Netz-Einheit: 0 = SI (MB/s), 1 = binär (MiB/s), 2 = Bit (Mbit/s).
    pub net_unit: u8,
    /// Monospace-Font im Panel (ruhigeres Bild).
    pub mono_font: bool,
    /// dGPU im Schlaf ausblenden statt „schläft" zeigen.
    pub hide_gpu_when_asleep: bool,
    /// Schwellen für Warn-/Kritisch-Farben im Popup (°C).
    pub warn_temp_c: u32,
    pub crit_temp_c: u32,
    /// CPU-Auslastung pro Kern im Popup zeigen.
    pub per_core: bool,
    /// Grafische Auslastungsbalken im Popup (statt nur Text) für CPU/RAM/GPU.
    pub graphical: bool,
    /// Einen kompakten Wert direkt neben dem Panel-Icon anzeigen (nur horizontale Leiste).
    pub panel_text: bool,
    /// Welche Metrik im Panel-Text steht (0=CPU, 1=RAM, 2=Netz, 3=GPU).
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
            metric_order: vec![0, 1, 2, 3, 4, 5],
            fahrenheit: false,
            net_unit: 0,
            mono_font: true,
            hide_gpu_when_asleep: false,
            warn_temp_c: 80,
            crit_temp_c: 90,
            per_core: true,
            graphical: false,
            panel_text: false,
            panel_metric: 0,
        }
    }
}

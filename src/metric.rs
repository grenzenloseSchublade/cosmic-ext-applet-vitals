// SPDX-License-Identifier: GPL-3.0-only

//! Metrik-Katalog: `MetricKind` (IDs, Labels, Erklärtexte, Sichtbarkeit)
//! und die Normalisierung gespeicherter Reihenfolgen.

use crate::config::Config;

/// Welche Metriken es gibt — die `u8`-IDs entsprechen `Config::metric_order`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    Cpu,
    Mem,
    Net,
    Gpu,
    Fans,
    Cores,
    Power,
    Battery,
    Disk,
    Swap,
    Load,
    Uptime,
    NetTotal,
}

impl MetricKind {
    /// Kanonische Reihenfolge — **Single Source of Truth** für die `u8`-IDs (Index = ID).
    /// Neue Metriken NUR hinten anhängen, sonst verschieben sich gespeicherte IDs.
    const ALL: [MetricKind; 13] = [
        Self::Cpu,
        Self::Mem,
        Self::Net,
        Self::Gpu,
        Self::Fans,
        Self::Cores,
        Self::Power,
        Self::Battery,
        Self::Disk,
        Self::Swap,
        Self::Load,
        Self::Uptime,
        Self::NetTotal,
    ];
    /// Im Panel-Text anzeigbare Metriken (kompakter Einzelwert), zykliert durch `CyclePanelMetric`.
    pub(crate) const PANEL: [MetricKind; 5] =
        [Self::Cpu, Self::Mem, Self::Net, Self::Gpu, Self::Power];

    pub(crate) fn from_u8(v: u8) -> Option<Self> {
        Self::ALL.get(v as usize).copied()
    }

    /// Die `u8`-ID (= Index in `ALL` — dort liegt die Single Source of Truth).
    pub(crate) fn id(self) -> u8 {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0) as u8
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Mem => "RAM",
            Self::Net => "Netz",
            Self::Gpu => "GPU",
            Self::Fans => "Lüfter",
            Self::Cores => "Kerne",
            Self::Power => "Watt",
            Self::Battery => "Akku",
            Self::Disk => "Disk",
            Self::Swap => "Swap",
            Self::Load => "Load",
            Self::Uptime => "Uptime",
            Self::NetTotal => "Netz Σ",
        }
    }

    /// Kurzerklärung für das Info-Icon in den Einstellungen.
    pub(crate) fn info(self) -> &'static str {
        match self {
            Self::Cpu => "Gesamtauslastung aller Kerne in Prozent; optional mit Temperatur.",
            Self::Mem => "Belegter Arbeitsspeicher (belegt/gesamt) und Prozent.",
            Self::Net => "Empfangs-/Senderate der Netzwerkschnittstellen (Einheit unter „Netz-Einheit“).",
            Self::Gpu => "Auslastung, Speicher und Temperatur der dedizierten GPU (NVIDIA/NVML).",
            Self::Fans => "Drehzahlen der Lüfter (Quelle: hwmon).",
            Self::Cores => "Auslastung jedes einzelnen CPU-Kerns als eigene Zeile.",
            Self::Power => "Leistungsaufnahme: System (RAPL psys), CPU-Package und GPU.",
            Self::Battery => "Ladezustand, Spannung sowie Lade-/Entladeleistung des Akkus.",
            Self::Disk => "Lese-/Schreibrate aller physischen Laufwerke zusammen.",
            Self::Swap => "Belegter Auslagerungsspeicher; ausgeblendet, wenn kein Swap eingerichtet ist.",
            Self::Load => "Load Average über 1, 5 und 15 Minuten.",
            Self::Uptime => "Zeit seit dem letzten Systemstart.",
            Self::NetTotal => "Kumulierter Netz-Verbrauch (empfangen/gesendet) seit dem Systemstart.",
        }
    }

    /// Erklärtext zur Metrik-Zeile der Hauptansicht: was die Anzeige konkret
    /// bedeutet (Spalten, Zustände, Quellen). Muster: Kopfzeile, darunter
    /// „•"-Punkte je Spalte/Zustand — gerendert von `info_content`.
    pub(crate) fn info_detail(self) -> &'static str {
        match self {
            Self::Cpu => "Gesamtauslastung aller Kerne.\n\
            • links % · rechts Paket-Temperatur (hwmon)\n\
            • Verlauf: Hovern zeigt Wert und Zeitpunkt",
            Self::Mem => "Arbeitsspeicher-Belegung.\n\
            • links % · Mitte belegt/gesamt GiB\n\
            • rechts RAM-Temperatur (falls Sensor vorhanden)\n\
            • Verlauf: Hovern zeigt Wert und Zeitpunkt",
            Self::Net => "Datenrate der aktiven Schnittstelle.\n\
            • ↓ empfangen · ↑ senden · Typ (WLAN/LAN/VPN)\n\
            • Einheit unter „Netz-Einheit“ wählbar\n\
            • Verlauf: Skala 0…≤ Fenster-Maximum, ↑ gedimmt; Hovern zeigt Werte und Zeitpunkt",
            Self::Gpu => "Dedizierte NVIDIA-GPU (NVML).\n\
            • „schläft“ — Stromsparmodus, wird nie geweckt\n\
            • „keine NVIDIA“ — keine dGPU gefunden\n\
            • „aktiv · Modus“ — wach, ohne Live-Werte\n\
            • sonst: Auslastung % · VRAM · Temperatur",
            Self::Fans => "Drehzahlen aller erkannten Lüfter (hwmon), in U/min.",
            Self::Cores => "Auslastung je CPU-Kern in Prozent, Reihen zu je 6 Kernen.",
            Self::Power => "Leistungsaufnahme des Systems.\n\
            • Gesamt: RAPL psys — „– · Netz“ heißt: am Netz nicht messbar (nur Akku-Messung verfügbar)\n\
            • CPU-Package · GPU (Schalter „Watt aufschlüsseln“)\n\
            • beim Laden: „Netzteil ≈“ psys + Ladeleistung\n\
            • Verlauf: Skala 0…≤ Fenster-Maximum; Hovern zeigt Wert und Zeitpunkt",
            Self::Battery => "Akku-Zustand.\n\
            • Spannung (V) · Status (lädt/entlädt/voll)\n\
            • Lade-/Entladeleistung in W, nur wenn Strom fließt",
            Self::Disk => "Datenrate aller physischen Laufwerke.\n\
            • ↓ lesen · ↑ schreiben\n\
            • Partitionen/virtuelle Devices nicht doppelt gezählt\n\
            • Quelle: /proc/diskstats",
            Self::Swap => "Auslagerungsspeicher: % und belegt/gesamt GiB.\n\
            • Zeile erscheint nur, wenn Swap eingerichtet ist",
            Self::Load => "Load Average 1 / 5 / 15 min.\n\
            • Ø lauffähige Prozesse; Werte über der Kernzahl bedeuten Wartezeiten",
            Self::Uptime => "Zeit seit dem letzten Systemstart.",
            Self::NetTotal => "Summe seit Systemstart (aktive Schnittstelle).\n\
            • ↓ empfangen · ↑ gesendet\n\
            • bei Wechsel WLAN↔LAN zählt die neue ab ihrem Stand",
        }
    }

    /// Ob diese Metrik laut Config sichtbar sein soll (mappt auf die `show_*`-Bools).
    pub(crate) fn enabled(self, c: &Config) -> bool {
        match self {
            Self::Cpu => c.show_cpu,
            Self::Mem => c.show_mem,
            Self::Net => c.show_net,
            Self::Gpu => c.show_gpu,
            Self::Fans => c.show_fans,
            Self::Cores => c.per_core,
            Self::Power => c.show_power,
            Self::Battery => c.show_battery,
            Self::Disk => c.show_disk,
            Self::Swap => c.show_swap,
            Self::Load => c.show_load,
            Self::Uptime => c.show_uptime,
            Self::NetTotal => c.show_net_total,
        }
    }
}

/// Bringt eine gespeicherte `metric_order` auf den aktuellen Stand: unbekannte
/// IDs raus, Duplikate raus, fehlende (neu hinzugekommene) IDs hinten anfügen.
/// Wird nur in-memory angewandt — kein Zurückschreiben, sonst entstünde eine
/// Schleife mit dem Config-Watcher.
pub(crate) fn normalize_order(order: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(MetricKind::ALL.len());
    for &id in order {
        if MetricKind::from_u8(id).is_some() && !out.contains(&id) {
            out.push(id);
        }
    }
    for kind in MetricKind::ALL {
        if !out.contains(&kind.id()) {
            out.push(kind.id());
        }
    }
    out
}

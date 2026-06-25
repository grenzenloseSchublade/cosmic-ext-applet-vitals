// SPDX-License-Identifier: GPL-3.0-only
//
// GPU/NVIDIA: Modus + dGPU-Zustand kommen aus sysfs (weckt die GPU NICHT).
// Auslastung/Temperatur kommen über NVML in-process (kein nvidia-smi-Subprozess)
// und NUR wenn die dGPU laut sysfs `active` ist — so wird eine schlafende dGPU
// (RTD3) niemals geweckt und es kann nichts beim Suspend hängen.

use crate::hw;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::Nvml;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GpuMode {
    Hybrid,
    Nvidia,
    Integrated,
    #[default]
    Unknown,
}

impl GpuMode {
    pub fn label(self) -> &'static str {
        match self {
            GpuMode::Hybrid => "hybrid",
            GpuMode::Nvidia => "nvidia",
            GpuMode::Integrated => "integriert",
            GpuMode::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GpuInfo {
    pub mode: GpuMode,
    /// Eine NVIDIA-dGPU ist vorhanden.
    pub present: bool,
    /// dGPU ist wach (runtime_status == active).
    pub awake: bool,
    pub util: Option<u32>,
    pub temp_c: Option<u32>,
    /// Belegter/gesamter VRAM in MB (nur wenn dGPU aktiv).
    pub vram_used_mb: Option<u32>,
    pub vram_total_mb: Option<u32>,
}

pub struct GpuReader {
    nvml: Option<Nvml>,
    /// Wartezähler nach fehlgeschlagener NVML-Init (vermeidet dlopen bei jedem Tick,
    /// gibt aber NICHT dauerhaft auf — nach Ablauf wird erneut versucht).
    backoff: u8,
    pci_path: Option<PathBuf>,
}

impl GpuReader {
    pub fn new() -> Self {
        Self {
            nvml: None,
            backoff: 0,
            pci_path: nvidia_pci_path(),
        }
    }

    /// `want_live`: nur dann NVML lesen, wenn der Anrufer Live-Werte braucht (Popup offen).
    /// Sonst — und immer wenn die dGPU schläft — wird das NVML-Handle **freigegeben**
    /// (`/dev/nvidia0` schließt → RTD3-Schlaf möglich). Wir wecken die dGPU **nie** selbst:
    /// NVML wird ausschließlich angefasst, wenn sie laut sysfs ohnehin schon `active` ist.
    pub fn read(&mut self, want_live: bool) -> GpuInfo {
        let mut info = GpuInfo {
            mode: read_mode(),
            ..Default::default()
        };

        let Some(pci) = &self.pci_path else {
            self.nvml = None;
            return info; // keine NVIDIA-dGPU
        };
        info.present = true;

        let awake = fs::read_to_string(pci.join(hw::RUNTIME_STATUS_REL))
            .map(|s| s.trim() == "active")
            .unwrap_or(false);
        info.awake = awake;

        // dGPU schläft oder keine Live-Werte gewünscht → NVML-Handle freigeben und NICHT anfassen.
        // So pinnt das Applet die dGPU nicht (kein offener fd → sie kann per RTD3 einschlafen)
        // und weckt sie nie.
        if !awake || !want_live {
            self.nvml = None;
            return info;
        }

        // NVML faul initialisieren (dlopt libnvidia-ml.so erst bei Bedarf).
        // Bei Fehlschlag (z. B. RTD3-Aufwach-Fenster) später erneut versuchen,
        // statt dauerhaft aufzugeben — mit kurzem Backoff gegen dlopen-Spam.
        if self.nvml.is_none() {
            if self.backoff == 0 {
                match Nvml::init() {
                    Ok(n) => self.nvml = Some(n),
                    Err(_) => self.backoff = 20,
                }
            } else {
                self.backoff -= 1;
            }
        }
        if let Some(nvml) = &self.nvml {
            if let Ok(dev) = nvml.device_by_index(0) {
                info.util = dev.utilization_rates().ok().map(|u| u.gpu);
                info.temp_c = dev.temperature(TemperatureSensor::Gpu).ok();
                // VRAM (Bytes → MB). Kein zusätzlicher Subprozess; NVML wird ohnehin nur bei aktiver dGPU angefasst.
                if let Ok(mem) = dev.memory_info() {
                    let mb = |b: u64| (b / 1024 / 1024) as u32;
                    info.vram_used_mb = Some(mb(mem.used));
                    info.vram_total_mb = Some(mb(mem.total));
                }
            }
        }
        info
    }
}

fn read_mode() -> GpuMode {
    match fs::read_to_string(hw::PRIME_DISCRETE_PATH)
        .ok()
        .map(|s| s.trim().to_string())
        .as_deref()
    {
        Some("on-demand") => GpuMode::Hybrid,
        Some("on") => GpuMode::Nvidia,
        Some("off") => GpuMode::Integrated,
        _ => GpuMode::Unknown,
    }
}

/// Erste PCI-Display-Device mit NVIDIA-Vendor (0x10de).
fn nvidia_pci_path() -> Option<PathBuf> {
    for e in fs::read_dir(hw::PCI_DEVICES_DIR).ok()?.flatten() {
        let p = e.path();
        let vendor = fs::read_to_string(p.join("vendor")).unwrap_or_default();
        let class = fs::read_to_string(p.join("class")).unwrap_or_default();
        if vendor.trim() == hw::NVIDIA_VENDOR_ID
            && class.trim().starts_with(hw::DISPLAY_CLASS_PREFIX)
        {
            return Some(p);
        }
    }
    None
}

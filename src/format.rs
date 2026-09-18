// SPDX-License-Identifier: GPL-3.0-only

//! Message-freie Format-Helfer für Werte-Texte (Zeiten, Bytes, Raten, Temperaturen).

use crate::config::Config;
use crate::metrics::Metrics;

/// Zeitfenster kompakt: „45 s" unter einer Minute, sonst „3 min".
pub(crate) fn fmt_span(s: u64) -> String {
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min", s / 60)
    }
}

/// Uptime menschenlesbar: „3 d 4 h 12 min" (führende Null-Einheiten entfallen).
pub(crate) fn fmt_uptime(s: u64) -> String {
    let d = s / 86_400;
    let h = (s % 86_400) / 3_600;
    let min = (s % 3_600) / 60;
    if d > 0 {
        format!("{d} d {h} h {min} min")
    } else if h > 0 {
        format!("{h} h {min} min")
    } else {
        format!("{min} min")
    }
}

/// Byte-Summe menschenlesbar in SI-Stufen (kB/MB/GB/TB), eine Nachkommastelle.
pub(crate) fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e12 {
        format!("{:.2} TB", b / 1e12)
    } else if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} kB", b / 1e3)
    }
}

pub(crate) fn fmt_mem(m: &Metrics) -> String {
    let gib = |kb: u64| kb as f64 / 1024.0 / 1024.0;
    format!("{:.1}/{:.1} GiB", gib(m.mem_used_kb), gib(m.mem_total_kb))
}

pub(crate) fn fmt_temp_val(c: f32, cfg: &Config) -> String {
    if cfg.fahrenheit {
        format!("{:.0}°F", c * 9.0 / 5.0 + 32.0)
    } else {
        format!("{c:.0}°")
    }
}

/// Bytes/s menschenlesbar gemäß gewählter Einheit.
pub(crate) fn fmt_rate(bps: f64, cfg: &Config) -> String {
    fmt_rate_unit(bps, cfg.net_unit)
}

/// Wie `fmt_rate`, aber ohne Config-Borrow — für move-Closures (Sparkline-Hover).
pub(crate) fn fmt_rate_unit(bps: f64, net_unit: u8) -> String {
    match net_unit {
        2 => {
            let bits = bps * 8.0;
            if bits >= 1e9 {
                format!("{:.1}Gb/s", bits / 1e9)
            } else if bits >= 1e6 {
                format!("{:.1}Mb/s", bits / 1e6)
            } else {
                format!("{:.0}Kb/s", bits / 1e3)
            }
        }
        1 => fmt_scale(bps, 1024.0),
        _ => fmt_scale(bps, 1000.0),
    }
}

fn fmt_scale(v: f64, base: f64) -> String {
    let units = ["B/s", "K/s", "M/s", "G/s"];
    let mut val = v;
    let mut i = 0;
    while val >= base && i + 1 < units.len() {
        val /= base;
        i += 1;
    }
    if i == 0 {
        format!("{val:.0}{}", units[i])
    } else {
        format!("{val:.1}{}", units[i])
    }
}

pub(crate) fn mem_pct(m: &Metrics) -> f32 {
    if m.mem_total_kb == 0 {
        0.0
    } else {
        m.mem_used_kb as f32 / m.mem_total_kb as f32 * 100.0
    }
}

/// GPU-Zeile bei **inaktiver** dGPU — mit Modus (erklärt das Fehlen von Werten).
pub(crate) fn gpu_inactive_text(g: &crate::metrics::gpu::GpuInfo) -> String {
    use crate::metrics::gpu::GpuMode;
    if !g.present {
        return "keine NVIDIA".into();
    }
    if g.mode == GpuMode::Integrated {
        return "inaktiv · integriert".into();
    }
    format!("schläft · {}", g.mode.label())
}

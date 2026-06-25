// SPDX-License-Identifier: GPL-3.0-only

use crate::config::Config;
use crate::metrics::{Collector, Metrics};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::{time, window::Id, Alignment, Length, Limits, Subscription};
use cosmic::prelude::*;
use cosmic::widget;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Symbolisches Panel-Icon (Prozessor-Chip), eingebettet → kein Theme-Install nötig,
/// wird vom COSMIC-Panel automatisch hell/dunkel eingefärbt.
const CHIP_SYMBOLIC: &[u8] = include_bytes!("../resources/icon-symbolic.svg");

/// Feste Breite der fetten Metrik-Beschriftung (linke Spalte im Popup).
const LABEL_WIDTH: f32 = 60.0;
/// Höhe/Dicke der Auslastungsbalken.
const BAR_GIRTH: f32 = 8.0;
/// Panel-Icon-Vergrößerung gegenüber der vom Panel vorgeschlagenen Größe (innerhalb der Zelltiefe).
const ICON_SCALE: f32 = 1.2;
/// Popup-Größenlimits (das Panel zwingt die Breite faktisch auf ~360 px).
const POPUP_MIN_WIDTH: f32 = 260.0;
const POPUP_MAX_WIDTH: f32 = 372.0;
const POPUP_MIN_HEIGHT: f32 = 120.0;
const POPUP_MAX_HEIGHT: f32 = 1080.0;

/// Welche Metriken es gibt — die `u8`-IDs entsprechen `Config::metric_order`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    Cpu,
    Mem,
    Net,
    Gpu,
    Fans,
    Cores,
}

impl MetricKind {
    /// Kanonische Reihenfolge — **Single Source of Truth** für die `u8`-IDs (Index = ID).
    const ALL: [MetricKind; 6] = [
        Self::Cpu,
        Self::Mem,
        Self::Net,
        Self::Gpu,
        Self::Fans,
        Self::Cores,
    ];
    /// Im Panel-Text anzeigbare Metriken (kompakter Einzelwert), zykliert durch `CyclePanelMetric`.
    const PANEL: [MetricKind; 4] = [Self::Cpu, Self::Mem, Self::Net, Self::Gpu];

    fn from_u8(v: u8) -> Option<Self> {
        Self::ALL.get(v as usize).copied()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Mem => "RAM",
            Self::Net => "Netz",
            Self::Gpu => "GPU",
            Self::Fans => "Lüfter",
            Self::Cores => "Kerne",
        }
    }

    /// Ob diese Metrik laut Config sichtbar sein soll (mappt auf die `show_*`-Bools).
    fn enabled(self, c: &Config) -> bool {
        match self {
            Self::Cpu => c.show_cpu,
            Self::Mem => c.show_mem,
            Self::Net => c.show_net,
            Self::Gpu => c.show_gpu,
            Self::Fans => c.show_fans,
            Self::Cores => c.per_core,
        }
    }
}

/// Anzeigemodus des Popups: Werteliste oder Einstellungen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Metrics,
    Settings,
}

pub struct AppModel {
    core: cosmic::Core,
    popup: Option<Id>,
    config: Config,
    /// Handle zum Zurückschreiben der Config (cosmic-config). `None`, falls nicht verfügbar.
    config_handler: Option<cosmic_config::Config>,
    /// Hinter Arc<Mutex>, damit die (blockierende) Erfassung in einem Hintergrund-Thread
    /// läuft (spawn_blocking) und den UI-Thread nie blockiert.
    collector: Arc<Mutex<Collector>>,
    metrics: Metrics,
    /// True, solange eine Hintergrund-Erfassung läuft (In-Flight-Guard gegen Thread-Stau,
    /// falls NVML einmal hängt).
    refreshing: bool,
    ui_mode: ViewMode,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    /// Ergebnis einer Hintergrund-Erfassung.
    MetricsUpdated(Metrics),
    TogglePopup,
    PopupClosed(Id),
    UpdateConfig(Config),
    // --- Einstellungen ---
    ToggleSettings,
    SetMetricShown(MetricKind, bool),
    MoveUp(usize),
    MoveDown(usize),
    SetFahrenheit(bool),
    SetMonoFont(bool),
    SetHideGpu(bool),
    SetCpuTemp(bool),
    CycleNetUnit,
    SetInterval(u64),
    SetGraphical(bool),
    SetPanelText(bool),
    CyclePanelMetric,
    ResetDefaults,
}

impl AppModel {
    /// Mutiert die Config über `f` (typischerweise ein generierter `set_<feld>`-Setter,
    /// der den Wert setzt **und** persistiert). Ohne Handle passiert nichts.
    fn persist<F, E>(&mut self, f: F)
    where
        F: FnOnce(&mut Config, &cosmic_config::Config) -> Result<bool, E>,
    {
        if let Some(handler) = self.config_handler.as_ref() {
            let _ = f(&mut self.config, handler);
        }
    }

    /// Zykliert ein `u8`-Feld (`(current + 1) % modulo`) und persistiert es über `set`.
    fn cycle_persist<F, E>(&mut self, current: u8, modulo: u8, set: F)
    where
        F: FnOnce(&mut Config, &cosmic_config::Config, u8) -> Result<bool, E>,
    {
        let n = (current + 1) % modulo;
        self.persist(move |c, h| set(c, h, n));
    }

    /// Stößt eine **Hintergrund-Erfassung** an (`spawn_blocking` → nie auf dem UI-Thread).
    /// `live` = ob NVML gelesen werden darf (Popup offen). Der In-Flight-Guard verhindert,
    /// dass sich Erfassungen stauen, falls eine (z. B. NVML) hängt.
    fn spawn_refresh(&mut self, live: bool) -> Task<cosmic::Action<Message>> {
        if self.refreshing {
            return Task::none();
        }
        self.refreshing = true;
        let collector = self.collector.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || collector.lock().unwrap_or_else(|e| e.into_inner()).refresh(live))
                    .await
                    .unwrap_or_default()
            },
            Message::MetricsUpdated,
        )
        .map(cosmic::Action::App)
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "io.github.grenzenloseschublade.CosmicAppletVitals";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _flags: Self::Flags) -> (Self, Task<cosmic::Action<Self::Message>>) {
        // Handle behalten, damit Einstellungen aus dem UI zurückgeschrieben werden können.
        let config_handler = cosmic_config::Config::new(Self::APP_ID, Config::VERSION).ok();
        let config = config_handler
            .as_ref()
            .map(|context| match Config::get_entry(context) {
                Ok(config) => config,
                Err((_errors, config)) => config,
            })
            .unwrap_or_default();

        let collector = Arc::new(Mutex::new(Collector::new()));
        // Sofortiger Erststand (synchron, billig, kein NVML weil Popup zu).
        let metrics = collector.lock().unwrap_or_else(|e| e.into_inner()).refresh(false);

        let app = AppModel {
            core,
            popup: None,
            config,
            config_handler,
            collector,
            metrics,
            refreshing: false,
            ui_mode: ViewMode::Metrics,
        };
        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let interval = self.config.interval_ms.max(250);
        Subscription::batch(vec![
            time::every(Duration::from_millis(interval)).map(|_| Message::Tick),
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| Message::UpdateConfig(update.config)),
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::Tick => {
                // Erfassung im Hintergrund anstoßen; Live-GPU (NVML) nur bei offenem Popup.
                return self.spawn_refresh(self.popup.is_some());
            }
            Message::MetricsUpdated(m) => {
                self.metrics = m;
                self.refreshing = false;
            }
            Message::UpdateConfig(config) => {
                self.config = config;
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                    // NVML im Hintergrund freigeben → dGPU darf wieder einschlafen.
                    return self.spawn_refresh(false);
                }
            }
            Message::TogglePopup => {
                return if let Some(p) = self.popup.take() {
                    // Popup wird geschlossen → NVML-Handle (im Hintergrund) freigeben.
                    Task::batch([destroy_popup(p), self.spawn_refresh(false)])
                } else {
                    self.ui_mode = ViewMode::Metrics;
                    let new_id = Id::unique();
                    self.popup.replace(new_id);
                    let mut popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        None,
                        None,
                        None,
                    );
                    popup_settings.positioner.size_limits = Limits::NONE
                        .max_width(POPUP_MAX_WIDTH)
                        .min_width(POPUP_MIN_WIDTH)
                        .min_height(POPUP_MIN_HEIGHT)
                        .max_height(POPUP_MAX_HEIGHT);
                    get_popup(popup_settings)
                };
            }
            Message::ToggleSettings => {
                self.ui_mode = match self.ui_mode {
                    ViewMode::Metrics => ViewMode::Settings,
                    ViewMode::Settings => ViewMode::Metrics,
                };
            }
            Message::SetMetricShown(kind, b) => {
                self.persist(move |c, h| match kind {
                    MetricKind::Cpu => c.set_show_cpu(h, b),
                    MetricKind::Mem => c.set_show_mem(h, b),
                    MetricKind::Net => c.set_show_net(h, b),
                    MetricKind::Gpu => c.set_show_gpu(h, b),
                    MetricKind::Fans => c.set_show_fans(h, b),
                    MetricKind::Cores => c.set_per_core(h, b),
                });
            }
            Message::MoveUp(i) => {
                if i > 0 && i < self.config.metric_order.len() {
                    let mut order = self.config.metric_order.clone();
                    order.swap(i, i - 1);
                    self.persist(move |c, h| c.set_metric_order(h, order));
                }
            }
            Message::MoveDown(i) => {
                if i + 1 < self.config.metric_order.len() {
                    let mut order = self.config.metric_order.clone();
                    order.swap(i, i + 1);
                    self.persist(move |c, h| c.set_metric_order(h, order));
                }
            }
            Message::SetFahrenheit(v) => self.persist(move |c, h| c.set_fahrenheit(h, v)),
            Message::SetMonoFont(v) => self.persist(move |c, h| c.set_mono_font(h, v)),
            Message::SetHideGpu(v) => self.persist(move |c, h| c.set_hide_gpu_when_asleep(h, v)),
            Message::SetCpuTemp(v) => self.persist(move |c, h| c.set_show_cpu_temp(h, v)),
            Message::CycleNetUnit => {
                self.cycle_persist(self.config.net_unit, 3, |c, h, n| c.set_net_unit(h, n));
            }
            Message::SetInterval(ms) => {
                let ms = ms.max(250);
                self.persist(move |c, h| c.set_interval_ms(h, ms));
            }
            Message::SetGraphical(v) => self.persist(move |c, h| c.set_graphical(h, v)),
            Message::SetPanelText(v) => self.persist(move |c, h| c.set_panel_text(h, v)),
            Message::CyclePanelMetric => {
                let modulo = MetricKind::PANEL.len() as u8;
                self.cycle_persist(self.config.panel_metric, modulo, |c, h, n| {
                    c.set_panel_metric(h, n)
                });
            }
            Message::ResetDefaults => {
                self.persist(|c, h| {
                    *c = Config::default();
                    c.write_entry(h).map(|()| true)
                });
            }
        }
        Task::none()
    }

    /// Panel: symbolisches Chip-Icon (etwas größer), optional mit kompaktem Wert daneben
    /// (nur horizontale Leiste — vertikal wäre Text zu breit).
    fn view(&self) -> Element<'_, Self::Message> {
        let mut handle = cosmic::widget::icon::from_svg_bytes(CHIP_SYMBOLIC);
        handle.symbolic = true;

        // Icon vergrößern, aber auf die Zelltiefe (Icon + 2·Padding) kappen, damit nichts clippt
        // und die Panel-Dicke unverändert bleibt.
        let base = self.core.applet.suggested_size(true).0;
        let cell = base + 2 * self.core.applet.suggested_padding(true).1;
        let icon_size = ((base as f32 * ICON_SCALE).round() as u16).min(cell);
        let icon = widget::icon(handle).size(icon_size);

        let content: Element<'_, Message> =
            if self.config.panel_text && self.core.applet.is_horizontal() {
                widget::row::with_children(vec![
                    icon.into(),
                    // Monospace → die feste Zeichenbreite aus `panel_value` ergibt konstante Pixelbreite.
                    self.core
                        .applet
                        .text(self.panel_value())
                        .font(cosmic::iced::Font::MONOSPACE)
                        .into(),
                ])
                .spacing(cosmic::theme::spacing().space_xxs)
                .align_y(Alignment::Center)
                .into()
            } else {
                icon.into()
            };

        // `autosize_window` (mit AUTOSIZE_MAIN_ID) lässt die Applet-Layer-Surface auf die
        // Inhaltsbreite wachsen — sonst bliebe sie auf Icon-Größe und der Text würde abgeschnitten.
        self.core
            .applet
            .autosize_window(self.panel_button(content))
            .into()
    }

    /// Klick-Popup: Werteliste oder Einstellungen, je nach `ui_mode`.
    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let (title, gear) = match self.ui_mode {
            ViewMode::Metrics => ("Vitals · System", "emblem-system-symbolic"),
            ViewMode::Settings => ("Einstellungen", "go-previous-symbolic"),
        };
        let spacing = cosmic::theme::spacing();

        // Header mit demselben horizontalen Einzug wie die list_column-Zeilen
        // (`space_m`), damit Titel/Zahnrad bündig zu den Werte- bzw. Einstellungszeilen stehen.
        let header_row = widget::row::with_children(vec![
            widget::text::heading(title).width(Length::Fill).into(),
            widget::button::icon(widget::icon::from_name(gear))
                .on_press(Message::ToggleSettings)
                .into(),
        ])
        .align_y(Alignment::Center)
        .spacing(spacing.space_xs);
        // Einheitlicher horizontaler Einzug `space_m` — bündig zu list_column-Zeilen (Metrik)
        // bzw. zu den eingerückten Abschnitts-Titeln und Items (Einstellungen).
        let header = widget::container(header_row).padding([spacing.space_xxs, spacing.space_m]);

        let body = match self.ui_mode {
            ViewMode::Metrics => self.metrics_view(),
            ViewMode::Settings => self.settings_view(),
        };

        let content = widget::column::with_children(vec![header.into(), body])
            .spacing(spacing.space_xxs);
        self.core.applet.popup_container(content).into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl AppModel {
    /// Anklickbarer Panel-Button (Muster wie `applet::text_button`): Querachse auf die Panel-Dicke
    /// fixiert (Inhalt zentriert), Längsachse wächst mit dem Inhalt — **keine** fixe Breite, **kein**
    /// `autosize` → Icon+Text wird vollständig dargestellt, nicht abgeschnitten.
    fn panel_button<'a>(&self, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
        let applet = &self.core.applet;
        let (maj, min) = applet.suggested_padding(true);
        let base = applet.suggested_size(true);
        if applet.is_horizontal() {
            let cell = (base.1 + 2 * min) as f32;
            widget::button::custom(widget::layer_container(content).center_y(Length::Fixed(cell)))
                .padding([0, maj])
                .class(cosmic::theme::Button::AppletIcon)
                .on_press(Message::TogglePopup)
                .into()
        } else {
            let cell = (base.0 + 2 * min) as f32;
            widget::button::custom(widget::layer_container(content).center_x(Length::Fixed(cell)))
                .padding([maj, 0])
                .class(cosmic::theme::Button::AppletIcon)
                .on_press(Message::TogglePopup)
                .into()
        }
    }

    /// Kompakter Wert für den Panel-Text (gemäß `panel_metric`).
    /// Werte in **fester Zeichenbreite** (rechtsbündig) — zusammen mit Monospace-Font im Panel
    /// bleibt die Applet-Breite stabil, auch wenn sich die Zahlen ändern.
    fn panel_value(&self) -> String {
        let m = &self.metrics;
        match MetricKind::from_u8(self.config.panel_metric) {
            Some(MetricKind::Mem) => format!("RAM {:>3.0}%", mem_pct(m)),
            Some(MetricKind::Net) => format!(
                "↓{:>8} ↑{:>8}",
                fmt_rate(m.net_down_bps, &self.config),
                fmt_rate(m.net_up_bps, &self.config)
            ),
            Some(MetricKind::Gpu) => m
                .gpu
                .util
                .map(|u| format!("GPU {u:>3}%"))
                .unwrap_or_else(|| "GPU   –".into()),
            _ => format!("CPU {:>3.0}%", m.cpu_pct),
        }
    }

    /// Werteliste in der konfigurierten Reihenfolge.
    fn metrics_view(&self) -> Element<'_, Message> {
        let mut list = widget::list_column();
        let mut any = false;
        for id in &self.config.metric_order {
            if let Some(kind) = MetricKind::from_u8(*id) {
                for row in self.metric_rows(kind) {
                    list = list.add(row);
                    any = true;
                }
            }
        }
        if !any {
            list = list.add(widget::text("Keine Metrik aktiv — über das Zahnrad aktivieren."));
        }
        list.into()
    }

    /// Die (0..n) Zeilen einer Metrik — leer, wenn deaktiviert oder keine Daten.
    /// CPU/RAM/GPU: bei `graphical` zweizeilig (Werte + Balken über volle Breite), sonst einzeilig.
    fn metric_rows(&self, kind: MetricKind) -> Vec<Element<'_, Message>> {
        let m = &self.metrics;
        let c = &self.config;
        if !kind.enabled(c) {
            return Vec::new();
        }
        match kind {
            MetricKind::Cpu => {
                // links % · (mittig leer) · rechts Temp (eingefärbt nach Schwellen).
                let temp = if c.show_cpu_temp { m.cpu_temp_c } else { None };
                vec![metric_or_bar(
                    c.graphical,
                    "CPU",
                    m.cpu_pct / 100.0,
                    format!("{:.0} %", m.cpu_pct),
                    String::new(),
                    temp_cell(temp, c, c.mono_font),
                    c.mono_font,
                )]
            }
            MetricKind::Mem => {
                // links % · mittig GiB-Belegung · rechts RAM-Temp (eigener Sensor, optional).
                vec![metric_or_bar(
                    c.graphical,
                    "RAM",
                    mem_pct(m) / 100.0,
                    format!("{:.0} %", mem_pct(m)),
                    fmt_mem(m),
                    temp_cell(m.ram_temp_c, c, c.mono_font),
                    c.mono_font,
                )]
            }
            MetricKind::Net => {
                // Typ (WLAN/LAN/VPN) statt rohem Iface-Namen, mit „·"-Trenner (wie GPU/RAM).
                let kind = m.net_kind.map(|k| format!(" · {k}")).unwrap_or_default();
                // Raten rechtsbündig in fester Zeichenbreite → mit Monospace springt die Breite nicht.
                vec![labeled_row(
                    "Netz",
                    format!(
                        "↓ {:>8} ↑ {:>8}{}",
                        fmt_rate(m.net_down_bps, c),
                        fmt_rate(m.net_up_bps, c),
                        kind
                    ),
                    c.mono_font,
                )]
            }
            MetricKind::Gpu => {
                let g = &m.gpu;
                // Option „GPU im Schlaf ausblenden": schlafende dGPU komplett weglassen.
                if g.present && !g.awake && c.hide_gpu_when_asleep {
                    return Vec::new();
                }
                let text = if !g.present {
                    Some("keine NVIDIA".to_string())
                } else if !g.awake {
                    // schläft → nur Zustand + Modus (pin-frei erfasst).
                    Some(gpu_inactive_text(g))
                } else if g.util.is_none() {
                    // aktiv, aber keine Live-Zahlen (Popup zu / NVML nicht gelesen) → nur Text.
                    Some(format!("aktiv · {}", g.mode.label()))
                } else {
                    None
                };
                match (text, g.util) {
                    (Some(t), _) => vec![labeled_row("GPU", t, c.mono_font)],
                    // aktiv mit Live-Zahlen: links % · mittig VRAM · rechts Temp (kein Modus → kein Umbruch).
                    (None, Some(u)) => {
                        let vram = match (g.vram_used_mb, g.vram_total_mb) {
                            (Some(used), Some(total)) => format!(
                                "{:.1}/{:.1} GB",
                                used as f32 / 1024.0,
                                total as f32 / 1024.0
                            ),
                            _ => String::new(),
                        };
                        vec![metric_or_bar(
                            c.graphical,
                            "GPU",
                            u as f32 / 100.0,
                            format!("{u} %"),
                            vram,
                            temp_cell(g.temp_c.map(|t| t as f32), c, c.mono_font),
                            c.mono_font,
                        )]
                    }
                    (None, None) => Vec::new(),
                }
            }
            MetricKind::Fans => {
                if m.fans_rpm.is_empty() {
                    Vec::new()
                } else {
                    let fans = m
                        .fans_rpm
                        .iter()
                        .map(|r| r.to_string())
                        .collect::<Vec<_>>()
                        .join(" / ");
                    vec![labeled_row("Lüfter", format!("{fans} rpm"), c.mono_font)]
                }
            }
            MetricKind::Cores => {
                // Auslastung je Kern in % (monospace, bündig). In Blöcke zu 8 brechen,
                // damit auch Viel-Kern-CPUs (12/16/…) nicht über die ~312 px Popup-Breite laufen.
                const PER_ROW: usize = 6;
                m.per_core
                    .chunks(PER_ROW)
                    .enumerate()
                    .map(|(i, chunk)| {
                        let cores = chunk
                            .iter()
                            .map(|p| format!("{p:>3.0}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        // Folgezeilen ohne Label, aber gleiche Spaltenbreite → bündig untereinander.
                        let label = if i == 0 { "Kerne %" } else { "" };
                        labeled_row(label, cores, true)
                    })
                    .collect()
            }
        }
    }

    /// Einstellungs-Ansicht: Metriken+Reihenfolge, Anzeige-Optionen, Intervall.
    fn settings_view(&self) -> Element<'_, Message> {
        let c = &self.config;
        let spacing = cosmic::theme::spacing();

        // --- Metriken & Reihenfolge ---
        let mut order_section =
            widget::settings::section().header(padded_heading("Metriken & Reihenfolge"));
        let order = &c.metric_order;
        for (i, id) in order.iter().enumerate() {
            if let Some(kind) = MetricKind::from_u8(*id) {
                let up = widget::button::icon(widget::icon::from_name("go-up-symbolic"))
                    .on_press_maybe((i > 0).then_some(Message::MoveUp(i)));
                let down = widget::button::icon(widget::icon::from_name("go-down-symbolic"))
                    .on_press_maybe((i + 1 < order.len()).then_some(Message::MoveDown(i)));
                let tog = widget::toggler(kind.enabled(c))
                    .on_toggle(move |b| Message::SetMetricShown(kind, b));
                // Toggler als LETZTES Element → gleiche rechte Kante wie die Toggler der `item()`-Zeilen;
                // ▲/▼ gruppiert direkt links davon.
                order_section = order_section.add(widget::settings::item_row(vec![
                    widget::text(kind.label()).width(Length::Fill).into(),
                    up.into(),
                    down.into(),
                    tog.into(),
                ]));
            }
        }

        // --- Anzeige ---
        // Knopf zykliert die Einheit; „⟳" signalisiert die Klick-Aktion, Wert zeigt den aktuellen Stand.
        let net_label = format!(
            "{}  ⟳",
            match c.net_unit {
                1 => "MiB/s (binär)",
                2 => "Mbit/s (Bit)",
                _ => "MB/s (SI)",
            }
        );
        let display_section = widget::settings::section().header(padded_heading("Anzeige"));
        let display_section = toggle_item(display_section, "CPU-Temperatur anzeigen", c.show_cpu_temp, Message::SetCpuTemp);
        let display_section = toggle_item(display_section, "Temperatur in °F", c.fahrenheit, Message::SetFahrenheit);
        let display_section = toggle_item(display_section, "Monospace-Schrift", c.mono_font, Message::SetMonoFont);
        let display_section = toggle_item(display_section, "GPU im Schlaf ausblenden", c.hide_gpu_when_asleep, Message::SetHideGpu);
        let display_section = display_section.add(widget::settings::item(
            "Netz-Einheit",
            widget::button::text(net_label).on_press(Message::CycleNetUnit),
        ));

        // --- Aktualisierung ---
        let interval_section = widget::settings::section()
            .header(padded_heading("Aktualisierung"))
            .add(
            widget::settings::item(
                "Intervall (ms)",
                widget::spin_button(
                    self.config.interval_ms.to_string(),
                    self.config.interval_ms,
                    250u64,
                    250u64,
                    5000u64,
                    Message::SetInterval,
                ),
            ),
        );

        // --- Darstellung ---
        let panel_metric_label = format!(
            "{}  ⟳",
            MetricKind::from_u8(c.panel_metric)
                .unwrap_or(MetricKind::Cpu)
                .label()
        );
        let display2_section = widget::settings::section().header(padded_heading("Darstellung"));
        let display2_section = toggle_item(display2_section, "Balken im Popup (CPU/RAM/GPU)", c.graphical, Message::SetGraphical);
        let display2_section = toggle_item(display2_section, "Wert neben dem Panel-Icon", c.panel_text, Message::SetPanelText);
        let display2_section = display2_section.add(widget::settings::item(
            "Panel-Wert",
            widget::button::text(panel_metric_label).on_press(Message::CyclePanelMetric),
        ));

        // Auf Standard zurücksetzen (schreibt alle Felder neu).
        let reset = widget::container(
            widget::button::standard("Auf Standard zurücksetzen").on_press(Message::ResetDefaults),
        )
        .padding([spacing.space_xs, 0]);

        widget::settings::view_column(vec![
            order_section.into(),
            display_section.into(),
            display2_section.into(),
            interval_section.into(),
            reset.into(),
        ])
        .into()
    }
}

// ---- UI-Helfer ----

/// Fette Metrik-Beschriftung in fester Spaltenbreite (`LABEL_WIDTH`).
fn bold_label<'a>(label: &'static str) -> Element<'a, Message> {
    widget::text(label)
        .font(cosmic::font::bold())
        .width(Length::Fixed(LABEL_WIDTH))
        .into()
}

/// Einzeilige Metrik-Zeile: fettes Label + Wert (optional Monospace für bündige Ziffern).
fn labeled_row<'a>(label: &'static str, value: String, mono: bool) -> Element<'a, Message> {
    let val = widget::text(value);
    let val = if mono {
        val.font(cosmic::iced::Font::MONOSPACE)
    } else {
        val
    };
    widget::row::with_children(vec![bold_label(label), val.into()])
        .spacing(cosmic::theme::spacing().space_xs)
        .align_y(Alignment::Center)
        .into()
}

/// Wert-Text, optional Monospace (für bündige Ziffern).
fn value_text<'a>(s: String, mono: bool) -> Element<'a, Message> {
    let t = widget::text(s);
    if mono {
        t.font(cosmic::iced::Font::MONOSPACE).into()
    } else {
        t.into()
    }
}

/// Drei-Spalten-Zeile (ohne „·"-Trenner): **links** Primärwert (z. B. Auslastung %),
/// **mittig** Detail (RAM-GiB / GPU-VRAM), **rechts** Temp (als fertiges Element, damit es
/// eingefärbt werden kann). Getrennt durch Fill-Spacer → linke und rechte Spalte sind verankert.
fn triple_row<'a>(
    label: &'static str,
    left: String,
    mid: String,
    right: Element<'a, Message>,
    mono: bool,
) -> Element<'a, Message> {
    widget::row::with_children(vec![
        bold_label(label),
        value_text(left, mono),
        widget::space::horizontal().into(),
        value_text(mid, mono),
        widget::space::horizontal().into(),
        right,
    ])
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center)
    .into()
}

/// Temperatur-Zelle: formatiert + nach Schwellen eingefärbt (`warn_temp_c`/`crit_temp_c`).
/// `None` → leere Zelle (kein Sensor).
fn temp_cell<'a>(celsius: Option<f32>, cfg: &Config, mono: bool) -> Element<'a, Message> {
    let Some(c) = celsius else {
        return value_text(String::new(), mono);
    };
    let mut t = widget::text(fmt_temp_val(c, cfg));
    if mono {
        t = t.font(cosmic::iced::Font::MONOSPACE);
    }
    if c >= cfg.crit_temp_c as f32 {
        t = t.class(cosmic::theme::Text::Color(cosmic::iced::Color::from_rgb(
            0.90, 0.22, 0.22, // kritisch: rot
        )));
    } else if c >= cfg.warn_temp_c as f32 {
        t = t.class(cosmic::theme::Text::Color(cosmic::iced::Color::from_rgb(
            0.95, 0.65, 0.15, // Warnung: orange
        )));
    }
    t.into()
}

/// CPU/RAM/GPU als Drei-Spalten-Zeile; bei `graphical` zusätzlich ein Balken (volle Breite) darunter.
fn metric_or_bar<'a>(
    graphical: bool,
    label: &'static str,
    frac: f32,
    left: String,
    mid: String,
    right: Element<'a, Message>,
    mono: bool,
) -> Element<'a, Message> {
    let head = triple_row(label, left, mid, right, mono);
    if graphical {
        let bar = widget::determinate_linear(frac.clamp(0.0, 1.0))
            .width(Length::Fill)
            .girth(Length::Fixed(BAR_GIRTH));
        widget::column::with_children(vec![head, bar.into()])
            .spacing(cosmic::theme::spacing().space_xxs)
            .into()
    } else {
        head
    }
}

/// Abschnitts-Titel mit horizontalem Einzug (`space_m`), damit er nicht am Fensterrand klebt
/// und bündig zu den (ebenfalls eingerückten) Items steht.
fn padded_heading<'a>(title: &'static str) -> Element<'a, Message> {
    widget::container(widget::text::heading(title))
        .padding([0, cosmic::theme::spacing().space_m])
        .into()
}

/// Hängt eine Toggler-Zeile an eine Settings-Section (entfernt die Wiederholung).
fn toggle_item<'a>(
    section: widget::settings::Section<'a, Message>,
    title: &'static str,
    value: bool,
    msg: fn(bool) -> Message,
) -> widget::settings::Section<'a, Message> {
    section.add(widget::settings::item(
        title,
        widget::toggler(value).on_toggle(msg),
    ))
}

fn mem_pct(m: &Metrics) -> f32 {
    if m.mem_total_kb == 0 {
        0.0
    } else {
        m.mem_used_kb as f32 / m.mem_total_kb as f32 * 100.0
    }
}

fn fmt_mem(m: &Metrics) -> String {
    let gib = |kb: u64| kb as f64 / 1024.0 / 1024.0;
    format!("{:.1}/{:.1} GiB", gib(m.mem_used_kb), gib(m.mem_total_kb))
}

fn fmt_temp_val(c: f32, cfg: &Config) -> String {
    if cfg.fahrenheit {
        format!("{:.0}°F", c * 9.0 / 5.0 + 32.0)
    } else {
        format!("{c:.0}°")
    }
}

/// Bytes/s menschenlesbar gemäß gewählter Einheit.
fn fmt_rate(bps: f64, cfg: &Config) -> String {
    match cfg.net_unit {
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

/// GPU-Zeile bei **inaktiver** dGPU — mit Modus (erklärt das Fehlen von Werten).
fn gpu_inactive_text(g: &crate::metrics::gpu::GpuInfo) -> String {
    use crate::metrics::gpu::GpuMode;
    if !g.present {
        return "keine NVIDIA".into();
    }
    if g.mode == GpuMode::Integrated {
        return "inaktiv · integriert".into();
    }
    format!("schläft · {}", g.mode.label())
}

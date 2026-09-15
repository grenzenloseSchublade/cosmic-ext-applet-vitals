// SPDX-License-Identifier: GPL-3.0-only

use crate::config::Config;
use crate::metrics::{Collector, Metrics};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::{time, window::Id, Alignment, Length, Limits, Subscription};
use cosmic::cctk::sctk::reexports::protocols::xdg::shell::client::xdg_positioner::{
    Anchor, Gravity,
};
use cosmic::iced::Rectangle;
use cosmic::prelude::*;
use cosmic::widget;
use iced_runtime::platform_specific::wayland::popup::{SctkPopupSettings, SctkPositioner};
use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

/// Fenster-Id des (einen) Wayland-Metrik-Tooltips — als xdg_popup darf er über
/// den Rand des Metrik-Popups hinausragen (ein iced-Tooltip kann das nicht).
static METRIC_TIP_WINDOW: LazyLock<Id> = LazyLock::new(Id::unique);
/// Autosize-Id für den Tooltip-Inhalt.
static METRIC_TIP_AUTOSIZE: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("metric-tooltip"));

/// Symbolisches Panel-Icon (Prozessor-Chip), eingebettet → kein Theme-Install nötig,
/// wird vom COSMIC-Panel automatisch hell/dunkel eingefärbt.
const CHIP_SYMBOLIC: &[u8] = include_bytes!("../resources/icon-symbolic.svg");

/// Feste Breite der fetten Metrik-Beschriftung (linke Spalte im Popup).
const LABEL_WIDTH: f32 = 60.0;
/// Samples je Verlaufs-Graph (bei 1,5-s-Intervall ≈ 3 Minuten Historie).
const HISTORY_LEN: usize = 120;
/// Höhe der Sparklines (Textzone + Kurvenbereich).
const SPARK_HEIGHT: f32 = 34.0;
/// Oberer Bereich der Sparkline, reserviert für die Beschriftung — die Kurve
/// zeichnet nur darunter, kann den Text also nie überdecken.
const SPARK_TEXT_ZONE: f32 = 11.0;
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
    const PANEL: [MetricKind; 5] = [Self::Cpu, Self::Mem, Self::Net, Self::Gpu, Self::Power];

    fn from_u8(v: u8) -> Option<Self> {
        Self::ALL.get(v as usize).copied()
    }

    /// Die `u8`-ID (= Index in `ALL`).
    const fn id(self) -> u8 {
        match self {
            Self::Cpu => 0,
            Self::Mem => 1,
            Self::Net => 2,
            Self::Gpu => 3,
            Self::Fans => 4,
            Self::Cores => 5,
            Self::Power => 6,
            Self::Battery => 7,
            Self::Disk => 8,
            Self::Swap => 9,
            Self::Load => 10,
            Self::Uptime => 11,
            Self::NetTotal => 12,
        }
    }

    fn label(self) -> &'static str {
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
    fn info(self) -> &'static str {
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

    /// Ob diese Metrik laut Config sichtbar sein soll (mappt auf die `show_*`-Bools).
    fn enabled(self, c: &Config) -> bool {
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
fn normalize_order(order: &[u8]) -> Vec<u8> {
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

/// Anzeigemodus des Popups: Werteliste oder Einstellungen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Metrics,
    Settings,
}

/// Ringpuffer der Verlaufswerte für die Sparklines. Wird bei JEDEM Tick
/// gefüllt (auch bei zugeklapptem Popup — nur so zeigt der Graph beim Öffnen
/// echte Historie); enthält nur Delta-Metriken, die ohnehin immer laufen.
#[derive(Default)]
struct History {
    cpu: VecDeque<f32>,
    mem: VecDeque<f32>,
    net_down: VecDeque<f32>,
    net_up: VecDeque<f32>,
    power: VecDeque<f32>,
    /// Letzter gültiger Watt-Wert: Messlücken (`sys_w == None` beim ersten
    /// Sample, Sanity-Reject, Resume) würden sonst als 0 gepuffert und
    /// zeichneten künstliche Abwärts-Zacken in die Kurve.
    last_power: f32,
}

impl History {
    fn push(&mut self, m: &Metrics) {
        let push = |q: &mut VecDeque<f32>, v: f32| {
            if q.len() == HISTORY_LEN {
                q.pop_front();
            }
            q.push_back(v);
        };
        push(&mut self.cpu, m.cpu_pct);
        push(&mut self.mem, mem_pct(m));
        push(&mut self.net_down, m.net_down_bps as f32);
        push(&mut self.net_up, m.net_up_bps as f32);
        self.last_power = m.power.sys_w.unwrap_or(self.last_power);
        push(&mut self.power, self.last_power);
    }
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
    /// Beim Popup-Öffnen wurde ein Live-Refresh vom In-Flight-Guard verworfen —
    /// nach Abschluss der laufenden Erfassung sofort nachholen.
    pending_live: bool,
    ui_mode: ViewMode,
    /// Verlaufswerte für die Sparklines.
    history: History,
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
    SetGraphs(bool),
    SetGraphCpu(bool),
    SetGraphMem(bool),
    SetGraphNet(bool),
    SetGraphPower(bool),
    SetAccentLabels(bool),
    /// Surface-Aktionen der Wayland-Tooltips (an libcosmic durchgereicht).
    Surface(cosmic::surface::Action),
    SetPanelText(bool),
    SetPowerBreakdown(bool),
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
            // Live-Wunsch nicht verlieren (Popup gerade geöffnet, Tick läuft):
            // nach Abschluss der laufenden Erfassung sofort nachholen.
            if live {
                self.pending_live = true;
            }
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
        let mut config = config_handler
            .as_ref()
            .map(|context| match Config::get_entry(context) {
                Ok(config) => config,
                Err((_errors, config)) => config,
            })
            .unwrap_or_default();
        // Alte gespeicherte Reihenfolgen um neue Metrik-IDs ergänzen.
        config.metric_order = normalize_order(&config.metric_order);

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
            pending_live: false,
            ui_mode: ViewMode::Metrics,
            history: History::default(),
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
                self.history.push(&m);
                self.metrics = m;
                self.refreshing = false;
                if self.pending_live && self.popup.is_some() {
                    self.pending_live = false;
                    return self.spawn_refresh(true);
                }
                self.pending_live = false;
            }
            Message::UpdateConfig(mut config) => {
                config.metric_order = normalize_order(&config.metric_order);
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
                    // Sofort-Refresh: Temps/Lüfter/GPU werden bei zugeklapptem Popup
                    // nicht erhoben — ohne diesen Anstoß blieben sie bis zu einem
                    // vollen Intervall leer.
                    Task::batch([get_popup(popup_settings), self.spawn_refresh(true)])
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
                    MetricKind::Power => c.set_show_power(h, b),
                    MetricKind::Battery => c.set_show_battery(h, b),
                    MetricKind::Disk => c.set_show_disk(h, b),
                    MetricKind::Swap => c.set_show_swap(h, b),
                    MetricKind::Load => c.set_show_load(h, b),
                    MetricKind::Uptime => c.set_show_uptime(h, b),
                    MetricKind::NetTotal => c.set_show_net_total(h, b),
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
            Message::SetGraphs(v) => self.persist(move |c, h| c.set_show_graphs(h, v)),
            Message::SetGraphCpu(v) => self.persist(move |c, h| c.set_graph_cpu(h, v)),
            Message::SetGraphMem(v) => self.persist(move |c, h| c.set_graph_mem(h, v)),
            Message::SetGraphNet(v) => self.persist(move |c, h| c.set_graph_net(h, v)),
            Message::SetGraphPower(v) => self.persist(move |c, h| c.set_graph_power(h, v)),
            Message::SetAccentLabels(v) => self.persist(move |c, h| c.set_accent_labels(h, v)),
            Message::Surface(a) => {
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(a),
                ));
            }
            Message::SetPanelText(v) => self.persist(move |c, h| c.set_panel_text(h, v)),
            Message::SetPowerBreakdown(v) => {
                self.persist(move |c, h| c.set_power_breakdown(h, v));
            }
            Message::CyclePanelMetric => {
                // Über die Position in PANEL zyklieren — die rohen IDs sind nicht
                // lückenlos (Watt = 6). Unbekannte/veraltete Werte fallen auf CPU zurück.
                let pos = MetricKind::from_u8(self.config.panel_metric)
                    .and_then(|k| MetricKind::PANEL.iter().position(|p| *p == k))
                    .unwrap_or(0);
                let next = MetricKind::PANEL[(pos + 1) % MetricKind::PANEL.len()];
                self.persist(move |c, h| c.set_panel_metric(h, next.id()));
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
            // Scrollbar: Die Einstellungsliste ist höher als der Platz unter dem Panel;
            // ohne Scrollen schneidet der Compositor unten ab (Reset-Knopf unerreichbar).
            ViewMode::Settings => widget::scrollable(self.settings_view()).into(),
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
            Some(MetricKind::Power) => m
                .power
                .sys_w
                .map(|w| format!("{w:>5.1} W"))
                .unwrap_or_else(|| "    – W".into()),
            _ => format!("CPU {:>3.0}%", m.cpu_pct),
        }
    }

    /// Werteliste in der konfigurierten Reihenfolge.
    fn metrics_view(&self) -> Element<'_, Message> {
        let mut list = widget::list_column();
        let mut any = false;
        for id in &self.config.metric_order {
            if let Some(kind) = MetricKind::from_u8(*id) {
                let rows = self.metric_rows(kind);
                if rows.is_empty() {
                    continue;
                }
                // Alle Zeilen EINER Metrik (Werte + Balken/Sparkline/Kern-Blöcke)
                // als EIN Listen-Item — die automatischen list_column-Divider
                // trennen nur noch Metriken, nicht deren Binnenzeilen.
                let bundle: Element<'_, Message> = if rows.len() == 1 {
                    rows.into_iter().next().unwrap()
                } else {
                    widget::column::with_children(rows)
                        .spacing(cosmic::theme::spacing().space_xxs)
                        .into()
                };
                list = list.add(bundle);
                any = true;
            }
        }
        if !any {
            list = list.add(widget::text("Keine Metrik aktiv — über das Zahnrad aktivieren."));
        }
        list.into()
    }

    /// Zeitfenster der Sparkline-Historie in Sekunden (Samples × Intervall).
    fn history_span_s(&self) -> u64 {
        HISTORY_LEN as u64 * self.config.interval_ms.max(250) / 1000
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
                let mut rows = vec![metric_or_bar(
                    c.graphical,
                    "CPU",
                    m.cpu_pct / 100.0,
                    format!("{:.0} %", m.cpu_pct),
                    String::new(),
                    temp_cell(temp, c, c.mono_font),
                    c.mono_font,
                    c.accent_labels,
                    self.popup,
                )];
                if c.show_graphs && c.graph_cpu {
                    rows.push(sparkline(SparkSpec {
                        series: vec![series_of(&self.history.cpu)],
                        fixed_max: Some(100.0),
                        caption_max: None,
                        span_s: self.history_span_s(),
                        format_value: Box::new(|v| format!("{v:.0} %")),
                    }));
                }
                rows
            }
            MetricKind::Mem => {
                // links % · mittig GiB-Belegung · rechts RAM-Temp (eigener Sensor, optional).
                let mut rows = vec![metric_or_bar(
                    c.graphical,
                    "RAM",
                    mem_pct(m) / 100.0,
                    format!("{:.0} %", mem_pct(m)),
                    fmt_mem(m),
                    temp_cell(m.ram_temp_c, c, c.mono_font),
                    c.mono_font,
                    c.accent_labels,
                    self.popup,
                )];
                if c.show_graphs && c.graph_mem {
                    rows.push(sparkline(SparkSpec {
                        series: vec![series_of(&self.history.mem)],
                        fixed_max: Some(100.0),
                        caption_max: None,
                        span_s: self.history_span_s(),
                        format_value: Box::new(|v| format!("{v:.0} %")),
                    }));
                }
                rows
            }
            MetricKind::Net => {
                // Typ (WLAN/LAN/VPN) statt rohem Iface-Namen, mit „·"-Trenner (wie GPU/RAM).
                let kind = m.net_kind.map(|k| format!(" · {k}")).unwrap_or_default();
                // Raten rechtsbündig in fester Zeichenbreite → mit Monospace springt die Breite nicht.
                let mut rows = vec![labeled_row(
                    "Netz",
                    format!(
                        "↓ {:>8} ↑ {:>8}{}",
                        fmt_rate(m.net_down_bps, c),
                        fmt_rate(m.net_up_bps, c),
                        kind
                    ),
                    c.mono_font,
                    c.accent_labels,
                    self.popup,
                )];
                if c.show_graphs && c.graph_net {
                    // ↓ voll, ↑ gedimmt; gemeinsames Maximum (autoskaliert).
                    let peak =
                        peak_of(self.history.net_down.iter().chain(self.history.net_up.iter()));
                    rows.push(sparkline(SparkSpec {
                        series: vec![
                            series_of(&self.history.net_down),
                            series_of(&self.history.net_up),
                        ],
                        fixed_max: None,
                        caption_max: (peak > 0.0).then(|| fmt_rate(peak as f64, c)),
                        span_s: self.history_span_s(),
                        format_value: {
                            let unit = c.net_unit;
                            Box::new(move |v| fmt_rate_unit(v as f64, unit))
                        },
                    }));
                }
                rows
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
                    (Some(t), _) => vec![labeled_row("GPU", t, c.mono_font, c.accent_labels, self.popup)],
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
                            c.accent_labels,
                            self.popup,
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
                    vec![labeled_row("Lüfter", format!("{fans} rpm"), c.mono_font, c.accent_labels, self.popup)]
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
                        labeled_row(label, cores, true, c.accent_labels, self.popup)
                    })
                    .collect()
            }
            MetricKind::Power => {
                let p = &m.power;
                let gpu_w = m.gpu.power_w;
                // Keine Quelle verfügbar (Desktop ohne Akku, RAPL gesperrt) → Zeile weglassen.
                if p.sys_w.is_none()
                    && p.cpu_pkg_w.is_none()
                    && gpu_w.is_none()
                    && !p.sys_from_battery
                {
                    return Vec::new();
                }
                let total = match p.sys_w {
                    Some(w) => format!("{w:.1} W"),
                    // Akku-Fallback am Netz: nur die Laderate wäre messbar → bewusst „–".
                    None if p.sys_from_battery => "– · Netz".into(),
                    None => "–".into(),
                };
                // Gesamt (psys) enthält CPU/GPU bereits — Aufschlüsselung ist optional
                // und kompakt ohne Einheiten-Wiederholung, damit die Zeile nicht umbricht.
                let mut parts = vec![total];
                if c.power_breakdown {
                    if let Some(w) = p.cpu_pkg_w {
                        parts.push(format!("CPU {w:.1}"));
                    }
                    if let Some(w) = gpu_w {
                        parts.push(format!("GPU {w:.1}"));
                    }
                }
                let mut rows = vec![labeled_row(
                    "Watt",
                    parts.join(" · "),
                    c.mono_font,
                    c.accent_labels,
                    self.popup,
                )];
                if c.show_graphs && c.graph_power {
                    let peak = peak_of(self.history.power.iter());
                    rows.push(sparkline(SparkSpec {
                        series: vec![series_of(&self.history.power)],
                        fixed_max: None,
                        caption_max: (peak > 0.0).then(|| format!("{peak:.0} W")),
                        span_s: self.history_span_s(),
                        format_value: Box::new(|v| format!("{v:.1} W")),
                    }));
                }
                rows
            }
            MetricKind::Battery => {
                use crate::metrics::power::BatStatus;
                let p = &m.power;
                // Kein Akku (Desktop) → Zeile weglassen.
                let Some(v) = p.bat_voltage_v else {
                    return Vec::new();
                };
                let mut parts = vec![format!("{v:.2} V")];
                match p.bat_status {
                    BatStatus::Charging => parts.push("lädt".into()),
                    BatStatus::Discharging => parts.push("entlädt".into()),
                    BatStatus::Full => parts.push("voll · Netz".into()),
                    BatStatus::Unknown => {}
                }
                // Lade-/Entladeleistung nur zeigen, wenn tatsächlich Strom fließt.
                if matches!(p.bat_status, BatStatus::Charging | BatStatus::Discharging) {
                    if let Some(w) = p.bat_power_w {
                        parts.push(format!("{w:.1} W"));
                    }
                }
                // Näherung „Zug am Netzteil" (psys + Ladeleistung), nur beim Laden.
                if let Some(cw) = p.charger_w {
                    parts.push(format!("Netzteil ≈ {cw:.0} W"));
                }
                vec![labeled_row("Akku", parts.join(" · "), c.mono_font, c.accent_labels, self.popup)]
            }
            MetricKind::Disk => {
                // Gleiche Pfeil-Konvention wie Netz: ↓ Lesen, ↑ Schreiben.
                vec![labeled_row(
                    "Disk",
                    format!(
                        "↓ {:>8} ↑ {:>8}",
                        fmt_rate(m.disk_read_bps, c),
                        fmt_rate(m.disk_write_bps, c)
                    ),
                    c.mono_font,
                    c.accent_labels,
                    self.popup,
                )]
            }
            MetricKind::Swap => {
                // Kein Swap eingerichtet → Zeile weglassen.
                if m.swap_total_kb == 0 {
                    return Vec::new();
                }
                let pct = m.swap_used_kb as f32 / m.swap_total_kb as f32 * 100.0;
                vec![labeled_row(
                    "Swap",
                    format!(
                        "{:.0} %  {:.1}/{:.1} GiB",
                        pct,
                        m.swap_used_kb as f32 / (1024.0 * 1024.0),
                        m.swap_total_kb as f32 / (1024.0 * 1024.0)
                    ),
                    c.mono_font,
                    c.accent_labels,
                    self.popup,
                )]
            }
            MetricKind::Load => {
                let Some((l1, l5, l15)) = m.loadavg else {
                    return Vec::new();
                };
                vec![labeled_row(
                    "Load",
                    format!("{l1:.2}  {l5:.2}  {l15:.2}"),
                    c.mono_font,
                    c.accent_labels,
                    self.popup,
                )]
            }
            MetricKind::Uptime => {
                let Some(s) = m.uptime_s else {
                    return Vec::new();
                };
                vec![labeled_row(
                    "Uptime",
                    fmt_uptime(s),
                    c.mono_font,
                    c.accent_labels,
                    self.popup,
                )]
            }
            MetricKind::NetTotal => {
                if m.net_iface.is_none() {
                    return Vec::new();
                }
                vec![labeled_row(
                    "Netz Σ",
                    format!(
                        "↓ {:>9} ↑ {:>9}",
                        fmt_bytes(m.net_total_rx),
                        fmt_bytes(m.net_total_tx)
                    ),
                    c.mono_font,
                    c.accent_labels,
                    self.popup,
                )]
            }
        }
    }

    /// Einstellungs-Ansicht: Metriken+Reihenfolge, Anzeige-Optionen, Intervall.
    fn settings_view(&self) -> Element<'_, Message> {
        let c = &self.config;
        let spacing = cosmic::theme::spacing();

        // --- Metriken & Reihenfolge ---
        let mut order_section = widget::settings::section().header(padded_heading_info(
            "Metriken & Reihenfolge",
            "▲/▼ ändert die Reihenfolge im Popup; der Schalter blendet die Metrik ein/aus.",
        ));
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
                    info_label(kind.label(), kind.info()),
                    up.into(),
                    down.into(),
                    tog.into(),
                ]));
            }
        }

        // --- Inhalt: WAS im Popup erscheint ---
        let content_section = widget::settings::section().header(padded_heading("Inhalt"));
        let content_section = toggle_item(
            content_section,
            "CPU-Temperatur anzeigen",
            "Zeigt die CPU-Temperatur zusätzlich in der CPU-Zeile (Quelle: hwmon).",
            c.show_cpu_temp,
            Message::SetCpuTemp,
        );
        let content_section = toggle_item(
            content_section,
            "GPU im Schlaf ausblenden",
            "Blendet die GPU-Zeile aus, wenn die dGPU per Runtime-PM schläft; verhindert unnötiges Aufwecken.",
            c.hide_gpu_when_asleep,
            Message::SetHideGpu,
        );
        let content_section = toggle_item(
            content_section,
            "Watt aufschlüsseln",
            "Zeigt System/CPU/GPU-Leistung als getrennte Werte statt nur des Gesamtwerts.",
            c.power_breakdown,
            Message::SetPowerBreakdown,
        );

        // --- Format: WIE Werte formatiert sind ---
        // Knopf zykliert die Einheit; „⟳" signalisiert die Klick-Aktion, Wert zeigt den aktuellen Stand.
        let net_label = format!(
            "{}  ⟳",
            match c.net_unit {
                1 => "MiB/s (binär)",
                2 => "Mbit/s (Bit)",
                _ => "MB/s (SI)",
            }
        );
        let format_section = widget::settings::section().header(padded_heading("Format"));
        let format_section = toggle_item(
            format_section,
            "Temperatur in °F",
            "Alle Temperaturen in Fahrenheit statt Celsius.",
            c.fahrenheit,
            Message::SetFahrenheit,
        );
        let format_section = format_section.add(widget::settings::item_row(vec![
            info_label(
                "Netz-Einheit",
                "Einheit für Netzwerk-Durchsatz: MB/s (SI, 10⁶), MiB/s (binär, 2²⁰) oder Mbit/s. Klick wechselt.",
            ),
            widget::button::text(net_label)
                .on_press(Message::CycleNetUnit)
                .into(),
        ]));
        let format_section = toggle_item(
            format_section,
            "Monospace-Schrift",
            "Feste Zeichenbreite — Werte springen beim Aktualisieren nicht.",
            c.mono_font,
            Message::SetMonoFont,
        );

        // --- Aktualisierung ---
        let interval_section = widget::settings::section()
            .header(padded_heading("Aktualisierung"))
            .add(widget::settings::item_row(vec![
                info_label(
                    "Intervall (ms)",
                    "Abstand zwischen Messungen (250–5000 ms). Kürzer = aktueller, minimal mehr CPU-Last.",
                ),
                widget::spin_button(
                    self.config.interval_ms.to_string(),
                    self.config.interval_ms,
                    250u64,
                    250u64,
                    5000u64,
                    Message::SetInterval,
                )
                .into(),
            ]));

        // --- Darstellung: Optik des Popups ---
        let display_section = widget::settings::section().header(padded_heading("Darstellung"));
        let display_section = toggle_item(
            display_section,
            "Balken im Popup (CPU/RAM/GPU)",
            "Zeigt Auslastung zusätzlich als Fortschrittsbalken statt nur als Zahl.",
            c.graphical,
            Message::SetGraphical,
        );
        let display_section = toggle_item(
            display_section,
            "Verlaufs-Graphen (Sparklines)",
            "Zeigt unter CPU, RAM, Netz und Watt einen Mini-Verlauf der letzten ~3 Minuten; die Historie läuft auch bei geschlossenem Popup mit.",
            c.show_graphs,
            Message::SetGraphs,
        );
        let display_section = sub_toggle_item(
            display_section,
            "Verlauf CPU",
            "Sparkline unter der CPU-Zeile (nur bei aktiven Verlaufs-Graphen).",
            c.graph_cpu,
            Message::SetGraphCpu,
        );
        let display_section = sub_toggle_item(
            display_section,
            "Verlauf RAM",
            "Sparkline unter der RAM-Zeile (nur bei aktiven Verlaufs-Graphen).",
            c.graph_mem,
            Message::SetGraphMem,
        );
        let display_section = sub_toggle_item(
            display_section,
            "Verlauf Netz",
            "Sparkline unter der Netz-Zeile: ↓ voll, ↑ gedimmt (nur bei aktiven Verlaufs-Graphen).",
            c.graph_net,
            Message::SetGraphNet,
        );
        let display_section = sub_toggle_item(
            display_section,
            "Verlauf Watt",
            "Sparkline unter der Watt-Zeile (nur bei aktiven Verlaufs-Graphen).",
            c.graph_power,
            Message::SetGraphPower,
        );
        let display_section = toggle_item(
            display_section,
            "Beschriftungen in Akzentfarbe",
            "Färbt die fetten Metrik-Beschriftungen im Popup in der System-Akzentfarbe (COSMIC-Einstellungen → Desktop → Erscheinungsbild).",
            c.accent_labels,
            Message::SetAccentLabels,
        );

        // --- Panel: Anzeige in der Leiste ---
        let panel_metric_label = format!(
            "{}  ⟳",
            MetricKind::from_u8(c.panel_metric)
                .unwrap_or(MetricKind::Cpu)
                .label()
        );
        let panel_section = widget::settings::section().header(padded_heading("Panel"));
        let panel_section = toggle_item(
            panel_section,
            "Wert neben dem Panel-Icon",
            "Zeigt den gewählten Messwert als Text direkt im Panel (nur horizontale Leiste).",
            c.panel_text,
            Message::SetPanelText,
        );
        let panel_section = panel_section.add(widget::settings::item_row(vec![
            indented(info_label(
                "Panel-Wert",
                "Welche Metrik neben dem Panel-Icon steht (CPU, RAM, Netz, GPU oder Watt). Klick wechselt.",
            )),
            widget::button::text(panel_metric_label)
                .on_press(Message::CyclePanelMetric)
                .into(),
        ]));

        // Auf Standard zurücksetzen (schreibt alle Felder neu).
        // Horizontal `space_m` — gleiche Einzugs-Konvention wie `padded_heading`,
        // damit der Button bündig zu Headings und Section-Karten steht.
        let reset = widget::container(info_box(
            widget::button::standard("Auf Standard zurücksetzen")
                .on_press(Message::ResetDefaults),
            "Setzt alle Einstellungen auf die Standardwerte zurück.",
            widget::tooltip::Position::Top,
        ))
        .padding([spacing.space_xs, spacing.space_m]);

        widget::settings::view_column(vec![
            order_section.into(),
            content_section.into(),
            format_section.into(),
            display_section.into(),
            panel_section.into(),
            interval_section.into(),
            reset.into(),
        ])
        .into()
    }
}

// ---- UI-Helfer ----

/// Erklärtext zur Metrik-Zeile der Hauptansicht: was die Anzeige konkret
/// bedeutet (Spalten, Zustände, Quellen). Schlüssel ist das angezeigte Label —
/// die Labels sind kanonisch (`MetricKind::label()` bzw. „Kerne %").
fn metric_value_info(label: &'static str) -> Option<&'static str> {
    // Muster: Kopfzeile (was die Zeile zeigt), darunter „•"-Punkte je
    // Spalte/Zustand — statt Semikolon-Fließtext.
    Some(match label {
        "CPU" => "Gesamtauslastung aller Kerne.\n\
            • links % · rechts Paket-Temperatur (hwmon)\n\
            • Verlauf: Hovern zeigt Wert und Zeitpunkt",
        "RAM" => "Arbeitsspeicher-Belegung.\n\
            • links % · Mitte belegt/gesamt GiB\n\
            • rechts RAM-Temperatur (falls Sensor vorhanden)\n\
            • Verlauf: Hovern zeigt Wert und Zeitpunkt",
        "Netz" => "Datenrate der aktiven Schnittstelle.\n\
            • ↓ empfangen · ↑ senden · Typ (WLAN/LAN/VPN)\n\
            • Einheit unter „Netz-Einheit“ wählbar\n\
            • Verlauf: Skala 0…≤ Fenster-Maximum, ↑ gedimmt; Hovern zeigt Werte und Zeitpunkt",
        "GPU" => "Dedizierte NVIDIA-GPU (NVML).\n\
            • „schläft“ — Stromsparmodus, wird nie geweckt\n\
            • „keine NVIDIA“ — keine dGPU gefunden\n\
            • „aktiv · Modus“ — wach, ohne Live-Werte\n\
            • sonst: Auslastung % · VRAM · Temperatur",
        "Lüfter" => "Drehzahlen aller erkannten Lüfter (hwmon), in U/min.",
        "Kerne %" => "Auslastung je CPU-Kern in Prozent, Reihen zu je 6 Kernen.",
        "Watt" => "Leistungsaufnahme des Systems.\n\
            • Gesamt: RAPL psys — „– · Netz“ heißt: am Netz nicht messbar (nur Akku-Messung verfügbar)\n\
            • CPU-Package · GPU (Schalter „Watt aufschlüsseln“)\n\
            • beim Laden: „Netzteil ≈“ psys + Ladeleistung\n\
            • Verlauf: Skala 0…≤ Fenster-Maximum; Hovern zeigt Wert und Zeitpunkt",
        "Akku" => "Akku-Zustand.\n\
            • Spannung (V) · Status (lädt/entlädt/voll)\n\
            • Lade-/Entladeleistung in W, nur wenn Strom fließt",
        "Disk" => "Datenrate aller physischen Laufwerke.\n\
            • ↓ lesen · ↑ schreiben\n\
            • Partitionen/virtuelle Devices nicht doppelt gezählt\n\
            • Quelle: /proc/diskstats",
        "Swap" => "Auslagerungsspeicher: % und belegt/gesamt GiB.\n\
            • Zeile erscheint nur, wenn Swap eingerichtet ist",
        "Load" => "Load Average 1 / 5 / 15 min.\n\
            • Ø lauffähige Prozesse; Werte über der Kernzahl bedeuten Wartezeiten",
        "Uptime" => "Zeit seit dem letzten Systemstart.",
        "Netz Σ" => "Summe seit Systemstart (aktive Schnittstelle).\n\
            • ↓ empfangen · ↑ gesendet\n\
            • bei Wechsel WLAN↔LAN zählt die neue ab ihrem Stand",
        _ => return None,
    })
}

/// Fette Metrik-Beschriftung in fester Spaltenbreite (`LABEL_WIDTH`);
/// optional in der System-Akzentfarbe (`accent_labels`). Gibt es einen
/// Erklärtext, ist NUR das Wort hoverbar; die Info-Box öffnet als
/// Wayland-Popup links vom Wort — über den Fensterrand hinaus, damit sie
/// die Werte nicht überdeckt (`parent` = Fenster-Id des Metrik-Popups).
fn bold_label<'a>(label: &'static str, accent: bool, parent: Option<Id>) -> Element<'a, Message> {
    let mut t = widget::text(label).font(cosmic::font::bold());
    if accent {
        t = t.class(cosmic::theme::Text::Accent);
    }
    let word: Element<'a, Message> = match (metric_value_info(label), parent) {
        (Some(info), Some(parent)) => metric_tooltip(t, info, parent),
        _ => t.into(),
    };
    // Feste Spaltenbreite außen — der Tooltip bleibt aufs Wort begrenzt.
    widget::container(word)
        .width(Length::Fixed(LABEL_WIDTH))
        .into()
}

/// Einzeilige Metrik-Zeile: fettes Label + Wert (optional Monospace für bündige Ziffern).
fn labeled_row<'a>(
    label: &'static str,
    value: String,
    mono: bool,
    accent: bool,
    parent: Option<Id>,
) -> Element<'a, Message> {
    let val = widget::text(value);
    let val = if mono {
        val.font(cosmic::iced::Font::MONOSPACE)
    } else {
        val
    };
    widget::row::with_children(vec![bold_label(label, accent, parent), val.into()])
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
    accent: bool,
    parent: Option<Id>,
) -> Element<'a, Message> {
    widget::row::with_children(vec![
        bold_label(label, accent, parent),
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
    accent: bool,
    parent: Option<Id>,
) -> Element<'a, Message> {
    let head = triple_row(label, left, mid, right, mono, accent, parent);
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

/// Rendert einen Info-Text strukturiert: Zeilen mit „• "-Präfix werden als
/// echte Aufzählung gesetzt (Bullet-Spalte + hängender Einzug beim Umbruch),
/// alle anderen Zeilen als normale Absätze. DIE eine Formatierung aller
/// Info-Boxen (Einstellungen, Hauptansicht, Reset-Button).
fn info_content<'a, M: 'a>(info: &'static str) -> Element<'a, M> {
    let mut col = widget::column::with_capacity(info.lines().count())
        .spacing(cosmic::theme::spacing().space_xxxs);
    for line in info.lines() {
        let el: Element<'a, M> = match line.strip_prefix("• ") {
            Some(rest) => widget::row::with_capacity(2)
                .push(widget::text("•").width(Length::Fixed(12.0)))
                .push(widget::text(rest).width(Length::Fill))
                .into(),
            None => widget::text(line).into(),
        };
        col = col.push(el);
    }
    col.into()
}

/// Einheitlich gestylte Info-Box um beliebigen Inhalt — die EINE Stelle für
/// Optik aller Erklär-Tooltips. Wie `Container::Dropdown` (Komponenten-
/// Hintergrund, der die Box sichtbar vom Popup absetzt — der Standard-Tooltip
/// mit `neutral_2` ist fast flächengleich), aber mit 1-px-Rand in der
/// System-Akzentfarbe statt Divider-Grau; Padding `space_s` statt der hart
/// verdrahteten `space_xxs` des Wrappers.
fn info_box<'a>(
    content: impl Into<Element<'a, Message>>,
    info: &'static str,
    position: widget::tooltip::Position,
) -> Element<'a, Message> {
    widget::tooltip(
        content,
        widget::container(info_content(info)).max_width(300.0),
        position,
    )
    .class(cosmic::theme::Container::custom(info_box_style))
    .padding(cosmic::theme::spacing().space_s)
    .into()
}

/// Der gemeinsame Look aller Info-Boxen: Komponenten-Hintergrund + 1-px-Rand
/// in der System-Akzentfarbe (genutzt vom iced-Tooltip UND vom Wayland-Tooltip).
fn info_box_style(theme: &cosmic::Theme) -> cosmic::iced::widget::container::Style {
    let t = theme.cosmic();
    cosmic::iced::widget::container::Style {
        icon_color: None,
        text_color: None,
        background: Some(cosmic::iced::Background::Color(
            t.bg_component_color().into(),
        )),
        border: cosmic::iced::Border {
            color: t.accent_color().into(),
            width: 1.0,
            radius: t.corner_radii.radius_s.into(),
        },
        shadow: Default::default(),
        snap: true,
    }
}

/// Info-Box als ECHTES Wayland-Popup (xdg_popup): darf über die Ränder des
/// Metrik-Popups hinausragen — ein iced-Tooltip wird dagegen ins Fenster
/// geklemmt und läge über den Werten. Öffnet links vom Anker (`Anchor::Left`);
/// `constraint_adjustment` lässt den Compositor am Bildschirmrand ausweichen.
fn metric_tooltip<'a>(
    content: impl Into<Element<'a, Message>>,
    info: &'static str,
    parent: Id,
) -> Element<'a, Message> {
    cosmic::widget::wayland::tooltip::widget::Tooltip::<'a, Message, Message>::new(
        content,
        Some(move |bounds: Rectangle| SctkPopupSettings {
            parent,
            id: *METRIC_TIP_WINDOW,
            grab: false,
            // Eingaben gehen am Tooltip vorbei (wie beim libcosmic-Applet-Tooltip).
            input_zone: Some(vec![Rectangle::new(
                cosmic::iced::Point::new(-1000.0, -1000.0),
                cosmic::iced::Size::default(),
            )]),
            positioner: SctkPositioner {
                size: None,
                size_limits: Limits::NONE.min_width(1.0).min_height(1.0).max_width(300.0),
                anchor_rect: Rectangle {
                    x: bounds.x.round() as i32,
                    y: bounds.y.round() as i32,
                    width: bounds.width.round() as i32,
                    height: bounds.height.round() as i32,
                },
                anchor: Anchor::Left,
                gravity: Gravity::Left,
                // slide|flip: am Bildschirmrand verschieben/spiegeln statt abschneiden.
                constraint_adjustment: 15,
                offset: (-4, 0),
                reactive: true,
            },
            parent_size: None,
            close_with_children: true,
        }),
        move || {
            widget::autosize::autosize(
                widget::container(info_content(info))
                    .max_width(300.0)
                    .padding(cosmic::theme::spacing().space_s)
                    .class(cosmic::theme::Container::custom(info_box_style)),
                METRIC_TIP_AUTOSIZE.clone(),
            )
            .into()
        },
        Message::Surface(cosmic::surface::Action::DestroyPopup(*METRIC_TIP_WINDOW)),
        Message::Surface,
    )
    .delay(Duration::from_millis(150))
    .into()
}

/// Info-Icon (ⓘ), das beim Hovern eine Erklärbox zeigt.
fn info_icon<'a>(
    info: &'static str,
    position: widget::tooltip::Position,
) -> Element<'a, Message> {
    info_box(
        widget::icon::from_name("dialog-information-symbolic").size(14),
        info,
        position,
    )
}

/// Label mit Info-Icon daneben; füllt die Breite, damit Controls rechtsbündig bleiben.
fn info_label<'a>(title: &'static str, info: &'static str) -> Element<'a, Message> {
    widget::row::with_capacity(2)
        .push(widget::text(title))
        .push(info_icon(info, widget::tooltip::Position::Top))
        .spacing(cosmic::theme::spacing().space_xxs)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// Abschnitts-Titel mit horizontalem Einzug (`space_m`), damit er nicht am Fensterrand klebt
/// und bündig zu den (ebenfalls eingerückten) Items steht.
fn padded_heading<'a>(title: &'static str) -> Element<'a, Message> {
    widget::container(widget::text::heading(title))
        .padding([0, cosmic::theme::spacing().space_m])
        .into()
}

/// Wie `padded_heading`, zusätzlich mit Info-Icon hinter dem Titel.
fn padded_heading_info<'a>(title: &'static str, info: &'static str) -> Element<'a, Message> {
    widget::container(
        widget::row::with_capacity(2)
            .push(widget::text::heading(title))
            // Bottom: Header sitzt ganz oben — Tooltip nach oben würde am Popup-Rand clippen.
            .push(info_icon(info, widget::tooltip::Position::Bottom))
            .spacing(cosmic::theme::spacing().space_xxs)
            .align_y(Alignment::Center),
    )
    .padding([0, cosmic::theme::spacing().space_m])
    .into()
}

/// Rückt ein Element als Unterpunkt ein (gemeinsame Einzugs-Konvention für
/// Settings-Unterzeilen; Breite bleibt Fill, damit Controls bündig bleiben).
fn indented<'a>(el: Element<'a, Message>) -> Element<'a, Message> {
    widget::container(el)
        .padding([0, 0, 0, cosmic::theme::spacing().space_m])
        .width(Length::Fill)
        .into()
}

/// Hängt eine Toggler-Zeile mit Info-Icon an eine Settings-Section (entfernt die Wiederholung).
fn toggle_item<'a>(
    section: widget::settings::Section<'a, Message>,
    title: &'static str,
    info: &'static str,
    value: bool,
    msg: fn(bool) -> Message,
) -> widget::settings::Section<'a, Message> {
    section.add(widget::settings::item_row(vec![
        info_label(title, info),
        widget::toggler(value).on_toggle(msg).into(),
    ]))
}

/// Wie `toggle_item`, aber als eingerückter Unterpunkt (z. B. die
/// Pro-Metrik-Schalter unter dem Verlaufs-Master).
fn sub_toggle_item<'a>(
    section: widget::settings::Section<'a, Message>,
    title: &'static str,
    info: &'static str,
    value: bool,
    msg: fn(bool) -> Message,
) -> widget::settings::Section<'a, Message> {
    section.add(widget::settings::item_row(vec![
        indented(info_label(title, info)),
        widget::toggler(value).on_toggle(msg).into(),
    ]))
}

fn mem_pct(m: &Metrics) -> f32 {
    if m.mem_total_kb == 0 {
        0.0
    } else {
        m.mem_used_kb as f32 / m.mem_total_kb as f32 * 100.0
    }
}

/// Sparkline-Verlauf als Canvas: Linie + zart gefüllte Fläche in der
/// System-Akzentfarbe; zweite Serie (Netz ↑) gedimmt. Neueste Werte rechts,
/// die x-Achse ist auf `HISTORY_LEN` fixiert — der Graph „läuft" von rechts ein.
struct Sparkline {
    series: Vec<Vec<f32>>,
    /// Normierungs-Maximum; `None` = gemeinsames Maximum der Serien (autoskaliert).
    fixed_max: Option<f32>,
    /// Fertig formatiertes Skalen-Maximum für die Beschriftung (nur autoskaliert).
    caption_max: Option<String>,
    /// Zeitfenster-Beschriftung („3 min").
    span_label: String,
    /// Zeitfenster in Sekunden (für den Zeit-Offset beim Hovern).
    span_s: u64,
    /// Formatiert einen Rohwert der Serie für die Hover-Anzeige.
    format_value: Box<dyn Fn(f32) -> String>,
}

/// Strichbreite der Sparkline-Kurven; `SPARK_HW` = halbe Breite, um den
/// Zeichenbereich so einzurücken, dass die Antialiasing-Kante nie an der
/// Canvas-Clip-Grenze abgeschnitten wird (sonst „dicke Grundlinie").
const SPARK_STROKE_W: f32 = 1.5;
const SPARK_HW: f32 = SPARK_STROKE_W / 2.0;

/// Der EINE Strich-Stil aller Sparklines: runde Ecken/Enden statt der
/// Miter-Defaults, die bei zackigen Daten (Netz-Peaks) Spitzen erzeugen.
fn spark_stroke(color: cosmic::iced::Color) -> widget::canvas::Stroke<'static> {
    widget::canvas::Stroke::default()
        .with_color(color)
        .with_width(SPARK_STROKE_W)
        .with_line_join(widget::canvas::LineJoin::Round)
        .with_line_cap(widget::canvas::LineCap::Round)
}

/// Hauchfeine gepunktete horizontale Regel-Linie über die volle Breite —
/// die gemeinsame Punkt-Optik von Null-Linie (unten) und Max-Referenzlinie (oben).
fn dotted_rule(frame: &mut widget::canvas::Frame, y: f32, w: f32, color: cosmic::iced::Color) {
    use cosmic::iced::Point;
    const DASH: [f32; 2] = [1.0, 3.0];
    frame.stroke(
        &widget::canvas::Path::line(Point::new(0.0, y), Point::new(w, y)),
        widget::canvas::Stroke {
            line_dash: widget::canvas::LineDash {
                segments: &DASH,
                offset: 0,
            },
            ..spark_stroke(color).with_width(1.0)
        },
    );
}

/// Kurvenpfad einer Serie: move_to zum ersten Punkt, line_to zu allen weiteren.
/// Mit `close_to_base = Some(base)` wird der Pfad zusätzlich über die
/// Basislinie geschlossen → gefüllte Fläche unter der Kurve.
fn curve_path(
    data: &[f32],
    x_of: impl Fn(usize) -> f32,
    y_of: impl Fn(f32) -> f32,
    close_to_base: Option<f32>,
) -> widget::canvas::Path {
    use cosmic::iced::Point;
    widget::canvas::Path::new(|b| {
        if let Some(base) = close_to_base {
            b.move_to(Point::new(x_of(0), base));
            b.line_to(Point::new(x_of(0), y_of(data[0])));
        } else {
            b.move_to(Point::new(x_of(0), y_of(data[0])));
        }
        for (i, &v) in data.iter().enumerate().skip(1) {
            b.line_to(Point::new(x_of(i), y_of(v)));
        }
        if let Some(base) = close_to_base {
            b.line_to(Point::new(x_of(data.len() - 1), base));
            b.close();
        }
    })
}

impl Sparkline {
    /// x-Position eines Sample-Index einer Serie der Länge `len`:
    /// die x-Achse ist auf `HISTORY_LEN` fixiert, kürzere Historie läuft
    /// von rechts ein (links Leerraum).
    fn x_of(w: f32, len: usize, i: usize) -> f32 {
        let n = HISTORY_LEN.max(2) as f32;
        w * ((i + HISTORY_LEN - len) as f32) / (n - 1.0)
    }

    /// y-Position eines Werts. Zeichenbereich [Textzone + halbe Strichbreite,
    /// h - halbe Strichbreite]: oben bleibt die Beschriftung frei, unten wird
    /// die Linie nie an der Canvas-Kante angeschnitten. Werte > 0 halten
    /// zusätzlich einen Mindestabstand zur Basiskante — eine flache, basisnahe
    /// Kurve (Watt im Leerlauf unter großem Fenster-Peak) verschmölze sonst
    /// mit Flächen-Kante und Grundlinie zu einem dicken Balken.
    fn y_of(h: f32, max: f32, v: f32) -> f32 {
        let top = SPARK_TEXT_ZONE + SPARK_HW;
        let y = h - (v / max).clamp(0.0, 1.0) * (h - top - SPARK_HW) - SPARK_HW;
        if v > 0.0 {
            y.min(h - SPARK_HW - 2.5)
        } else {
            y
        }
    }
}

impl<Message> widget::canvas::Program<Message, cosmic::Theme> for Sparkline {
    /// Hover-Position (bounds-relativ); `None` = Cursor nicht über dem Graph.
    type State = Option<cosmic::iced::Point>;

    fn update(
        &self,
        state: &mut Self::State,
        event: &widget::canvas::Event,
        bounds: Rectangle,
        cursor: cosmic::iced::mouse::Cursor,
    ) -> Option<widget::canvas::Action<Message>> {
        use cosmic::iced::mouse;
        if let widget::canvas::Event::Mouse(
            mouse::Event::CursorMoved { .. }
            | mouse::Event::CursorEntered
            | mouse::Event::CursorLeft,
        ) = event
        {
            let new = cursor.position_in(bounds);
            if *state != new {
                *state = new;
                return Some(widget::canvas::Action::request_redraw());
            }
        }
        None
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        _bounds: Rectangle,
        _cursor: cosmic::iced::mouse::Cursor,
    ) -> cosmic::iced::mouse::Interaction {
        if state.is_some() {
            cosmic::iced::mouse::Interaction::Crosshair
        } else {
            cosmic::iced::mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: cosmic::iced::mouse::Cursor,
    ) -> Vec<widget::canvas::Geometry> {
        use cosmic::iced::Point;
        use widget::canvas::{Frame, Path};
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let raw_max = self.fixed_max.unwrap_or_else(|| {
            self.series
                .iter()
                .flatten()
                .fold(0.0f32, |a, &v| a.max(v))
        });
        let max = raw_max.max(1e-6);
        let accent: cosmic::iced::Color = theme.cosmic().accent_color().into();

        // Nur-Null-Historie (z. B. Watt ohne psys): nichts zeichnen —
        // eine flache Linie auf der Grundkante wäre nur Rauschen.
        let has_data = self.fixed_max.is_some() || raw_max > 0.0;

        // Bewusst KEINE explizite Null-Linie an der Unterkante (ausprobiert,
        // wieder entfernt): Kurve + zarte Fläche wirken ohne sie ruhiger.
        for (si, data) in self.series.iter().enumerate() {
            if data.len() < 2 || !has_data || data.iter().all(|&v| v <= 0.0) {
                continue;
            }
            let x_of = |i: usize| Self::x_of(w, data.len(), i);
            let y_of = |v: f32| Self::y_of(h, max, v);
            // Erste Serie voll, weitere gedimmt (Netz ↑ neben ↓).
            let alpha = if si == 0 { 1.0 } else { 0.45 };
            let mut color = accent;
            color.a = alpha;

            if si == 0 {
                let line = curve_path(data, x_of, y_of, None);
                frame.stroke(&line, spark_stroke(color));

                // Fläche unter der ersten Serie, sehr zart; endet an derselben
                // Basislinie wie die Kurve (kein Haarspalt zur Linie).
                let area = curve_path(data, x_of, y_of, Some(h - SPARK_HW));
                let mut fill = accent;
                fill.a = 0.12;
                frame.fill(&area, fill);
            } else {
                // Zweitserie (Netz ↑): NUR dort zeichnen, wo Traffic ist —
                // sonst legt sich ihre Null-Linie auf die Basislinie der
                // Erstserie und die addierte Deckung wirkt „fett". Läufe mit
                // v > 0 werden um je einen Nachbarpunkt erweitert, damit die
                // Flanken vollständig sind.
                let mut run_start: Option<usize> = None;
                let draw_run = |from: usize, to: usize, frame: &mut Frame| {
                    let a = from.saturating_sub(1);
                    let b_end = (to + 1).min(data.len() - 1);
                    if b_end <= a {
                        return;
                    }
                    let seg = curve_path(&data[a..=b_end], |i| x_of(i + a), y_of, None);
                    frame.stroke(&seg, spark_stroke(color));
                };
                for (i, &v) in data.iter().enumerate() {
                    if v > 0.0 {
                        run_start.get_or_insert(i);
                    } else if let Some(s) = run_start.take() {
                        draw_run(s, i - 1, &mut frame);
                    }
                }
                if let Some(s) = run_start {
                    draw_run(s, data.len() - 1, &mut frame);
                }
            }
        }

        let mut text_color: cosmic::iced::Color =
            theme.cosmic().background.component.on.into();
        text_color.a = 0.35;

        // Hauchfeine gepunktete Referenzlinie auf Kurven-Oberkante (= Skalen-
        // Maximum) direkt unter dem Text — macht die Koordinaten-Lesart
        // „Beschriftung gehört zur Oberkante" explizit. Nur bei autoskalierten
        // Graphen; bei fester 100-%-Skala wäre sie Rauschen.
        if self.caption_max.is_some() && has_data {
            let mut rule_color = text_color;
            rule_color.a = 0.25;
            dotted_rule(&mut frame, SPARK_TEXT_ZONE + SPARK_HW, w, rule_color);
        }

        // --- Hover: Crosshair + Marker + Werte in der Textzone ---
        // Die Textzone ist kurvenfrei reserviert; der Hover-Text ersetzt dort
        // temporär die Standard-Beschriftung (kein Hintergrund-Chip nötig).
        let mut hover_caption: Option<String> = None;
        if let (Some(p), Some(first), true) = (state, self.series.first(), has_data) {
            if first.len() >= 2 {
                let n = HISTORY_LEN.max(2) as f32;
                // Cursor-x → globaler Sample-Slot → Index in der Serie.
                let slot = (p.x / w * (n - 1.0)).round() as isize;
                let idx = slot - (HISTORY_LEN - first.len()) as isize;
                if (0..first.len() as isize).contains(&idx) {
                    let idx = idx as usize;
                    let x = Self::x_of(w, first.len(), idx);
                    // Vertikale Führungslinie über den Kurvenbereich.
                    let mut guide = text_color;
                    guide.a = 0.3;
                    frame.stroke(
                        &Path::line(
                            Point::new(x, SPARK_TEXT_ZONE + SPARK_HW),
                            Point::new(x, h - SPARK_HW),
                        ),
                        spark_stroke(guide).with_width(1.0),
                    );
                    // Marker auf der Erstserie.
                    frame.fill(
                        &Path::circle(Point::new(x, Self::y_of(h, max, first[idx])), 2.5),
                        accent,
                    );
                    // Wert(e) + Zeit-Offset: „↓ 2,1 M/s ↑ 300 K/s · −45 s".
                    let ago_s = (first.len() - 1 - idx) as u64 * self.span_s
                        / HISTORY_LEN.max(1) as u64;
                    let when = if ago_s == 0 {
                        "jetzt".to_string()
                    } else {
                        format!("−{}", fmt_span(ago_s))
                    };
                    let vals = match self.series.get(1).and_then(|s| s.get(idx)) {
                        Some(&up) => format!(
                            "↓ {} ↑ {}",
                            (self.format_value)(first[idx]),
                            (self.format_value)(up)
                        ),
                        None => (self.format_value)(first[idx]),
                    };
                    hover_caption = Some(format!("{vals} · {when}"));
                }
            }
        }

        // Minimal-Beschriftung oben rechts: beim Hovern Wert+Zeit (voll
        // deckend), sonst Skalen-Max (nur autoskaliert) + Zeitfenster,
        // winzig und stark gedimmt — informativ, nicht dominant.
        let (caption, cap_color) = match hover_caption {
            Some(hc) => {
                let mut c: cosmic::iced::Color =
                    theme.cosmic().background.component.on.into();
                c.a = 0.9;
                (hc, c)
            }
            None => (
                match (&self.caption_max, has_data) {
                    (Some(mx), true) => format!("≤ {mx} · {}", self.span_label),
                    _ => self.span_label.clone(),
                },
                text_color,
            ),
        };
        if !caption.is_empty() {
            frame.fill_text(widget::canvas::Text {
                content: caption,
                position: Point::new(w - 2.0, 0.0),
                color: cap_color,
                size: cosmic::iced::Pixels(9.0),
                align_x: cosmic::iced::alignment::Horizontal::Right.into(),
                align_y: cosmic::iced::alignment::Vertical::Top.into(),
                ..Default::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

/// Ringpuffer-Historie → Serie für eine Sparkline.
fn series_of(q: &VecDeque<f32>) -> Vec<f32> {
    q.iter().copied().collect()
}

/// Spitzenwert eines Werte-Iterators (0, wenn leer) — für autoskalierte Graphen.
fn peak_of<'a>(iter: impl Iterator<Item = &'a f32>) -> f32 {
    iter.fold(0.0f32, |a, &v| a.max(v))
}

/// Parameter einer Sparkline-Zeile — benannte Felder statt fünf
/// Positionsargumenten an den Callsites.
struct SparkSpec {
    series: Vec<Vec<f32>>,
    /// Normierungs-Maximum; `None` = gemeinsames Maximum der Serien (autoskaliert).
    fixed_max: Option<f32>,
    /// Fertig formatiertes Skalen-Maximum für die Beschriftung (nur autoskaliert).
    caption_max: Option<String>,
    /// Zeitfenster der Historie in Sekunden.
    span_s: u64,
    /// Formatiert einen Rohwert der Serie für die Hover-Anzeige.
    format_value: Box<dyn Fn(f32) -> String>,
}

/// Sparkline-Zeile unter einer Metrik (volle Breite, feste Höhe).
fn sparkline<'a>(spec: SparkSpec) -> Element<'a, Message> {
    widget::canvas(Sparkline {
        series: spec.series,
        fixed_max: spec.fixed_max,
        caption_max: spec.caption_max,
        span_label: fmt_span(spec.span_s),
        span_s: spec.span_s,
        format_value: spec.format_value,
    })
    .width(Length::Fill)
    .height(Length::Fixed(SPARK_HEIGHT))
    .into()
}

/// Zeitfenster kompakt: „45 s" unter einer Minute, sonst „3 min".
fn fmt_span(s: u64) -> String {
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min", s / 60)
    }
}

/// Uptime menschenlesbar: „3 d 4 h 12 min" (führende Null-Einheiten entfallen).
fn fmt_uptime(s: u64) -> String {
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
fn fmt_bytes(b: u64) -> String {
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
    fmt_rate_unit(bps, cfg.net_unit)
}

/// Wie `fmt_rate`, aber ohne Config-Borrow — für move-Closures (Sparkline-Hover).
fn fmt_rate_unit(bps: f64, net_unit: u8) -> String {
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

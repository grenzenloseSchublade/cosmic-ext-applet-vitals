// SPDX-License-Identifier: GPL-3.0-only

use crate::config::Config;
use crate::format::{
    fmt_bytes, fmt_mem, fmt_rate, fmt_rate_unit, fmt_uptime, gpu_inactive_text, mem_pct,
};
use crate::metric::{normalize_order, MetricKind};
use crate::metrics::{Collector, Metrics};
use crate::sparkline::{peak_of, series_of, sparkline, SparkCaches, SparkSpec, HISTORY_LEN};
use crate::widgets::{
    cycle_item, info_box, info_label, labeled_row, metric_or_bar, padded_heading,
    padded_heading_info, sub_toggle_item, temp_cell, toggle_item, RowCtx,
};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::{time, window::Id, Alignment, Length, Limits, Subscription};
use cosmic::prelude::*;
use cosmic::widget;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Symbolisches Panel-Icon (Prozessor-Chip), eingebettet → kein Theme-Install nötig,
/// wird vom COSMIC-Panel automatisch hell/dunkel eingefärbt.
const CHIP_SYMBOLIC: &[u8] = include_bytes!("../resources/icon-symbolic.svg");

/// Panel-Icon-Vergrößerung gegenüber der vom Panel vorgeschlagenen Größe (innerhalb der Zelltiefe).
const ICON_SCALE: f32 = 1.2;
/// Popup-Größenlimits (das Panel zwingt die Breite faktisch auf ~360 px).
const POPUP_MIN_WIDTH: f32 = 260.0;
const POPUP_MAX_WIDTH: f32 = 372.0;
const POPUP_MIN_HEIGHT: f32 = 120.0;
const POPUP_MAX_HEIGHT: f32 = 1080.0;

/// Abonniert das logind-Signal `PrepareForSleep` (System-Bus): pausiert die
/// Delta-Erfassung über die Schlafphase. Ohne logind (Container, andere Init-
/// Systeme) endet der Stream still — das Applet läuft ohne Suspend-Reset weiter.
fn logind_sleep_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            4,
            |mut tx: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
            use cosmic::iced::futures::{SinkExt, StreamExt};
            let Ok(conn) = zbus::Connection::system().await else {
                return;
            };
            let Ok(proxy) = zbus::Proxy::new(
                &conn,
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
            )
            .await
            else {
                return;
            };
            let Ok(mut signals) = proxy.receive_signal("PrepareForSleep").await else {
                return;
            };
            while let Some(msg) = signals.next().await {
                if let Ok(sleeping) = msg.body().deserialize::<bool>() {
                    let _ = tx.send(Message::PrepareForSleep(sleeping)).await;
                }
            }
            },
        )
    })
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
    /// Geometrie-Caches der Sparklines (Invalidierung bei neuen Daten).
    spark_caches: SparkCaches,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    /// Ergebnis einer Hintergrund-Erfassung.
    MetricsUpdated(Metrics),
    TogglePopup,
    PopupClosed(Id),
    /// logind `PrepareForSleep`: true = System schläft gleich, false = Aufwachen.
    PrepareForSleep(bool),
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
            spark_caches: SparkCaches::default(),
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
            logind_sleep_subscription(),
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::Tick => {
                // Erfassung im Hintergrund anstoßen; Live-GPU (NVML) nur bei offenem Popup.
                return self.spawn_refresh(self.popup.is_some());
            }
            Message::PrepareForSleep(sleeping) => {
                let mut c = self.collector.lock().unwrap_or_else(|e| e.into_inner());
                c.paused = sleeping;
                if !sleeping {
                    // Aufwachen: Delta-Zustände sofort verwerfen — zwischen den
                    // Signalen läuft nicht zwingend ein Tick mit `paused`.
                    c.reset_deltas();
                }
            }
            Message::MetricsUpdated(m) => {
                self.history.push(&m);
                self.spark_caches.clear();
                self.metrics = m;
                self.refreshing = false;
                // Verschluckten Live-Wunsch genau einmal nachholen.
                if std::mem::take(&mut self.pending_live) && self.popup.is_some() {
                    return self.spawn_refresh(true);
                }
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
        let rc = RowCtx {
            mono: c.mono_font,
            accent: c.accent_labels,
            parent: self.popup,
            kind,
        };
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
                    &rc,
                )];
                if c.show_graphs && c.graph_cpu {
                    rows.push(sparkline(SparkSpec {
                        series: vec![series_of(&self.history.cpu)],
                        cache: &self.spark_caches.cpu,
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
                    &rc,
                )];
                if c.show_graphs && c.graph_mem {
                    rows.push(sparkline(SparkSpec {
                        series: vec![series_of(&self.history.mem)],
                        cache: &self.spark_caches.mem,
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
                        "↓ {:>7} ↑ {:>7}{}",
                        fmt_rate(m.net_down_bps, c),
                        fmt_rate(m.net_up_bps, c),
                        kind
                    ),
                    &rc,
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
                        cache: &self.spark_caches.net,
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
                    (Some(t), _) => vec![labeled_row("GPU", t, &rc)],
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
                            &rc,
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
                    vec![labeled_row("Lüfter", format!("{fans} rpm"), &rc)]
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
                        labeled_row(label, cores, &RowCtx { mono: true, ..rc })
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
                    &rc,
                )];
                if c.show_graphs && c.graph_power {
                    let peak = peak_of(self.history.power.iter());
                    rows.push(sparkline(SparkSpec {
                        series: vec![series_of(&self.history.power)],
                        cache: &self.spark_caches.power,
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
                vec![labeled_row("Akku", parts.join(" · "), &rc)]
            }
            MetricKind::Disk => {
                // Gleiche Pfeil-Konvention wie Netz: ↓ Lesen, ↑ Schreiben.
                vec![labeled_row(
                    "Disk",
                    format!(
                        "↓ {:>7} ↑ {:>7}",
                        fmt_rate(m.disk_read_bps, c),
                        fmt_rate(m.disk_write_bps, c)
                    ),
                    &rc,
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
                    &rc,
                )]
            }
            MetricKind::Load => {
                let Some((l1, l5, l15)) = m.loadavg else {
                    return Vec::new();
                };
                vec![labeled_row(
                    "Load",
                    format!("{l1:.2}  {l5:.2}  {l15:.2}"),
                    &rc,
                )]
            }
            MetricKind::Uptime => {
                let Some(s) = m.uptime_s else {
                    return Vec::new();
                };
                vec![labeled_row(
                    "Uptime",
                    fmt_uptime(s),
                    &rc,
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
                    &rc,
                )]
            }
        }
    }

    /// Einstellungs-Ansicht — Sektionen: Metriken & Reihenfolge, Inhalt,
    /// Format, Darstellung, Panel, Aktualisierung; plus Zurücksetzen.
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
        let net_label = match c.net_unit {
            1 => "MiB/s (binär)",
            2 => "Mbit/s (Bit)",
            _ => "MB/s (SI)",
        };
        let format_section = widget::settings::section().header(padded_heading("Format"));
        let format_section = toggle_item(
            format_section,
            "Temperatur in °F",
            "Alle Temperaturen in Fahrenheit statt Celsius.",
            c.fahrenheit,
            Message::SetFahrenheit,
        );
        let format_section = cycle_item(
            format_section,
            "Netz-Einheit",
            "Einheit für Netzwerk-Durchsatz: MB/s (SI, 10⁶), MiB/s (binär, 2²⁰) oder Mbit/s. Klick wechselt.",
            net_label.to_string(),
            Message::CycleNetUnit,
            false,
        );
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
        let panel_metric_label = MetricKind::from_u8(c.panel_metric)
            .unwrap_or(MetricKind::Cpu)
            .label();
        let panel_section = widget::settings::section().header(padded_heading("Panel"));
        let panel_section = toggle_item(
            panel_section,
            "Wert neben dem Panel-Icon",
            "Zeigt den gewählten Messwert als Text direkt im Panel (nur horizontale Leiste).",
            c.panel_text,
            Message::SetPanelText,
        );
        let panel_section = cycle_item(
            panel_section,
            "Panel-Wert",
            "Welche Metrik neben dem Panel-Icon steht (CPU, RAM, Netz, GPU oder Watt). Klick wechselt.",
            panel_metric_label.to_string(),
            Message::CyclePanelMetric,
            true,
        );

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

// SPDX-License-Identifier: GPL-3.0-only

//! UI-Helfer des Popups: Metrik-Zeilen, Info-Boxen/-Tooltips und
//! Settings-Zeilen-Bausteine.

use crate::app::Message;
use crate::config::Config;
use crate::format::fmt_temp_val;
use crate::metric::MetricKind;
use cosmic::cctk::sctk::reexports::protocols::xdg::shell::client::xdg_positioner::{
    Anchor, Gravity,
};
use cosmic::iced::{window::Id, Alignment, Length, Limits, Rectangle};
use cosmic::prelude::*;
use cosmic::widget;
use iced_runtime::platform_specific::wayland::popup::{SctkPopupSettings, SctkPositioner};
use std::sync::LazyLock;
use std::time::Duration;

/// Fenster-Id des (einen) Wayland-Metrik-Tooltips — als xdg_popup darf er über
/// den Rand des Metrik-Popups hinausragen (ein iced-Tooltip kann das nicht).
static METRIC_TIP_WINDOW: LazyLock<Id> = LazyLock::new(Id::unique);
/// Autosize-Id für den Tooltip-Inhalt.
static METRIC_TIP_AUTOSIZE: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("metric-tooltip"));

/// Feste Breite der fetten Metrik-Beschriftung (linke Spalte im Popup);
/// mit etwas Puffer für erhöhte COSMIC-Textskalierung.
const LABEL_WIDTH: f32 = 64.0;
/// Feste Breiten der Mittel-/Temperatur-Spalte in `triple_row` — rechtsbündig
/// verankert, damit die Werte zeilenübergreifend fluchten (statt der früher
/// frei flottierenden Fill-Spacer-Mitte).
const MID_COL_WIDTH: f32 = 110.0;
const TEMP_COL_WIDTH: f32 = 56.0;
/// Warn-/Kritisch-Farben (Temperatur-Schwellen, Balken-Auslastung).
const WARN_COLOR: cosmic::iced::Color = cosmic::iced::Color::from_rgb(0.95, 0.65, 0.15);
const CRIT_COLOR: cosmic::iced::Color = cosmic::iced::Color::from_rgb(0.90, 0.22, 0.22);
/// Höhe/Dicke der Auslastungsbalken.
const BAR_GIRTH: f32 = 8.0;

/// Kontext einer Metrik-Zeile — bündelt die früher einzeln durchgereichten
/// Parameter (Monospace, Akzentfarbe, Popup-Fenster fürs Wayland-Tooltip)
/// plus die Metrik selbst (typsicherer Schlüssel für den Erklärtext).
#[derive(Clone, Copy)]
pub(crate) struct RowCtx {
    pub(crate) mono: bool,
    pub(crate) accent: bool,
    pub(crate) parent: Option<Id>,
    pub(crate) kind: MetricKind,
}

/// Fette Metrik-Beschriftung in fester Spaltenbreite (`LABEL_WIDTH`);
/// optional in der System-Akzentfarbe. Nur das Wort ist hoverbar; die
/// Info-Box öffnet als Wayland-Popup links über den Fensterrand hinaus,
/// damit sie die Werte nicht überdeckt. Leeres Label (Kerne-Folgezeilen)
/// bekommt kein Tooltip.
fn bold_label<'a>(label: &'static str, rc: &RowCtx) -> Element<'a, Message> {
    let mut t = widget::text(label).font(cosmic::font::bold());
    if rc.accent {
        t = t.class(cosmic::theme::Text::Accent);
    }
    let word: Element<'a, Message> = match rc.parent {
        Some(parent) if !label.is_empty() => {
            metric_tooltip(t, rc.kind.info_detail(), parent)
        }
        _ => t.into(),
    };
    // Feste Spaltenbreite außen — der Tooltip bleibt aufs Wort begrenzt.
    widget::container(word)
        .width(Length::Fixed(LABEL_WIDTH))
        .into()
}

/// Einzeilige Metrik-Zeile: fettes Label + Wert (optional Monospace für bündige Ziffern).
pub(crate) fn labeled_row<'a>(
    label: &'static str,
    value: String,
    rc: &RowCtx,
) -> Element<'a, Message> {
    let val = widget::text(value);
    let val = if rc.mono {
        val.font(cosmic::iced::Font::MONOSPACE)
    } else {
        val
    };
    widget::row::with_children(vec![bold_label(label, rc), val.into()])
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
/// **mittig** Detail (RAM-GiB / GPU-VRAM), **rechts** Temp (als fertiges Element,
/// damit es eingefärbt werden kann). Mittel- und Temp-Spalte haben feste Breite
/// und sind rechtsbündig verankert → die Werte fluchten zeilenübergreifend.
fn triple_row<'a>(
    label: &'static str,
    left: Element<'a, Message>,
    mid: String,
    right: Element<'a, Message>,
    rc: &RowCtx,
) -> Element<'a, Message> {
    widget::row::with_children(vec![
        bold_label(label, rc),
        left,
        widget::space::horizontal().into(),
        widget::container(value_text(mid, rc.mono))
            .width(Length::Fixed(MID_COL_WIDTH))
            .align_x(Alignment::End)
            .into(),
        widget::container(right)
            .width(Length::Fixed(TEMP_COL_WIDTH))
            .align_x(Alignment::End)
            .into(),
    ])
    .spacing(cosmic::theme::spacing().space_xs)
    .align_y(Alignment::Center)
    .into()
}

/// Temperatur-Zelle: formatiert + nach Schwellen eingefärbt (`warn_temp_c`/`crit_temp_c`).
/// `None` → leere Zelle (kein Sensor).
pub(crate) fn temp_cell<'a>(celsius: Option<f32>, cfg: &Config, mono: bool) -> Element<'a, Message> {
    let Some(c) = celsius else {
        return value_text(String::new(), mono);
    };
    let mut t = widget::text(fmt_temp_val(c, cfg));
    if mono {
        t = t.font(cosmic::iced::Font::MONOSPACE);
    }
    if c >= cfg.crit_temp_c as f32 {
        t = t.class(cosmic::theme::Text::Color(CRIT_COLOR));
    } else if c >= cfg.warn_temp_c as f32 {
        t = t.class(cosmic::theme::Text::Color(WARN_COLOR));
    }
    t.into()
}

/// CPU/RAM/GPU als Drei-Spalten-Zeile; bei `graphical` zusätzlich ein Balken (volle Breite) darunter.
/// Auslastungs-Zelle: der Prozentwert färbt sich an den Schwellen wie die
/// Temperaturen (≥ 90 % kritisch, ≥ 75 % Warnung) — der Theme-Balken selbst
/// ist per libcosmic-API nicht einfärbbar (StyleSheet fest verdrahtet).
fn usage_cell<'a>(frac: f32, s: String, mono: bool) -> Element<'a, Message> {
    let mut t = widget::text(s);
    if mono {
        t = t.font(cosmic::iced::Font::MONOSPACE);
    }
    if frac >= 0.9 {
        t = t.class(cosmic::theme::Text::Color(CRIT_COLOR));
    } else if frac >= 0.75 {
        t = t.class(cosmic::theme::Text::Color(WARN_COLOR));
    }
    t.into()
}

pub(crate) fn metric_or_bar<'a>(
    graphical: bool,
    label: &'static str,
    frac: f32,
    left: String,
    mid: String,
    right: Element<'a, Message>,
    rc: &RowCtx,
) -> Element<'a, Message> {
    let head = triple_row(label, usage_cell(frac, left, rc.mono), mid, right, rc);
    if graphical {
        // Schwellen-Marker bei 75 %/90 % als visuelle Warnlinien.
        let bar = widget::determinate_linear(frac.clamp(0.0, 1.0))
            .markers(vec![0.75, 0.9])
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
pub(crate) fn info_box<'a>(
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
pub(crate) fn info_label<'a>(title: &'static str, info: &'static str) -> Element<'a, Message> {
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
pub(crate) fn padded_heading<'a>(title: &'static str) -> Element<'a, Message> {
    widget::container(widget::text::heading(title))
        .padding([0, cosmic::theme::spacing().space_m])
        .into()
}

/// Wie `padded_heading`, zusätzlich mit Info-Icon hinter dem Titel.
pub(crate) fn padded_heading_info<'a>(
    title: &'static str,
    info: &'static str,
) -> Element<'a, Message> {
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
pub(crate) fn toggle_item<'a>(
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

/// Zyklus-Zeile: Label + Info links, rechts ein Button, der den aktuellen
/// Wert zeigt und beim Klick weiterschaltet („⟳"-Konvention).
pub(crate) fn cycle_item<'a>(
    section: widget::settings::Section<'a, Message>,
    title: &'static str,
    info: &'static str,
    current: String,
    msg: Message,
    sub: bool,
) -> widget::settings::Section<'a, Message> {
    let label = info_label(title, info);
    section.add(widget::settings::item_row(vec![
        if sub { indented(label) } else { label },
        widget::button::text(format!("{current}  ⟳"))
            .on_press(msg)
            .into(),
    ]))
}

/// Wie `toggle_item`, aber als eingerückter Unterpunkt (z. B. die
/// Pro-Metrik-Schalter unter dem Verlaufs-Master).
pub(crate) fn sub_toggle_item<'a>(
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

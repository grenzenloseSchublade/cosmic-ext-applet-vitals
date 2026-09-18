// SPDX-License-Identifier: GPL-3.0-only

//! Sparkline-Verlaufsgraphen (Canvas) samt Geometrie-Caches und Zeichen-Helfern.

use crate::app::Message;
use crate::format::fmt_span;
use cosmic::iced::{Length, Rectangle};
use cosmic::prelude::*;
use cosmic::widget;
use std::collections::VecDeque;

/// Samples je Verlaufs-Graph (bei 1,5-s-Intervall ≈ 3 Minuten Historie).
pub(crate) const HISTORY_LEN: usize = 120;
/// Höhe der Sparklines (Textzone + Kurvenbereich).
const SPARK_HEIGHT: f32 = 34.0;
/// Oberer Bereich der Sparkline, reserviert für die Beschriftung — die Kurve
/// zeichnet nur darunter, kann den Text also nie überdecken.
const SPARK_TEXT_ZONE: f32 = 11.0;

/// Geometrie-Caches der vier Sparklines: die statische Kurven-Geometrie wird
/// nur bei neuen Daten (Tick) neu tesselliert, nicht bei jedem Hover-Frame.
#[derive(Default)]
pub(crate) struct SparkCaches {
    pub(crate) cpu: widget::canvas::Cache,
    pub(crate) mem: widget::canvas::Cache,
    pub(crate) net: widget::canvas::Cache,
    pub(crate) power: widget::canvas::Cache,
}

impl SparkCaches {
    pub(crate) fn clear(&self) {
        self.cpu.clear();
        self.mem.clear();
        self.net.clear();
        self.power.clear();
    }
}

/// Sparkline-Verlauf als Canvas: Linie + zart gefüllte Fläche in der
/// System-Akzentfarbe; zweite Serie (Netz ↑) gedimmt. Neueste Werte rechts,
/// die x-Achse ist auf `HISTORY_LEN` fixiert — der Graph „läuft" von rechts ein.
struct Sparkline<'c> {
    series: Vec<Vec<f32>>,
    /// Persistenter Geometrie-Cache (lebt im AppModel): die Kurven-Tessellation
    /// läuft nur bei neuen Daten, nicht bei jedem Hover-Frame.
    cache: &'c widget::canvas::Cache,
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

impl Sparkline<'_> {
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

impl<Message> widget::canvas::Program<Message, cosmic::Theme> for Sparkline<'_> {
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

        let mut text_color: cosmic::iced::Color =
            theme.cosmic().background.component.on.into();
        text_color.a = 0.35;

        // Statische Geometrie (Kurven, Fläche, Referenzlinie) aus dem Cache —
        // neu tesselliert nur nach Daten-Update (SparkCaches::clear), nicht
        // bei Hover-Redraws.
        let static_geom = self.cache.draw(renderer, bounds.size(), |frame| {
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
                        draw_run(s, i - 1, frame);
                    }
                }
                if let Some(s) = run_start {
                    draw_run(s, data.len() - 1, frame);
                }
            }
        }

        // Hauchfeine gepunktete Referenzlinie auf Kurven-Oberkante (= Skalen-
        // Maximum) direkt unter dem Text. Nur bei autoskalierten Graphen;
        // bei fester 100-%-Skala wäre sie Rauschen.
        if self.caption_max.is_some() && has_data {
            let mut rule_color = text_color;
            rule_color.a = 0.25;
            dotted_rule(frame, SPARK_TEXT_ZONE + SPARK_HW, w, rule_color);
        }
        });

        // --- Dynamische Ebene: Hover-Overlay + Beschriftung (billig) ---
        let mut frame = Frame::new(renderer, bounds.size());

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
        vec![static_geom, frame.into_geometry()]
    }
}

/// Ringpuffer-Historie → Serie für eine Sparkline.
pub(crate) fn series_of(q: &VecDeque<f32>) -> Vec<f32> {
    q.iter().copied().collect()
}

/// Spitzenwert eines Werte-Iterators (0, wenn leer) — für autoskalierte Graphen.
pub(crate) fn peak_of<'a>(iter: impl Iterator<Item = &'a f32>) -> f32 {
    iter.fold(0.0f32, |a, &v| a.max(v))
}

/// Parameter einer Sparkline-Zeile — benannte Felder statt fünf
/// Positionsargumenten an den Callsites.
pub(crate) struct SparkSpec<'c> {
    pub(crate) series: Vec<Vec<f32>>,
    /// Persistenter Geometrie-Cache aus dem AppModel.
    pub(crate) cache: &'c widget::canvas::Cache,
    /// Normierungs-Maximum; `None` = gemeinsames Maximum der Serien (autoskaliert).
    pub(crate) fixed_max: Option<f32>,
    /// Fertig formatiertes Skalen-Maximum für die Beschriftung (nur autoskaliert).
    pub(crate) caption_max: Option<String>,
    /// Zeitfenster der Historie in Sekunden.
    pub(crate) span_s: u64,
    /// Formatiert einen Rohwert der Serie für die Hover-Anzeige.
    pub(crate) format_value: Box<dyn Fn(f32) -> String>,
}

/// Sparkline-Zeile unter einer Metrik (volle Breite, feste Höhe).
pub(crate) fn sparkline<'a>(spec: SparkSpec<'a>) -> Element<'a, Message> {
    widget::canvas(Sparkline {
        series: spec.series,
        cache: spec.cache,
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

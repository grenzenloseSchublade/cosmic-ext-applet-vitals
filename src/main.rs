// SPDX-License-Identifier: GPL-3.0-only

mod app;
mod config;
mod hw;
mod metrics;

fn main() -> cosmic::iced::Result {
    // Startet die Event-Loop des Applets mit `()` als Flags.
    cosmic::applet::run::<app::AppModel>(())
}

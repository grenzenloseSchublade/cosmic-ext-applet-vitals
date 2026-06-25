#!/usr/bin/env bash
# User-lokale Installation (kein sudo) nach ~/.local.
set -euo pipefail
cd "$(dirname "$0")"

NAME=cosmic-ext-applet-vitals
APPID=io.github.grenzenloseschublade.CosmicAppletVitals

echo "==> cargo build --release"
cargo build --release

BIN="target/release/$NAME"
install -Dm0755 "$BIN"                        "$HOME/.local/bin/$NAME"
install -Dm0644 resources/app.desktop         "$HOME/.local/share/applications/$APPID.desktop"
install -Dm0644 resources/app.metainfo.xml    "$HOME/.local/share/appdata/$APPID.metainfo.xml"
install -Dm0644 resources/icon.svg            "$HOME/.local/share/icons/hicolor/scalable/apps/$APPID.svg"

echo
echo "Fertig. Falls ~/.local/bin nicht im PATH ist, einmal ab-/anmelden."
echo "In COSMIC: Einstellungen → Leiste (oder Dock) → Applets → 'Vitals' hinzufügen."

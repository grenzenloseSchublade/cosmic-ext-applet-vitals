name := 'cosmic-ext-applet-vitals'
appid := 'io.github.grenzenloseschublade.CosmicAppletVitals'

rootdir := ''
prefix := '/usr'
userdir := env('HOME') / '.local'

base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
bin-dst := base-dir / 'bin' / name
desktop-dst := base-dir / 'share' / 'applications' / appid + '.desktop'
metainfo-dst := base-dir / 'share' / 'metainfo' / appid + '.metainfo.xml'
icon-dst := base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg'

# Standard: Release-Build
default: build-release

clean:
    cargo clean

build-debug *args:
    cargo build {{args}}

build-release *args: (build-debug '--release' args)

# clippy
check *args:
    cargo clippy --all-features {{args}} -- -W clippy::pedantic

# Zum Testen ausführen
run *args:
    env RUST_BACKTRACE=full cargo run --release {{args}}

# System-Installation (prefix=/usr, braucht sudo)
install:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{bin-dst}}
    install -Dm0644 resources/app.desktop {{desktop-dst}}
    install -Dm0644 resources/app.metainfo.xml {{metainfo-dst}}
    install -Dm0644 resources/icon.svg {{icon-dst}}

# User-Installation nach ~/.local (kein sudo) — Standard für dich
install-user: build-release
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{ userdir / 'bin' / name }}
    install -Dm0644 resources/app.desktop {{ userdir / 'share/applications' / appid + '.desktop' }}
    install -Dm0644 resources/app.metainfo.xml {{ userdir / 'share/metainfo' / appid + '.metainfo.xml' }}
    install -Dm0644 resources/icon.svg {{ userdir / 'share/icons/hicolor/scalable/apps' / appid + '.svg' }}
    @echo "Installiert nach ~/.local. In COSMIC: Einstellungen → Leiste/Dock → Applets → Vitals."

uninstall:
    rm -f {{bin-dst}} {{desktop-dst}} {{icon-dst}} {{metainfo-dst}}
    rm -f {{ base-dir / 'share/appdata' / appid + '.metainfo.xml' }}  # Altpfad früherer Versionen

uninstall-user:
    rm -f {{ userdir / 'bin' / name }} \
          {{ userdir / 'share/applications' / appid + '.desktop' }} \
          {{ userdir / 'share/metainfo' / appid + '.metainfo.xml' }} \
          {{ userdir / 'share/appdata' / appid + '.metainfo.xml' }} \
          {{ userdir / 'share/icons/hicolor/scalable/apps' / appid + '.svg' }}

# --- RAPL-Freigabe (opt-in, siehe README "Leistungsmessung (RAPL)") ---
# Macht die RAPL-Energiezähler für die Gruppe "rapl" lesbar (CVE-2020-8694-Abwägung!).
rapl-rule := '90-cosmic-vitals-rapl.rules'

install-rapl-rule:
    sudo groupadd -f rapl
    sudo usermod -aG rapl {{ env('USER') }}
    sudo install -Dm0644 {{ 'resources' / rapl-rule }} {{ '/etc/udev/rules.d' / rapl-rule }}
    sudo udevadm control --reload
    sudo udevadm trigger -s powercap
    @echo "Fertig. WICHTIG: einmal ab-/anmelden (Gruppenmitgliedschaft), danach Applet neu starten."

uninstall-rapl-rule:
    sudo rm -f {{ '/etc/udev/rules.d' / rapl-rule }}
    sudo udevadm control --reload
    @echo "Regel entfernt. Rechte gelten bis zum Reboot weiter; Gruppe 'rapl' bleibt bestehen."

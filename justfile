name := 'cosmic-ext-applet-vitals'
appid := 'io.github.grenzenloseschublade.CosmicAppletVitals'

rootdir := ''
prefix := '/usr'
userdir := env('HOME') / '.local'

base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
bin-dst := base-dir / 'bin' / name
desktop-dst := base-dir / 'share' / 'applications' / appid + '.desktop'
appdata-dst := base-dir / 'share' / 'appdata' / appid + '.metainfo.xml'
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
    install -Dm0644 resources/app.metainfo.xml {{appdata-dst}}
    install -Dm0644 resources/icon.svg {{icon-dst}}

# User-Installation nach ~/.local (kein sudo) — Standard für dich
install-user: build-release
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{ userdir / 'bin' / name }}
    install -Dm0644 resources/app.desktop {{ userdir / 'share/applications' / appid + '.desktop' }}
    install -Dm0644 resources/app.metainfo.xml {{ userdir / 'share/appdata' / appid + '.metainfo.xml' }}
    install -Dm0644 resources/icon.svg {{ userdir / 'share/icons/hicolor/scalable/apps' / appid + '.svg' }}
    @echo "Installiert nach ~/.local. In COSMIC: Einstellungen → Leiste/Dock → Applets → Vitals."

uninstall:
    rm -f {{bin-dst}} {{desktop-dst}} {{icon-dst}} {{appdata-dst}}

uninstall-user:
    rm -f {{ userdir / 'bin' / name }} \
          {{ userdir / 'share/applications' / appid + '.desktop' }} \
          {{ userdir / 'share/appdata' / appid + '.metainfo.xml' }} \
          {{ userdir / 'share/icons/hicolor/scalable/apps' / appid + '.svg' }}

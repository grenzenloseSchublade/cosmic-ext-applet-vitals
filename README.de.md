# Vitals — COSMIC System-Monitor-Applet

*🌐 Sprache: **Deutsch** · [English](README.md)*

Ein schlankes, **robustes** System-Monitor-Applet für die [COSMIC](https://system76.com/cosmic)-Leiste (Panel) oder das Dock. Zeigt **CPU-/RAM-Auslastung, Temperaturen, Netzwerk-Durchsatz** und den **NVIDIA-GPU-Status** — mit Detail-Popup beim Anklicken.

## Design-Philosophie: *only read, never blocking*

Das Leitprinzip ist **„only read, never blocking"** — das Applet **liest** Systemzustände nur und tut dabei **nie etwas, das blockieren oder Hardware-Zustände verändern kann**:

- **Only read:** Werte kommen ausschließlich aus `/proc`, `/sys` und in-process via NVML — **kein** Subprozess, **kein** Schreibzugriff, **kein** Wecken schlafender Geräte.
- **Never blocking:** keine Aufrufe, die im Treiber/Suspend hängen können. Eine schlafende NVIDIA-dGPU (RTD3) wird **nie** geweckt (NVML nur bei `runtime_status == active`); fehlende Sensoren werden ausgeblendet statt zu blockieren oder zu paniken.

So bleibt das Applet leichtgewichtig und ist die direkte Antwort auf den Suspend-Hänger üblicher Monitore (siehe unten).

## Warum dieses Applet?

> **Was Vitals besonders macht:** soweit bekannt das **erste/einzige COSMIC-System-Monitor-Applet, das eine NVIDIA-Hybrid-dGPU schlafen lässt**. Andere Monitore starten entweder `nvidia-smi` (blockiert den Suspend) oder halten ein dauerhaftes NVML-Handle, das die **dGPU wachhält** (Akkuverbrauch, kein RTD3). Vitals weckt/pinnt sie nie — siehe *only read, never blocking* oben.

Übliche System-Monitor-Applets starten zum Auslesen der GPU bei **jedem Tick** `nvidia-smi` als Subprozess. Beim Suspend bleibt so ein Aufruf im sich abbauenden NVIDIA-Treiber hängen und blockiert das Einschlafen ~90 s lang (systemd-Timeout). Vitals vermeidet das **konstruktionsbedingt**:

- **Kein einziger Subprozess** — alle Werte kommen direkt aus `/proc` und `/sys`.
- **GPU batterieschonend & suspend-fest:** die NVIDIA-dGPU wird nur über **NVML in-process** abgefragt — und das **nur**, wenn sie laut sysfs (`runtime_status == active`) ohnehin schon wach ist **und** das Detail-Popup geöffnet ist. Sonst wird das NVML-Handle **freigegeben** (`/dev/nvidia0` geschlossen), damit die dGPU per **RTD3** einschlafen kann. So **pinnt** das Applet die dGPU nicht und **weckt** sie nie selbst. Den Zustand (schläft/aktiv) + Modus liest es pin-frei aus sysfs; Live-Werte (Auslastung/VRAM/Temp) gibt es nur, wenn die GPU gerade arbeitet und man hinschaut.
- **Panik-frei:** fehlende Sensoren (z. B. geblacklistetes `spd5118` für RAM-Temp) werden einfach ausgeblendet.
- **Unabhängig von `nvidia-powerd`** — läuft mit Dynamic Boost an oder aus.

## Anzeige

- **Panel:** symbolisches Chip-Icon, optional mit kompaktem Wert daneben (z. B. `CPU 12%` oder `↓1.2M/s ↑0.1M/s`, nur horizontale Leiste). Die Applet-Fläche **wächst dynamisch** mit dem Text (via `core.applet.autosize_window`).
- **Popup (Details):** CPU (gesamt + Temp, optional pro Kern), RAM (genutzt/gesamt + Temp), Netz (↓/↑ + Typ **WLAN/LAN/VPN**), GPU (bei aktiver dGPU Last + VRAM + Temp; sonst Zustand/Modus), Lüfterdrehzahlen, **Leistungsaufnahme** (System gesamt / CPU-Paket / GPU in Watt — siehe *Leistungsmessung (RAPL)* unten) sowie optional eine **Akku-Zeile** (Spannung, Ladezustand, Lade-/Entladeleistung). Weitere Opt-in-Metriken (standardmäßig aus): **Disk-I/O** (Lese-/Schreibrate über physische Laufwerke), **Swap**, **Load Average** (1/5/15 min), **Uptime** und **kumuliertes Netz-Gesamtvolumen** seit dem Booten. Optional als **Auslastungsbalken** (zweizeilig, volle Breite) für CPU/RAM/GPU.
- **Verlaufs-Graphen (Sparklines):** ein ~3-minütiger Mini-Graph unter CPU, RAM, Netz (↓ voll, ↑ gedimmt) und Watt, gezeichnet in der System-Akzentfarbe; die Historie läuft auch bei geschlossenem Popup weiter. Hovern zeigt Wert und Zeitversatz an der Cursor-Position; autoskalierte Graphen tragen eine gepunktete Max-Referenzlinie und eine `≤ max · span`-Beschriftung. Master-Schalter plus Einzelschalter je Metrik.
- **Info-Tooltips überall:** jedes Einstellungs-Label hat ein ⓘ-Icon mit Erklärung; die fetten Metrik-Wörter in der Hauptansicht zeigen beim Hovern eine strukturierte Info-Box (ein echtes Wayland-Popup, das über den Fensterrand hinausragen darf und so die Werte nie verdeckt).

## Datenquellen

| Wert | Quelle |
|---|---|
| CPU-Last | `/proc/stat` (Delta) |
| CPU-Temp | hwmon `coretemp` „Package id 0" (Fallback `k10temp`/`acpitz`) |
| RAM | `/proc/meminfo` |
| RAM-Temp | hwmon `spd5118` (optional) |
| Lüfter | hwmon `thinkpad` `fan*_input` |
| Netz ↑/↓ | `/sys/class/net/<if>/statistics/{rx,tx}_bytes` (Default-Route-Interface) |
| Netz-Typ | WLAN (`…/<if>/wireless`), VPN (`tun`/`tap`/`wg`/`ppp`), sonst LAN |
| dGPU-Zustand | `/sys/bus/pci/devices/<nvidia>/power/runtime_status` |
| NVIDIA-Modus | `/etc/prime-discrete` |
| GPU-Last/Temp/VRAM/Watt | NVML (`libnvidia-ml`, `memory_info()`, `power_usage()`) — nur wenn dGPU aktiv |
| System-/CPU-Paket-Leistung | RAPL `/sys/class/powercap` (`psys` / `package-0`, Zählerdelta) — braucht die Opt-in-udev-Regel, siehe unten |
| Akku-Spannung/-Leistung/-Status | `/sys/class/power_supply/BAT*/{voltage_now,power_now,status}` |
| Disk-I/O | `/proc/diskstats` (512-Byte-Sektoren, Delta; Partitionen und gestapelte Devices wie `dm-`/`md`/`zram` ausgeschlossen, um Doppelzählung zu vermeiden) |
| Swap | `/proc/meminfo` (`SwapTotal`/`SwapFree`) |
| Load Average | `/proc/loadavg` |
| Uptime | `/proc/uptime` |
| Netz gesamt (Σ) | kumulierte `rx_bytes`/`tx_bytes` — dieselben Zähler wie für die Rate |

## Hardware-Anpassung

Alle hardware-/board-spezifischen Pfade und Sensor-Bezeichner liegen **an einer Stelle**: [`src/hw.rs`](src/hw.rs). Für andere Hardware nur dort anpassen — kein Suchen im restlichen Code:

- **CPU-Temp:** `CPU_TEMP_LABELED` (Chip + `temp*_label`), Intel `coretemp`/„Package id 0" und AMD `k10temp`/„Tctl" sind dabei; Fallback `acpitz`.
- **RAM-Temp:** `RAM_TEMP_CHIP` (Default `spd5118`).
- **Lüfter:** `FAN_CHIP` (Default `thinkpad`) + `FAN_MAX_INDEX` — z. B. `dell_smm` oder `nct6…` eintragen.
- **GPU:** `PRIME_DISCRETE_PATH`, `NVIDIA_VENDOR_ID`, `DISPLAY_CLASS_PREFIX`, `RUNTIME_STATUS_REL`.
- **Netz:** `VPN_IFACE_PREFIXES`.

Eigene hwmon-Chipnamen herausfinden:

```sh
for f in /sys/class/hwmon/hwmon*/name; do echo "$f -> $(cat "$f")"; done
grep . /sys/class/hwmon/hwmon*/temp*_label
```

## Leistungsmessung (RAPL)

Die Watt-Zeile zeigt bis zu drei Werte: **System gesamt**, **CPU-Paket** (inkl. iGPU) und **GPU**. System und CPU stammen aus den Intel/AMD-**RAPL**-Energiezählern unter `/sys/class/powercap` (`psys` und `package-0`, berechnet als Zählerdelta pro Tick, Überlauf wird behandelt).

**Rechte:** Seit Kernel 5.10 ist `energy_uj` nur noch für **root** lesbar — die Mitigation für [CVE-2020-8694](https://nvd.nist.gov/vuln/detail/CVE-2020-8694) („PLATYPUS", ein Power-Seitenkanal, über den auf Mehrbenutzer-Systemen Geheimnisse anderer Nutzer leaken können). Vitals liefert deshalb eine **Opt-in**-udev-Regel mit, die die Zähler nur für Mitglieder einer eigenen Gruppe `rapl` freigibt (Modus 0440 — bewusst **nicht** world-readable):

```sh
just install-rapl-rule    # legt Gruppe "rapl" an, fügt dich hinzu, installiert die udev-Regel
# danach: einmal ab-/anmelden (Gruppenmitgliedschaft), Applet neu starten
```

**Abwägung:** Auf einem Einzelnutzer-Laptop ist das Risiko gering; auf geteilten/Mehrbenutzer-Maschinen die Regel besser **nicht** installieren. Entfernen: `just uninstall-rapl-rule`.

**Ohne die Regel** degradiert das Applet sauber: Im Akkubetrieb fällt der Gesamtwert auf die Entladeleistung des Akkus zurück (`power_now`); am Netz wird bewusst kein Gesamtwert gezeigt („– · Netz"), weil der Akku dann nur die Laderate misst. Der GPU-Wert ist RAPL-unabhängig (NVML, nur bei wacher dGPU und offenem Popup — die dGPU wird weiterhin **nie** geweckt). Beim Laden schätzt die Akku-Zeile zusätzlich den **Zug am Netzteil** (`psys` + Ladeleistung; Wandlerverluste nicht enthalten). Eine echte Messung des USB-C-Netzteil-Ausgangs ist nicht möglich — der PD-Controller (UCSI) meldet nur den ausgehandelten Vertrag, keine Live-Werte. Die echte CPU-Kernspannung (Vcore) ist auf modernen Laptops ohne root-/MSR-Zugriff nicht verfügbar; angezeigt wird nur die Akku-Spannung.

## Datenschutz — kein Heimtelefonieren

Vitals **telefoniert nicht nach Hause**: keine Netzwerkverbindung, keine Telemetrie/Analytics, kein Auto-Update-Callback, kein Tracking. Es liest **ausschließlich lokal** aus `/proc`, `/sys` und der NVIDIA-Bibliothek **in-process** (kein Subprozess) und **schreibt/sendet nichts** nach außen — die einzige Persistenz ist die lokale Einstellungsdatei. Das ist die direkte Folge der Design-Philosophie **„only read, never blocking"** und lässt sich am schlanken Quellcode nachvollziehen.

## Bauen

Voraussetzung: Rust (`cargo`), Wayland-/xkb-Dev-Pakete (für libcosmic).

```sh
cargo build --release         # oder: just build-release
```

## Installieren

**A) Fertiges `.deb` (amd64) — am einfachsten:**

Das `.deb` vom [aktuellen Release](https://github.com/grenzenloseSchublade/cosmic-ext-applet-vitals/releases/latest) herunterladen und installieren:

```sh
sudo apt install ./cosmic-ext-applet-vitals_1.0.0_amd64.deb
```

Das vorgebaute Binary zielt auf **amd64** mit aktueller glibc (Klasse Pop!_OS / Ubuntu 24.04+ / Debian 13+). libcosmic ist statisch gelinkt; dynamisch werden nur Systembibliotheken benötigt (`libc6`, `libgcc-s1`, `libxkbcommon0`). Es ist **architektur**-spezifisch, nicht maschinenspezifisch — hwmon-Chips werden zur Laufzeit erkannt, mit sauberem Fallback.

**B) Aus dem Quellcode — benutzer-lokal (ohne sudo, empfohlen für den Eigengebrauch):**

```sh
./install.sh                  # oder: just install-user
```

Installiert Binary nach `~/.local/bin`, `.desktop` nach `~/.local/share/applications`. System-weit: `sudo just install` (prefix=/usr).

**Neuen Build ausrollen (Applet neu starten):** Installieren ersetzt nur die Datei auf der Platte — das laufende Applet führt das alte (gelöschte) Binary weiter aus, bis es neu gestartet wird. `cosmic-panel` respawnt ein gekilltes Applet **nicht** von allein; stattdessen das Panel neu starten (cosmic-session startet es samt aller Applets neu):

```sh
just install-user
kill $(pgrep -x cosmic-panel)     # Session startet Panel + Applets automatisch neu
# prüfen, dass das neue Binary läuft (kein "(deleted)"-Suffix):
for p in $(pgrep -f cosmic-ext-applet-vitals); do readlink /proc/$p/exe; done
```

Hinweis: `pkill -f cosmic-ext-applet-vitals` ist in Skripten eine Falle — das Muster trifft auch die Shell, die den Befehl ausführt, und killt so das eigene Skript. Stattdessen wie oben über `/proc/<pid>/exe` filtern.

**C) Eigenes `.deb` bauen:**

```sh
sudo apt install debhelper pkg-config libxkbcommon-dev libwayland-dev   # einmalig
dpkg-buildpackage -b -us -uc                                            # erzeugt ../*.deb
sudo apt install ../cosmic-ext-applet-vitals_1.0.0_*.deb
```

Voraussetzung ist eine aktuelle Rust-Toolchain (`cargo`/`rustc`); der Build holt die Crates aus dem Netz. Das Packaging liegt unter [`debian/`](debian/) (natives Format).

**D) PPA:** *geplant* — eine Launchpad-PPA für bequeme `apt`-Updates ist für die Zukunft vorgesehen, aber noch nicht eingerichtet.

## Distribution — und warum kein Flatpak

**Bezugswege:** Quellcode auf **GitHub**; native **`.deb`**-Pakete (Build via [`debian/`](debian/)), später optional über eine **Launchpad-PPA** für `apt`-Updates — passend zu Pop!_OS/Ubuntu/Debian.

**Bewusst kein Flatpak.** Ein Flatpak-Sandbox müsste sich breite Rechte erbitten — Lesezugriff auf `/proc`, `/sys`, `/etc/prime-discrete` und `/dev/nvidia*` — was dem Leitprinzip **„only read, never blocking"** und dem schlanken, transparenten Zugriffsmodell direkt widerspricht. Ein natives `.deb` ohne Sandbox-Sonderrechte ist hier ehrlicher und einfacher zu prüfen.

## Als Applet hinzufügen

COSMIC → **Einstellungen → Leiste** (oder **Dock**) → **Applets** → **„Vitals"** hinzufügen.
Dasselbe Applet lässt sich in Panel **und/oder** Dock platzieren. Klick öffnet das Detail-Popup.

## Einstellungen im Popup

Im Detail-Popup oben rechts das **Zahnrad** anklicken → Einstellungs-Ansicht (zurück über den **Pfeil**):

- **Metriken & Reihenfolge:** je Metrik ein Schalter (an/aus) plus **▲/▼** zum Umsortieren. Die Reihenfolge gilt sofort für die Werteliste und wird persistiert (`metric_order`).
- **Inhalt** (was das Popup zeigt): CPU-Temperatur, „GPU im Schlaf ausblenden", **Watt aufschlüsseln** (System/CPU/GPU als getrennte Werte).
- **Format:** °C/°F, **Netz-Einheit** (Klick zykliert SI → binär → Bit), Monospace-Schrift.
- **Darstellung:** **Balken im Popup** (CPU/RAM/GPU), **Verlaufs-Graphen (Sparklines)** — Master-Schalter plus eingerückte Einzelschalter für CPU/RAM/Netz/Watt — sowie **Labels in Akzentfarbe**.
- **Panel:** **Wert neben dem Panel-Icon** (kompakt, nur in horizontaler Leiste) mit eingerückter Auswahl **Panel-Wert** (CPU/RAM/Netz/GPU/Watt).
- **Aktualisierung:** Intervall in ms (Schritt 250, min 250).
- **Auf Standard zurücksetzen:** alle Optionen (inkl. Reihenfolge) auf die Defaults.

Jedes Label trägt ein ⓘ-Info-Tooltip, das die Option erklärt.

Alles greift **live** und landet in der cosmic-config (siehe unten).

## Konfiguration

Persistiert über cosmic-config (live, ohne Neustart). Optionen u. a.:

| Option | Bedeutung | Default |
|---|---|---|
| `interval_ms` | Aktualisierungsintervall (ms) | 1500 |
| `show_cpu` / `show_cpu_temp` / `show_mem` / `show_net` / `show_gpu` | Welche Werte inline im Panel | an |
| `fahrenheit` | Temperatur in °F | aus |
| `net_unit` | 0 = MB/s, 1 = MiB/s, 2 = Mbit/s | 0 |
| `show_fans` | Lüfterzeile im Popup | an |
| `show_power` | Watt-Zeile (System/CPU/GPU) im Popup | an |
| `power_breakdown` | Watt-Zeile zusätzlich in CPU-/GPU-Werte aufgeschlüsselt | an |
| `show_battery` | Akku-Zeile (Spannung, Status) im Popup | aus |
| `show_disk` / `show_swap` / `show_load` / `show_uptime` / `show_net_total` | die neueren Opt-in-Metrikzeilen | aus |
| `mono_font` | Monospace-Font in der Werteliste | an |
| `hide_gpu_when_asleep` | dGPU im Schlaf ganz ausblenden | aus |
| `per_core` | CPU pro Kern im Popup | an |
| `metric_order` | Reihenfolge der Metriken (IDs: 0=CPU, 1=RAM, 2=Netz, 3=GPU, 4=Lüfter, 5=Kerne, 6=Watt, 7=Akku, 8=Disk, 9=Swap, 10=Load, 11=Uptime, 12=Netz Σ) | `[0…12]` |
| `graphical` | Auslastungsbalken im Popup (CPU/RAM/GPU) | aus |
| `show_graphs` | Master-Schalter für Sparklines | an |
| `graph_cpu` / `graph_mem` / `graph_net` / `graph_power` | Sparkline-Einzelschalter je Metrik | an |
| `accent_labels` | fette Metrik-Labels in der System-Akzentfarbe | an |
| `panel_text` | Kompakter Wert neben dem Panel-Icon (nur horizontale Leiste) | aus |
| `panel_metric` | Welche Metrik im Panel-Text (0=CPU,1=RAM,2=Netz,3=GPU,6=Watt) | 0 |
| `warn_temp_c` / `crit_temp_c` | Schwellen für Farbwarnungen | 80 / 90 |

Konfigdatei: `~/.config/cosmic/io.github.grenzenloseschublade.CosmicAppletVitals/v1/`.

## Deinstallieren

```sh
just uninstall-user           # bzw. sudo just uninstall
```

## Bekannte Einschränkungen / Roadmap

- **Panel-Text nur horizontal.** Der kompakte Wert neben dem Icon erscheint nur in horizontaler Leiste; in vertikaler Leiste/Dock bleibt es beim reinen Icon (Text wäre dort zu breit).

*(In 1.0 gelöst: Die Metrik-Erfassung läuft jetzt via `spawn_blocking` auf einem Hintergrund-Thread — ein hängender NVML-Aufruf kann die UI nicht mehr einfrieren.)*

## Feedback & Mitwirken

Fragen, Fehlerberichte und Anregungen bitte über die **[GitHub-Issues](https://github.com/grenzenloseSchublade/cosmic-ext-applet-vitals/issues)** des Projekts — das ist der zentrale Kontakt- und Feedback-Kanal. Pull Requests sind willkommen.

## Lizenz

GPL-3.0-only.

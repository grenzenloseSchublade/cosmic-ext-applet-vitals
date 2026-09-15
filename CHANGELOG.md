# Changelog

## Unveröffentlicht

- **Fünf neue Metriken** (alle einzeln abschaltbar, in „Metriken & Reihenfolge"
  sortierbar): **Disk** (Lese-/Schreibrate über physische Laufwerke,
  /proc/diskstats), **Swap** (nur sichtbar wenn eingerichtet), **Load**
  (1/5/15 min), **Uptime** sowie **Netz Σ** (kumulierter RX/TX-Verbrauch seit
  Boot — aus den ohnehin gelesenen Zählern). Load/Uptime werden nur bei
  offenem Popup gelesen; Disk läuft als Delta-Metrik immer mit.

- **Neue Metrik „Watt":** Leistungsaufnahme System gesamt (RAPL `psys`,
  Fallback Akku-Entladeleistung), CPU-Paket (RAPL `package-0`) und GPU
  (NVML `power_usage()`, weiterhin nur bei wacher dGPU — kein Wecken).
- **Neue Metrik „Akku"** (optional, standardmäßig aus): Spannung, Ladezustand,
  Lade-/Entladeleistung; beim Laden Näherung „Netzteil ≈ psys + Ladeleistung".
- **RAPL-Freigabe als Opt-in:** udev-Regel (Gruppe `rapl`, 0440) via
  `just install-rapl-rule`; ohne Regel saubere Degradation
  (CVE-2020-8694-Abwägung im README).
- Panel-Wert kann jetzt auch „Watt" zeigen; Zyklus-Logik über die
  PANEL-Position statt roher ID (Bugfix für nicht-lückenlose IDs).
- Gespeicherte `metric_order` wird beim Laden um neue Metrik-IDs ergänzt —
  bestehende Einstellungen bleiben erhalten (kein Config-Versions-Bump).
- Packaging: metainfo.xml einheitlich nach `share/metainfo`; eingecheckte
  Debug-Binaries entfernt.
- Watt-Zeile einzeilig mit „·"-Trennern (Gesamt zuerst, statt Drei-Spalten-Layout
  mit Umbruch bei schmalem Popup); neuer Schalter „Watt aufschlüsseln"
  (`power_breakdown`) blendet die CPU/GPU-Teilwerte aus.
- Einstellungs-Seite scrollbar — vorher wurde die Liste unterhalb der
  Popup-Höhe abgeschnitten („Auf Standard zurücksetzen" unerreichbar).
- **Info-Tooltips in den Einstellungen:** jedes Label (inkl. Metrik-Zeilen und
  Abschnitts-Header „Metriken & Reihenfolge") hat ein ⓘ-Icon mit Erklärtext
  beim Hovern. Info-Boxen mit abgesetztem Komponenten-Hintergrund und Rand in
  der System-Akzentfarbe (zentral im `info_box`-Helper).
- **Hover-Erklärungen in der Hauptansicht:** die fetten Metrik-Wörter (CPU,
  GPU, Watt, …) zeigen beim Hovern eine Info-Box, die die Anzeige konkret
  erklärt (Spalten, GPU-Zustände, Watt-Quellen); nur das Wort ist hoverbar.
  Die Box ist ein echtes Wayland-Popup (`xdg_popup`) und öffnet links vom
  Wort **über den Fensterrand hinaus** — ein normaler iced-Tooltip wird ins
  Fenster geklemmt und läge über den Werten.
- **Akzentfarbe für Metrik-Beschriftungen** im Popup (neuer Schalter
  „Beschriftungen in Akzentfarbe" unter „Darstellung", Standard an) — nutzt
  die in COSMIC gewählte Akzentfarbe, keine hartkodierten Farben; Orange/Rot
  bleiben den Temperatur-Warnschwellen vorbehalten.
- **Weniger Syscalls pro Tick:** hwmon-Pfade (CPU-/RAM-Temp, Lüfter) und
  Akku-Pfade werden einmal aufgelöst und gecacht statt das Verzeichnis bei
  jedem Tick zu scannen (vorher 3–5 volle `/sys/class/hwmon`-Scans pro Tick);
  Invalidierung bei Lesefehler, Akku-Rescan periodisch (Hot-Swap), fehlende
  Sensoren werden beim Popup-Öffnen erneut gesucht (Modul-Nachladen).
- Nur im Popup sichtbare Momentanwerte (Temperaturen, Lüfter, per-Core-Prozente,
  PRIME-Modus) werden bei geschlossenem Popup nicht mehr erhoben; beim Öffnen
  stößt das Applet sofort eine Erfassung an. `/proc/stat`-Parsing ohne
  Zwischen-Allokationen.

## 1.0.0 — 2026-06-25

Erste stabile Version.

- **Metrik-Erfassung im Hintergrund:** Die (blockierende) Erfassung läuft via
  `tokio::task::spawn_blocking` auf einem Hintergrund-Thread, nie auf dem UI-Thread.
  Ein NVML-Stall friert die Oberfläche nicht mehr ein (Werte werden höchstens kurz
  veraltet). In-Flight-Guard verhindert Thread-Stau.
- Damit ist der letzte Roadmap-Punkt erfüllt; ansonsten funktionsgleich zu 0.9.0.

## 0.9.0 — 2026-06-25

Erste öffentliche Vorab-Version. Funktionsvollständig; v1.0 folgt, sobald die
Metrik-Erfassung in einen Hintergrund-Task ausgelagert ist.

### Funktionen
- System-Monitor-Applet für die COSMIC-Leiste/Dock: CPU- und RAM-Auslastung,
  Temperaturen, Netz-Durchsatz, Lüfter, NVIDIA-GPU.
- **Einstellungen im Popup** (Zahnrad): Metriken an/aus, Reihenfolge (▲/▼),
  °C/°F, Netz-Einheit, Intervall, Monospace, „Auf Standard zurücksetzen".
- **Darstellung:** Werte- oder Balken-Ansicht (CPU/RAM/GPU, zweizeilig); optional
  kompakter Wert neben dem Panel-Icon (dynamische Breite); 3-Spalten-Layout
  (Auslastung · Detail · Temp). Temp-Werte ab Warn-/Kritisch-Schwelle eingefärbt.
- **GPU:** util/VRAM/Temp nur bei aktiver dGPU; Netz-Typ WLAN/LAN/VPN.

### Design-Prinzipien
- **only read, never blocking:** nur lesen aus `/proc`, `/sys` und NVML in-process —
  kein Subprozess, keine Netzwerkaktivität, keine Telemetrie („kein Heimtelefonieren").
- **Suspend-/akkufreundlich:** die NVIDIA-dGPU wird nie geweckt oder wachgehalten
  (NVML nur bei ohnehin aktiver GPU + offenem Popup; Handle wird sonst freigegeben).
- **Hardware-anpassbar:** alle board-/treiber-spezifischen Pfade und Sensor-Namen
  zentral in [`src/hw.rs`](src/hw.rs).

### Bekannt / geplant für 1.0
- Metrik-Erfassung läuft noch im UI-Thread → in einen Hintergrund-Task auslagern.

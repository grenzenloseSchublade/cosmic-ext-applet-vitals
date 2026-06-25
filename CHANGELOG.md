# Changelog

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

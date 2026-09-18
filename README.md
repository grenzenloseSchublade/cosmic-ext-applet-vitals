# Vitals — COSMIC system monitor applet

*🌐 Language: **English** · [Deutsch](README.de.md)*

A lean, **robust** system monitor applet for the [COSMIC](https://system76.com/cosmic) panel or dock. Shows **CPU/RAM usage, temperatures, network throughput** and the **NVIDIA GPU status** — with a detail popup on click.

## Design philosophy: *only read, never blocking*

The guiding principle is **"only read, never blocking"** — the applet only **reads** system state and never does anything that could block or change hardware state:

- **Only read:** all values come from `/proc`, `/sys` and in-process NVML — **no** subprocess, **no** writes, **no** waking of sleeping devices.
- **Never blocking:** no calls that can hang in a driver/during suspend. A sleeping NVIDIA dGPU (RTD3) is **never** woken (NVML only when `runtime_status == active`); missing sensors are hidden instead of blocking or panicking.

This keeps the applet lightweight and is the direct answer to the suspend hang of typical monitors (see below).

## Why this applet?

> **What makes Vitals different:** as far as we know, it is the **first/only COSMIC system-monitor applet that keeps an NVIDIA hybrid dGPU asleep**. Other monitors either spawn `nvidia-smi` (which hangs suspend) or hold a persistent NVML handle that **pins the dGPU awake** (battery drain, no RTD3). Vitals never wakes or pins it — see *only read, never blocking* above.

Typical system monitor applets spawn `nvidia-smi` as a subprocess on **every tick** to read the GPU. During suspend such a call hangs in the tearing-down NVIDIA driver and blocks sleep for ~90 s (systemd timeout). Vitals avoids this **by design**:

- **Not a single subprocess** — all values come straight from `/proc` and `/sys`.
- **Battery- and suspend-friendly GPU:** the NVIDIA dGPU is queried only via **in-process NVML** — and only when it is already awake per sysfs (`runtime_status == active`) **and** the detail popup is open. Otherwise the NVML handle is **released** (`/dev/nvidia0` closed) so the dGPU can sleep via **RTD3**. Thus the applet never **pins** the dGPU and never **wakes** it. State (asleep/active) + mode are read pin-free from sysfs; live values (usage/VRAM/temp) only appear when the GPU is actually working and you're looking.
- **Panic-free:** missing sensors (e.g. a blacklisted `spd5118` for RAM temp) are simply hidden.
- **Independent of `nvidia-powerd`** — works with Dynamic Boost on or off.

## Display

- **Panel:** symbolic chip icon, optionally with a compact value next to it (e.g. `CPU 12%` or `↓1.2M/s ↑0.1M/s`, horizontal panel only). The applet area **grows dynamically** with the text (via `core.applet.autosize_window`).
- **Popup (details):** CPU (total + temp, optional per core), RAM (used/total + temp), network (↓/↑ + type **WLAN/LAN/VPN**), GPU (when the dGPU is active: usage + VRAM + temp; otherwise state/mode), fan speeds, **power draw** (system total / CPU package / GPU, in watts — see *Power measurement (RAPL)* below) and optionally a **battery line** (voltage, charging state, charge/discharge power). Additional opt-in metrics (off by default): **disk I/O** (read/write rate over physical drives), **swap**, **load average** (1/5/15 min), **uptime** and **cumulative network total** since boot. Optionally as **usage bars** (two-line, full width) for CPU/RAM/GPU.
- **History graphs (sparklines):** a ~3-minute mini-graph under CPU, RAM, network (↓ solid, ↑ dimmed) and watts, drawn in the system accent color; history keeps collecting while the popup is closed. Hovering shows the value and time offset at the cursor; autoscaled graphs carry a dotted max reference line and a `≤ max · span` caption. Master toggle plus per-metric toggles.
- **Info tooltips everywhere:** every settings label has an ⓘ icon with an explanation; the bold metric words in the main view show a structured explanation box on hover (a real Wayland popup that may extend beyond the window edge, so it never covers the values).

## Data sources

| Value | Source |
|---|---|
| CPU load | `/proc/stat` (delta) |
| CPU temp | hwmon `coretemp` "Package id 0" (fallback `k10temp`/`acpitz`) |
| RAM | `/proc/meminfo` |
| RAM temp | hwmon `spd5118` (optional) |
| Fans | hwmon `thinkpad` `fan*_input` |
| Net ↑/↓ | `/sys/class/net/<if>/statistics/{rx,tx}_bytes` (default-route interface) |
| Net type | WLAN (`…/<if>/wireless`), VPN (`tun`/`tap`/`wg`/`ppp`), otherwise LAN |
| dGPU state | `/sys/bus/pci/devices/<nvidia>/power/runtime_status` |
| NVIDIA mode | `/etc/prime-discrete` |
| GPU usage/temp/VRAM/power | NVML (`libnvidia-ml`, `memory_info()`, `power_usage()`) — only when the dGPU is active |
| System / CPU package power | RAPL `/sys/class/powercap` (`psys` / `package-0`, counter delta) — needs the opt-in udev rule, see below |
| Battery voltage/power/state | `/sys/class/power_supply/BAT*/{voltage_now,power_now,status}` |
| Disk I/O | `/proc/diskstats` (512-byte sectors, delta; partitions and stacked devices like `dm-`/`md`/`zram` excluded to avoid double counting) |
| Swap | `/proc/meminfo` (`SwapTotal`/`SwapFree`) |
| Load average | `/proc/loadavg` |
| Uptime | `/proc/uptime` |
| Net total (Σ) | cumulative `rx_bytes`/`tx_bytes` — same counters as the rate |

## Adapting to other hardware

All hardware-/board-specific paths and sensor identifiers live **in one place**: [`src/hw.rs`](src/hw.rs). For other hardware, edit only there — no hunting through the rest of the code:

- **CPU temp:** `CPU_TEMP_LABELED` (chip + `temp*_label`); Intel `coretemp`/"Package id 0" and AMD `k10temp`/"Tctl" are included; fallback `acpitz`.
- **RAM temp:** `RAM_TEMP_CHIP` (default `spd5118`).
- **Fans:** `FAN_CHIP` (default `thinkpad`) + `FAN_MAX_INDEX` — e.g. add `dell_smm` or `nct6…`.
- **GPU:** `PRIME_DISCRETE_PATH`, `NVIDIA_VENDOR_ID`, `DISPLAY_CLASS_PREFIX`, `RUNTIME_STATUS_REL`.
- **Net:** `VPN_IFACE_PREFIXES`.

Find your own hwmon chip names:

```sh
for f in /sys/class/hwmon/hwmon*/name; do echo "$f -> $(cat "$f")"; done
grep . /sys/class/hwmon/hwmon*/temp*_label
```

## Power measurement (RAPL)

The watts line shows up to three values: **system total**, **CPU package** (incl. iGPU) and **GPU**. System and CPU come from Intel/AMD **RAPL** energy counters under `/sys/class/powercap` (`psys` and `package-0`, computed as counter delta per tick, wrap-around handled).

**Permissions:** since kernel 5.10, `energy_uj` is readable by **root only** — the mitigation for [CVE-2020-8694](https://nvd.nist.gov/vuln/detail/CVE-2020-8694) ("PLATYPUS", a power side channel that can leak secrets across users on multi-user machines). Vitals therefore ships an **opt-in** udev rule that re-exposes the counters to members of a dedicated `rapl` group (mode 0440 — deliberately **not** world-readable):

```sh
just install-rapl-rule    # creates group "rapl", adds you, installs the udev rule
# then: log out/in once (group membership), restart the applet
```

**Trade-off:** on a single-user laptop the risk is low; on shared/multi-user machines consider **not** installing the rule. Remove it with `just uninstall-rapl-rule`.

**Without the rule** the applet degrades gracefully: on battery the system total falls back to the battery's discharge rate (`power_now`); on AC no total is shown ("– · Netz") because the battery then only measures the charge rate. The GPU value is independent of RAPL (NVML, only while the dGPU is awake and the popup is open — the dGPU is still **never** woken). While charging, the battery line additionally estimates the **draw from the charger** (`psys` + charge power; conversion losses not included). A real measurement of the USB-C charger output is not possible — the PD controller (UCSI) only reports the negotiated contract, not live values. True CPU core voltage (Vcore) is not exposed on modern laptops without root/MSR access; only the battery voltage is shown.

## Privacy — no phoning home

Vitals **does not phone home**: no network connection, no telemetry/analytics, no auto-update callback, no tracking. It reads **purely locally** from `/proc`, `/sys` and the NVIDIA library **in-process** (no subprocess) and **writes/sends nothing** outward — the only persistence is the local settings file. This is the direct consequence of the **"only read, never blocking"** design philosophy and can be verified from the lean source.

## Building

Requires: Rust (`cargo`), Wayland/xkb dev packages (for libcosmic).

```sh
cargo build --release         # or: just build-release
```

### UI layout model (for contributors)

The UI is built with libcosmic/iced widgets, **not** HTML/CSS — but the layout model is close to CSS flexbox: `widget::row()` / `widget::column()` are flex containers (main axis horizontal/vertical), `.width(Length::Fill)` behaves like `flex-grow`, `Length::Shrink` like fit-content, fixed lengths like a px width. `.spacing()` is the gap between children, `.padding()` the inner padding, `.align_y(Alignment::Center)` cross-axis alignment. There are no CSS classes; styling comes from **theme classes** (e.g. `cosmic::theme::Container::Dropdown`) or custom style closures, which adapt to light/dark automatically. Conventions in this codebase:

- Settings rows are `widget::settings::item_row(vec![label, control])`; the label takes `Length::Fill` so all controls share one right edge.
- Horizontal indentation of headings and footer buttons is `theme::spacing().space_m` (see `padded_heading` in `src/app.rs`) — reuse it instead of hard-coding pixels.
- All explanatory tooltips go through the `info_box` helper in `src/app.rs` — one place that defines their container class, padding and max width.

### Measuring per-tick syscalls

The collector's efficiency claims (cached hwmon/battery paths, popup-gated sensor reads) can be verified with `strace` — but attaching to a running process needs ptrace rights (Yama `ptrace_scope`), i.e. `sudo`:

```sh
sudo strace -f -e trace=openat -p $(pgrep -f cosmic-ext-applet-vitals | head -1) -o /tmp/vitals.strace &
sleep 10; sudo pkill -x strace
grep -c hwmon /tmp/vitals.strace   # expected: 0 while the popup is closed
```

## Installing

**A) Prebuilt `.deb` (amd64) — easiest:**

Download the `.deb` from the [latest release](https://github.com/grenzenloseSchublade/cosmic-ext-applet-vitals/releases/latest) and install it:

```sh
sudo apt install ./cosmic-ext-applet-vitals_1.0.0_amd64.deb
```

The prebuilt binary targets **amd64** with a recent glibc (Pop!_OS / Ubuntu 24.04+ / Debian 13+ class). libcosmic is statically linked; the only dynamic dependencies are system libraries (`libc6`, `libgcc-s1`, `libxkbcommon0`). It is **architecture**-specific, not machine-specific — hwmon chips are detected at runtime with graceful fallback.

**B) From source — user-local (no sudo, recommended for personal use):**

```sh
./install.sh                  # or: just install-user
```

Installs the binary to `~/.local/bin`, the `.desktop` file to `~/.local/share/applications`. System-wide: `sudo just install` (prefix=/usr).

**Deploying a new build (restart the applet):** installing only replaces the file on disk — the running applet keeps executing the old (deleted) binary until it is restarted. `cosmic-panel` does **not** respawn a killed applet on its own; restart the panel instead (cosmic-session respawns it together with all applets):

```sh
just install-user
kill $(pgrep -x cosmic-panel)     # session restarts panel + applets automatically
# verify the new binary is running (no "(deleted)" suffix):
for p in $(pgrep -f cosmic-ext-applet-vitals); do readlink /proc/$p/exe; done
```

Note: `pkill -f cosmic-ext-applet-vitals` is a trap in scripts — the pattern also matches the shell that runs the command, killing your own script. Filter by `/proc/<pid>/exe` as above instead.

**C) Build your own `.deb`:**

```sh
sudo apt install debhelper pkg-config libxkbcommon-dev libwayland-dev   # once
dpkg-buildpackage -b -us -uc                                            # produces ../*.deb
sudo apt install ../cosmic-ext-applet-vitals_1.0.0_*.deb
```

A recent Rust toolchain (`cargo`/`rustc`) is required; the build fetches the crates from the network. The packaging lives under [`debian/`](debian/) (native format).

**D) PPA:** *planned* — a Launchpad PPA for convenient `apt` updates is intended for the future but not set up yet.

## Distribution — and why no Flatpak

**Channels:** source on **GitHub**; native **`.deb`** packages (built via [`debian/`](debian/)), later optionally a **Launchpad PPA** for `apt` updates — suited to Pop!_OS/Ubuntu/Debian.

**Deliberately no Flatpak.** A Flatpak sandbox would have to request broad permissions — read access to `/proc`, `/sys`, `/etc/prime-discrete` and `/dev/nvidia*` — which directly contradicts the **"only read, never blocking"** principle and the lean, transparent access model. A native `.deb` without sandbox exceptions is more honest here and easier to audit.

## Adding the applet

COSMIC → **Settings → Panel** (or **Dock**) → **Applets** → add **"Vitals"**.
The same applet can be placed in the panel **and/or** the dock. A click opens the detail popup.

## Settings in the popup

In the detail popup, click the **gear** in the top right → settings view (back via the **arrow**):

- **Metrics & order:** a toggle per metric (on/off) plus **▲/▼** to reorder. The order applies immediately to the value list and is persisted (`metric_order`).
- **Content** (what the popup shows): CPU temperature, "hide GPU while asleep", **watt breakdown** (system/CPU/GPU as separate values).
- **Format:** °C/°F, **net unit** (click cycles SI → binary → bit), monospace font.
- **Presentation:** **bars in the popup** (CPU/RAM/GPU), **history graphs (sparklines)** — master toggle plus indented per-metric toggles for CPU/RAM/Net/Watts — and **accent-colored labels**.
- **Panel:** **value next to the panel icon** (compact, horizontal panel only) with the indented **panel value** picker (CPU/RAM/Net/GPU/Watts).
- **Refresh:** interval in ms (step 250, min 250).
- **Reset to defaults:** all options (including the order) back to defaults.

Every label carries an ⓘ info tooltip explaining the option.

Everything applies **live** and is stored in cosmic-config (see below).

## Configuration

Persisted via cosmic-config (live, no restart). Options include:

| Option | Meaning | Default |
|---|---|---|
| `interval_ms` | refresh interval (ms) | 1500 |
| `show_cpu` / `show_cpu_temp` / `show_mem` / `show_net` / `show_gpu` | which values appear | on |
| `fahrenheit` | temperature in °F | off |
| `net_unit` | 0 = MB/s, 1 = MiB/s, 2 = Mbit/s | 0 |
| `show_fans` | fan line in the popup | on |
| `show_power` | watts line (system/CPU/GPU) in the popup | on |
| `power_breakdown` | watt line additionally split into CPU/GPU values | on |
| `show_battery` | battery line (voltage, state) in the popup | off |
| `show_disk` / `show_swap` / `show_load` / `show_uptime` / `show_net_total` | the newer opt-in metric lines | off |
| `mono_font` | monospace font in the value list | on |
| `hide_gpu_when_asleep` | hide the dGPU entirely while asleep | off |
| `per_core` | CPU per core in the popup | on |
| `metric_order` | order of metrics (IDs: 0=CPU, 1=RAM, 2=Net, 3=GPU, 4=Fans, 5=Cores, 6=Watts, 7=Battery, 8=Disk, 9=Swap, 10=Load, 11=Uptime, 12=Net Σ) | `[0…12]` |
| `graphical` | usage bars in the popup (CPU/RAM/GPU) | off |
| `show_graphs` | sparklines master toggle | on |
| `graph_cpu` / `graph_mem` / `graph_net` / `graph_power` | per-metric sparkline toggles | on |
| `accent_labels` | bold metric labels in the system accent color | on |
| `panel_text` | compact value next to the panel icon (horizontal panel only) | off |
| `panel_metric` | which metric in the panel text (0=CPU, 1=RAM, 2=Net, 3=GPU, 6=Watts) | 0 |
| `warn_temp_c` / `crit_temp_c` | thresholds for colored temperature warnings | 80 / 90 |

Config files: `~/.config/cosmic/io.github.grenzenloseschublade.CosmicAppletVitals/v1/`.

## Uninstalling

```sh
just uninstall-user           # or: sudo just uninstall
```

## Known limitations / roadmap

- **Panel text only horizontal.** The compact value next to the icon appears only in a horizontal panel; in a vertical panel/dock it stays icon-only (text would be too wide there).

*(Resolved in 1.0: metric collection now runs on a background thread via `spawn_blocking`, so a stalled NVML call can never freeze the UI.)*

## Feedback & contributing

Questions, bug reports and suggestions: please use the project's **[GitHub issues](https://github.com/grenzenloseSchublade/cosmic-ext-applet-vitals/issues)** — that is the central contact and feedback channel. Pull requests welcome.

## License

GPL-3.0-only.

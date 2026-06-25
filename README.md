# Vitals — COSMIC system monitor applet

*🌐 Language: **English** · [Deutsch](README.de.md)*

A lean, **robust** system monitor applet for the [COSMIC](https://system76.com/cosmic) panel or dock. Shows **CPU/RAM usage, temperatures, network throughput** and the **NVIDIA GPU status** — with a detail popup on click.

## Design philosophy: *only read, never blocking*

The guiding principle is **"only read, never blocking"** — the applet only **reads** system state and never does anything that could block or change hardware state:

- **Only read:** all values come from `/proc`, `/sys` and in-process NVML — **no** subprocess, **no** writes, **no** waking of sleeping devices.
- **Never blocking:** no calls that can hang in a driver/during suspend. A sleeping NVIDIA dGPU (RTD3) is **never** woken (NVML only when `runtime_status == active`); missing sensors are hidden instead of blocking or panicking.

This keeps the applet lightweight and is the direct answer to the suspend hang of typical monitors (see below).

## Why this applet?

Typical system monitor applets spawn `nvidia-smi` as a subprocess on **every tick** to read the GPU. During suspend such a call hangs in the tearing-down NVIDIA driver and blocks sleep for ~90 s (systemd timeout). Vitals avoids this **by design**:

- **Not a single subprocess** — all values come straight from `/proc` and `/sys`.
- **Battery- and suspend-friendly GPU:** the NVIDIA dGPU is queried only via **in-process NVML** — and only when it is already awake per sysfs (`runtime_status == active`) **and** the detail popup is open. Otherwise the NVML handle is **released** (`/dev/nvidia0` closed) so the dGPU can sleep via **RTD3**. Thus the applet never **pins** the dGPU and never **wakes** it. State (asleep/active) + mode are read pin-free from sysfs; live values (usage/VRAM/temp) only appear when the GPU is actually working and you're looking.
- **Panic-free:** missing sensors (e.g. a blacklisted `spd5118` for RAM temp) are simply hidden.
- **Independent of `nvidia-powerd`** — works with Dynamic Boost on or off.

## Display

- **Panel:** symbolic chip icon, optionally with a compact value next to it (e.g. `CPU 12%` or `↓1.2M/s ↑0.1M/s`, horizontal panel only). The applet area **grows dynamically** with the text (via `core.applet.autosize_window`).
- **Popup (details):** CPU (total + temp, optional per core), RAM (used/total + temp), network (↓/↑ + type **WLAN/LAN/VPN**), GPU (when the dGPU is active: usage + VRAM + temp; otherwise state/mode), fan speeds. Optionally as **usage bars** (two-line, full width) for CPU/RAM/GPU.

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
| GPU usage/temp/VRAM | NVML (`libnvidia-ml`, `memory_info()`) — only when the dGPU is active |

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

## Privacy — no phoning home

Vitals **does not phone home**: no network connection, no telemetry/analytics, no auto-update callback, no tracking. It reads **purely locally** from `/proc`, `/sys` and the NVIDIA library **in-process** (no subprocess) and **writes/sends nothing** outward — the only persistence is the local settings file. This is the direct consequence of the **"only read, never blocking"** design philosophy and can be verified from the lean source.

## Building

Requires: Rust (`cargo`), Wayland/xkb dev packages (for libcosmic).

```sh
cargo build --release         # or: just build-release
```

## Installing

**A) Prebuilt `.deb` (amd64) — easiest:**

Download the `.deb` from the [latest release](https://github.com/grenzenloseSchublade/cosmic-ext-applet-vitals/releases/latest) and install it:

```sh
sudo apt install ./cosmic-ext-applet-vitals_0.9.0_amd64.deb
```

The prebuilt binary targets **amd64** with a recent glibc (Pop!_OS / Ubuntu 24.04+ / Debian 13+ class). libcosmic is statically linked; the only dynamic dependencies are system libraries (`libc6`, `libgcc-s1`, `libxkbcommon0`). It is **architecture**-specific, not machine-specific — hwmon chips are detected at runtime with graceful fallback.

**B) From source — user-local (no sudo, recommended for personal use):**

```sh
./install.sh                  # or: just install-user
```

Installs the binary to `~/.local/bin`, the `.desktop` file to `~/.local/share/applications`. System-wide: `sudo just install` (prefix=/usr).

**C) Build your own `.deb`:**

```sh
sudo apt install debhelper pkg-config libxkbcommon-dev libwayland-dev   # once
dpkg-buildpackage -b -us -uc                                            # produces ../*.deb
sudo apt install ../cosmic-ext-applet-vitals_0.9.0_*.deb
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
- **Display:** CPU temperature, °C/°F, monospace font, "hide GPU while asleep", **net unit** (click cycles SI → binary → bit).
- **Presentation:** **bars in the popup** (graphical usage for CPU/RAM/GPU instead of text), **value next to the panel icon** (compact, horizontal panel only), and **panel value** (which metric is shown there: CPU/RAM/Net/GPU).
- **Refresh:** interval in ms (step 250, min 250).
- **Reset to defaults:** all options (including the order) back to defaults.

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
| `mono_font` | monospace font in the value list | on |
| `hide_gpu_when_asleep` | hide the dGPU entirely while asleep | off |
| `per_core` | CPU per core in the popup | on |
| `metric_order` | order of metrics (IDs: 0=CPU, 1=RAM, 2=Net, 3=GPU, 4=Fans, 5=Cores) | `[0,1,2,3,4,5]` |
| `graphical` | usage bars in the popup (CPU/RAM/GPU) | off |
| `panel_text` | compact value next to the panel icon (horizontal panel only) | off |
| `panel_metric` | which metric in the panel text (0=CPU, 1=RAM, 2=Net, 3=GPU) | 0 |
| `warn_temp_c` / `crit_temp_c` | thresholds for colored temperature warnings | 80 / 90 |

Config files: `~/.config/cosmic/io.github.grenzenloseschublade.CosmicAppletVitals/v1/`.

## Uninstalling

```sh
just uninstall-user           # or: sudo just uninstall
```

## Known limitations / roadmap

- **Metric collection runs on the UI thread.** Since NVML is only called for an *awake* dGPU, a hang is very unlikely; a theoretical NVML stall (driver bug while the GPU is active) could briefly freeze the display, though. Planned: move collection to a background task and return the result via a message.
- **Panel text only horizontal.** The compact value next to the icon appears only in a horizontal panel; in a vertical panel/dock it stays icon-only (text would be too wide there).

## Feedback & contributing

Questions, bug reports and suggestions: please use the project's **[GitHub issues](https://github.com/grenzenloseSchublade/cosmic-ext-applet-vitals/issues)** — that is the central contact and feedback channel. Pull requests welcome.

## License

GPL-3.0-only.

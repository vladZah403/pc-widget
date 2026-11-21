# PC Monitor 2.0 — stage 0 prototypes

Throwaway code that answers the questions of the 2.0 plan before the real
crates (`pcm-core`, `pcm-win`, `pcm-render`, `pcm-app`) are written. All
numbers were measured on the developer laptop (Ryzen 5 5500U, Radeon iGPU,
Windows 10, no admin rights) on 2026-11-21.

| Prototype | What it is | Result | Plan target |
|---|---|---|---|
| `fps/` | ETW frame counter, a port of 1.8's `FpsReader` | Same numbers as 1.8 on the same frames (dwm: 26.54 vs 26.54) | ±1 FPS in 3 games — **waiting for the game test** (`compare.bat`) |
| `sensors-helper/` | `pcm-sensors.exe`, C# 5 / .NET Framework 4.8, LibreHardwareMonitorLib 0.9.6, JSON lines on stdout | 26 MB private, 7 ms a reading, ~0.75% of one core | helper + app ≤ 70 MB — **admin run pending** (`snapshot.bat`) |
| `ring-window/` | layered window, tiny-skia + fontdue card, `UpdateLayeredWindow` | 0.05% of one core, 4.6 MB private, 0.39 ms a frame | ≤ 1% CPU idle |
| `dashboard/` | eframe 0.37 (glow) window, tiny-skia card as a texture + egui cards | open: 0.4% of one core, 102 MB private, first frame 135 ms; after closing: 46–49 MB stay in the process | open ≤ 120 MB, closed ≤ 30 MB |

## Findings

- **egui, not Tauri.** egui with OpenGL looks right with the 1.8 palette and
  system fonts, idles at one frame a second and stays under the 120 MB
  target. wgpu (Direct3D 12) was much heavier: 357 MB open, 266 MB after
  closing (`--features wgpu`).
- **The dashboard runs as its own process** (`PC-Monitor.exe --dashboard`).
  In-process, the OpenGL driver keeps ~45 MB after the window is destroyed,
  so the 30 MB target can't be met; a child process gives all of it back on
  close. It opens in ~0.2 s and gets live data from the parent over its
  stdin (no pipes or ports anyone else can open).
- **Cache everything static in cards.** The first ring window drew
  everything every frame and used 1.37% CPU; with the background and glyphs
  cached and unchanged frames skipped it is 0.05%.
- **Battery stays out of the helper.** LibreHardwareMonitorLib spends ~70 ms
  per battery update; 2.0 reads `IOCTL_BATTERY_*` itself.
- **GPU temperature without admin rights** comes from
  `D3DKMTQueryAdapterInfo(KMTQAITYPE_ADAPTERPERFDATA)` (as in Task Manager):
  41.7 °C on the Radeon iGPU, where 1.8 shows "≈CPU". NVIDIA and Intel are
  still to be checked.
- **FPS without admin rights:** the DXGI provider delivers events to a
  non-elevated session; whether the kernel provider (DX9 / OpenGL / Vulkan)
  needs admin is still open (dwm doesn't raise `Present_Info`).

## Toolchain

Rust stable (MSVC) with Visual Studio Build Tools 2026 ("Desktop
development with C++"). The gnu toolchain alone can't link eframe 0.37
(raw-dylib needs a full MinGW). On a PC whose path has non-ASCII
characters, put `target-dir` outside the project in a local
`.cargo/config.toml`.

```
cargo build --release -p pcm-proto-fps        # FPS
cargo build --release -p pcm-proto-ring       # ring window
cargo build --release -p pcm-proto-dashboard  # egui window
prototypes\sensors-helper\build.bat           # C# helper
```

# kompact

A minimal Wayland compositor, built from scratch on Smithay 0.4. Two backends, one binary:

- `--backend drm`: runs directly on real hardware via DRM/KMS + libseat, from a bare TTY with no
  X or Wayland session underneath. This is the main target (tested on an NVIDIA GT 710 with the
  nouveau driver).
- `--backend winit`: runs nested inside an existing session (X11 or another Wayland compositor),
  for development in a VM.

Without `--backend` it guesses: bare TTY (no `$DISPLAY`/`$WAYLAND_DISPLAY`) means drm, anything
else means winit.

See `ROADMAP.md` for the phase plan and what has been confirmed on hardware.

## What's implemented

**Protocols:** `wl_compositor`, `xdg_shell` (toplevels and popups with real grabs), `wl_shm`,
`wl_seat`, `wl_output` + xdg-output, data device (clipboard and drag-and-drop),
`wlr-layer-shell` (panels, docks, wallpapers, launchers; exclusive zones are respected),
`cursor-shape-v1`.

**Window management (Phase 2):**

- **Smart placement:** new windows go into open space if there is any, otherwise they cascade in
  32 px steps, taking the first free slot and wrapping at the screen edge, so nothing opens
  off-screen or exactly on top of another window. Panels are kept clear.
- **Snapping:** drag a window to the left/right screen edge for that half, into a corner for that
  quadrant, or to the top edge to maximize. `Super+Arrow` does the same from the keyboard (see
  below). Un-snapping restores the previous size and position.
- **Remembered geometry per app:** when an app's window closes, its size and position are saved
  by app-id and reused the next time it opens (if no other window of it is open). Saved across
  restarts in `~/.local/state/kompact/geometry` (`$XDG_STATE_HOME` if set), at most 64 apps.
- **Focus:** click to focus (default), or focus-follows-mouse with `--focus-follows-mouse` or
  `Super+M` at runtime (sloppy: hovering focuses without raising; empty desktop and panels keep
  the last focus). Closing the focused window focuses the top-most remaining one.
- **Resizing:** drag any of a window's 8 edges/corners (client-side borders, e.g. foot), or
  `Super+right-drag` from the nearest edge/corner. Left/top edges keep the opposite edge fixed.
  Resize cursors are shown over borders and during the drag; apps' own cursors (e.g. Firefox's
  link hand) are drawn too.
- **Workspaces:** 9 virtual desktops. Windows on hidden workspaces aren't drawn. Panels show on
  every workspace.

**Session integration:** on the DRM backend, `WAYLAND_DISPLAY` is exported to `systemd --user`
and D-Bus activation at startup (and removed on exit), so services like `xdg-desktop-portal-gtk`
can reach the compositor. Without this, snap Firefox took ~26 s to open (portal timeouts).

## Keybindings

Shortcuts come from a **preset**, which you can change key by key (Phase 10, `src/keymap.rs`):
**macOS** (the default; Super plays the Command key), **Windows 11**, or **kompact classic** (the
keys before Phase 10). The startup log lists every key (`keys: Windows: …` lines). Your choice and
changes live in `~/.config/kompact/keys`:

```text
preset = windows
window.close = Super+W
desktop.show = none
```

`KOMPACT_KEYS=classic|macos|windows` picks a preset and ignores the file. `Ctrl+Alt+F1..F12`
(switch VT) is fixed in every preset; Quit and Lock always keep a key.

| Action | macOS | Windows 11 | classic |
|---|---|---|---|
| Spotlight | `Super+Space` | `Super+S`, tap `Super` | `Super+Space` |
| Next window | `Super+Tab` | `Alt+Tab` | `Super+Tab` |
| Close window | `Super+Q` | `Alt+F4` | `Super+Q` |
| Minimize | `Super+M` | `Super+Down` (not snapped) | – |
| Restore last minimized | `Super+U` | `Super+U` | `Super+U` |
| Maximize / restore | `Ctrl+Super+F` | `Super+F` | `Super+F` |
| Snap (halves, quadrants; Up maximizes) | `Ctrl+Super+arrows` | `Super+arrows` | `Super+arrows` |
| Show workspace 1–9 | `Ctrl+1–9` | `Ctrl+Super+1–9` | `Super+1–9` |
| Send window to workspace 1–9 | `Ctrl+Shift+1–9` | `Shift+Super+1–9` | `Shift+Super+1–9` |
| Previous / next workspace | `Ctrl+Left/Right` | `Ctrl+Super+Left/Right` | – |
| Workspace overview | `Ctrl+Up` | `Super+Tab` | – |
| Show desktop | `Super+F3` | `Super+D` | – |
| Terminal (`foot`, or `--spawn`) | `Super+Return` | `Super+Return` | `Super+Return` |
| Files | `Shift+Super+F` | `Super+E` | – |
| Lock | `Ctrl+Super+Q` | `Super+L` | `Super+L` |
| Quit kompact | `Shift+Super+Escape` | `Shift+Super+Escape` | `Super+Escape` |
| Screenshot: every monitor | `Shift+Super+3`, `Print` | `Print` | `Print` |
| Screenshot: monitor under the pointer | `Ctrl+Print` | `Alt+Print` | `Ctrl+Print` |
| Screenshot: a region | `Shift+Super+4`, `Shift+Print` | `Shift+Super+S`, `Shift+Print` | `Shift+Print` |
| Focus follows mouse | – | – | `Super+M` |
| Debug overlay | `Ctrl+Alt+Super+D` | `Ctrl+Alt+Super+D` | `Super+D` |

**Mouse:** `Super+left-drag` moves a window (release at an edge/corner to snap),
`Super+right-drag` resizes from the nearest edge/corner, click focuses and raises. Apps can also
move, resize, maximize and fullscreen themselves (e.g. dragging foot's title bar or border).

## Not implemented yet

- Multi-monitor arrangement settings (Phase 5, deferred): every connected monitor is driven
  side by side in connector order and follows hotplug (`KOMPACT_OUTPUTS=first` for one screen,
  `KOMPACT_PRIMARY=<connector>` to choose the main one), windows return to a monitor that comes
  back, and the dock follows the pointer, but there's no way yet to reorder or stack monitors,
  and the panel is on one monitor only. See `ROADMAP.md` Phase 5, "Future updates".
- Screenshots and screen sharing (screencopy, portal), idle screen blanking (DPMS), XWayland,
  fractional scaling, text input / IME. (The desktop shell, middle-click paste and kompact's
  own title bars, once listed here, are done; app activation, notifications and the lock
  screen are Phase 8, see `ROADMAP.md`.)
- Named cursors kompact draws itself: arrow, resize arrows and (8b) text, hand, crosshair, move
  and not-allowed; other names (wait, help, zoom) fall back to the arrow. Animated client
  cursors show their first frame only.

## Building

Needs **rustc 1.86 or newer** for a fresh dependency resolve (calloop 0.14.5,
wayland-protocols 0.32.13). On Ubuntu 24.04 install `rustc-1.89`/`cargo-1.89` from apt and put
`/usr/lib/rust-1.89/bin` on `PATH`.

System dependencies (Debian/Ubuntu names; Alpine in parens):

```
pkg-config libwayland-dev libxkbcommon-dev libinput-dev libudev-dev \
libgbm-dev libdrm-dev libseat-dev libegl1-mesa-dev libgles2-mesa-dev
```
Alpine: `pkgconf wayland-dev libxkbcommon-dev libinput-dev eudev-dev mesa-dev libdrm-dev seatd-dev mesa-gles mesa-egl`

```sh
cargo build --release
cargo test            # pure logic (placement, snapping, resize, focus, workspaces, ...)
```

The release profile is tuned for size (`opt-level = "z"`, LTO, one codegen unit,
`panic = "abort"`, stripped). `cargo build --profile min` is an even smaller variant.

## Running on real hardware (drm backend)

1. The compositor gets GPU and input access from libseat: logind (systemd distros) or `seatd`
   (Alpine: `apk add seatd`, `rc-update add seatd default`, `rc-service seatd start`).
2. Log in on a free TTY (`Ctrl+Alt+F2`, ...). Don't start it from inside another graphical
   session; it would fight over DRM master.
3. Run it with logging to a file:
   ```sh
   RUST_LOG=info ./target/release/kompact --backend drm > ktest.log 2>&1
   ```
4. Start programs from the foot window kompact opens (it has `WAYLAND_DISPLAY` set), e.g. a
   layer-shell panel or `firefox &`.

`KOMPACT_DRM_DEVICE=/dev/dri/cardN` forces a specific GPU instead of libseat's primary pick. If
`XDG_RUNTIME_DIR` is unset, kompact creates a private fallback under `/tmp`.

## Running nested (winit backend)

```sh
./target/release/kompact --backend winit
```
Opens a window on the current display and runs the compositor inside it. In a VM without 3D
acceleration, install Mesa's software GL (Alpine: `apk add mesa-gl mesa-dri-swrast`) or EGL
setup fails.

## Diagnostics

- `Super+D` shows a debug overlay in the top-right corner, updated once a second.
- A `stats:` line (RSS, fds by kind, window counts) is logged every 60 s; set
  `KOMPACT_STATS_SECS=10` for more often, `0` to turn it off.
- On the DRM backend each `stats:` line also has frame timing for the interval: render passes, how
  many were empty (nothing changed), page flips, missed vblanks, and p50/p95/max of compositor CPU
  time per frame (`cpu_ms`), queue-to-flip wait (`wait_ms`) and flip-to-flip time while drawing
  continuously (`frame_ms`, 16.7 = 60 fps), plus input-to-photon latency from a key press
  (`key_ms`) or pointer event (`ptr_ms`) to the flip of the next frame (inputs that changed
  nothing on screen are counted as `no_redraw` instead). Each popup logs a `popup shown:` line
  with its click-to-screen time.
- `tools/perf-baseline.sh ktest.log` summarises that timing per workspace (run one test scene per
  workspace); see `ROADMAP.md` Phase 4 for the baseline numbers.
- `tools/soak.sh` (run from a terminal inside kompact) samples kompact's memory for 30 minutes
  while opening and closing windows, then reports one-off steps and the slow creep a leak would
  cause. `tools/soak.sh --summarize file.csv` re-reads a saved run.

## Input notes

- Keyboard layout comes from xkbcommon's defaults; set `XKB_DEFAULT_LAYOUT` (and
  `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_OPTIONS`) before starting, e.g.
  `XKB_DEFAULT_LAYOUT=de ./kompact`.
- Key repeat: 200 ms delay, 25 per second.
- Some machines expose one physical keyboard/mouse as several input devices; duplicate key and
  button events are filtered out.
- Apps that draw their own cursor images need a cursor theme installed (e.g. Adwaita,
  `XCURSOR_THEME=Adwaita`).

## DRM backend design, and debugging a freeze

The DRM backend (`src/backend_udev.rs`) is deliberately conservative, because a bad KMS state on
the GT 710/nouveau has needed full reboots in the past: it requires an active libseat session,
uses the legacy KMS path when atomic isn't available, keeps cursor and overlay planes disabled
(the cursor is composited in software), disables direct scan-out so every frame is a full render,
and treats a real DRM/page-flip error as fatal (clean stop) rather than retrying the same failing
commit every frame. Changes are made one step at a time and confirmed on hardware before the next
(see `ROADMAP.md`).

If `--backend drm` locks up: Smithay logs `Adding connector` / `Setting new mode` when it *starts*
a KMS commit, before the ioctl that could block. If those lines are missing from the log, the hang
is earlier, in kompact's own frame building, not in the kernel.

Expected, harmless log lines on this setup: `Unable to become drm master, assuming unprivileged
mode` at startup and `Failed to reset crtc ... Permission denied` at exit (logind owns the device
and has already revoked access when kompact tears down).

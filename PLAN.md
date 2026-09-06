# CustoMouse — Research Findings & Implementation Plan

## 1. Purpose

A small, always-available Windows tray app that lets you install, switch, and author
**themed cursor packs** — including fully animated ones (e.g. a Hollow Knight needle,
a Silksong spool) — with sharp rendering at every DPI and cursor size.

The app is a **pack manager + build pipeline**, not a drawing tool. You supply art;
it produces correct, high-quality system cursors and applies them safely.

---

## 2. Research findings that drive the design

### 2.1 Windows format reality (verified empirically, not from docs)

Parsed from `C:\Windows\Cursors` on this Windows 11 machine:

```
aero_busy.ani   556304 bytes
  anih: cFrames=18 cSteps=18 bitcount=32 iDispRate=3 jiffies (50ms = 20fps) flags=1
  frame0 images (w,h,hotx,hoty,bytes):
     (64,64,32,32,16936)  (48,48,24,24,9640)  (32,32,16,16,4264)
```

Conclusions:

| Claim commonly found online | Reality |
|---|---|
| `.ani` cannot store multiple resolutions; ship one `.ani` per size | **False.** Each `.ani` frame is a full ICO directory holding several sizes |
| `_l` / `_xl` are DPI variants | **False.** Same dimensions, different pixels — they are the *accessibility* large-pointer schemes (redrawn bolder) |
| Hotspot is per-file | **False.** Hotspot is stored **per image** in the ICO directory, so it must be rescaled per size |

**Design consequence:** emit **one multi-size file per role** — `.cur` for static,
`.ani` for animated — each embedding **32/48/64/96/128** px. This is simpler than
the per-size-file approach used by most tools *and* sharper.

### 2.2 Answer to "SVG or what format?"

SVG is the right **source** format and the wrong **output** format. Windows has no
vector cursor support; everything must be rasterized into `.cur`/`.ani`.

- **Source:** SVG (ideal — one file, resamples perfectly to every size) or PNG at 128px+.
- **Output:** `.cur` / `.ani` with all five sizes baked in.

Most third-party cursor packs look blurry purely because they ship 32px only and let
Windows upscale. Rendering each size from vector/high-res source is the single biggest
quality win available, and it is nearly free once the pipeline exists.

### 2.3 How cursors are actually applied

- Per-role paths live in `HKCU\Control Panel\Cursors` (`Arrow`, `Wait`, `Hand`, …).
- An **empty string** means "use the system default" — not "no cursor".
  (`Crosshair` and `IBeam` are empty on this machine.)
- The role set is **not fixed**: `Person`/`Pin` exist only on some installs.
  → Enumerate roles from the live registry; never hardcode the list.
- `CursorBaseSize` (px) is the accessibility size: 32, 48, 64 … 256.
- Registry writes alone do nothing. Must call
  `SystemParametersInfo(SPI_SETCURSORS, 0, NULL, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)`.
- Applying is **HKCU-only** — no admin rights, no system files touched.

### 2.4 Prior art

| Project | Takeaway |
|---|---|
| **Cursory** (C, OSS) | Source read directly. Apply path validated ours (see 2.6). Emits **one size per cursor**, so it never hits the `.ani` frame ceiling — and is never sharp at high DPI. No tray, no authoring. |
| **Windows Cursor Switcher** (OSS) | Scheme create/switch; portable. Minimal UI. |
| **Cursor-Changer** (Python, OSS) | Per-active-app cursor switching — nice later feature. |
| **Stardock CursorFX** (paid, market leader) | Wins on: import-your-own-PNG theme editor, live preview, size/color/shadow tweaks without re-authoring, one-click community library, trails/sounds. Loses on: proprietary `.CursorFX` format, custom rendering layer instead of native cursors. |

**What we copy:** the authoring wizard, live preview, one-click apply, size override.
**What we do better:** an open, inspectable pack format, native `.cur`/`.ani` output
(so packs keep working even without our app running), multi-size by default.

### 2.5 Linux

Xcursor is a *better* format than `.ani`: one binary file holds every size **and**
per-frame delays in ms. Themes live at `~/.icons/<theme>/cursors/` with an
`index.theme` plus symlinks for the many aliases (`left_ptr`, `xterm`, `hand2`, …).
Works for both X11 and Wayland.

Because Xcursor is a strict superset of what our pack format expresses, the **same
`pack.json` exports to both platforms**. Shipping a Linux *exporter* on Windows is
cheap; a full Linux build of the app comes later.

### 2.6 What the prior art confirmed, and where we differ

`MurkyYT/Cursory`'s source (`src/main.c`, `src/wincur.c`) was read directly rather
than taken from its README. It independently confirms several details:

| Detail | Cursory | Us | Note |
|---|---|---|---|
| `SPI_SETCURSORS` flags | `SystemParametersInfo(SPI_SETCURSORS, 0, NULL, 0)` | `SPIF_SENDCHANGE` | Both work. **Neither passes `SPIF_UPDATEINIFILE`**, which every tutorial recommends and which fails — see 2.7 |
| Role count | `Cursor g_Cursors[17]` | 17 roles | Same set |
| Scheme registration | `(default)` = name, `Scheme Source` DWORD, `Cursors\Schemes` | identical | Our apply path matches shipping prior art |
| AND-mask stride | `((width + 31) / 32) * 4` | identical | Confirms our DIB layout |
| `anih` flags | `1` (AF_ICON) | identical | |
| Images per cursor | **one** (`wincur_create_ARGB_as_CUR(frame, width, height, …)`) | **multi-size** | The real difference |

Cursory emits a single size per cursor. That is why it never encounters the
per-frame ceiling documented in 2.8 — and why its cursors cannot be sharp above
their authored size. Multi-size output is this project's actual differentiator,
and it is measured, not assumed: embedding a real 128px image raises alpha-edge
energy at 128px from **816 to 3857**, a 4.7x improvement over letting Windows upscale.

### 2.7 `SPIF_UPDATEINIFILE` must not be passed

Measured on Windows 11, calling `SystemParametersInfoW(SPI_SETCURSORS, ...)`:

```text
SPIF_UPDATEINIFILE | SPIF_SENDCHANGE  -> 0, "The handle is invalid" (os error 6)
SPIF_SENDCHANGE                       -> 1, succeeds
SPIF_UPDATEINIFILE                    -> 0, "The handle is invalid"
0                                     -> 1, succeeds   (what Cursory uses)
```

The flag asks Windows to persist the setting to the user profile, which is
meaningless for this action. Nearly every example online gets this wrong.

### 2.8 The `.ani` per-frame ceiling (undocumented)

Microsoft's own Q&A confirms no documented `.ani` size limit exists. There is one,
and it bites as soon as you embed several large sizes per frame. Measured with
`LoadImageW`:

| Frame contents | Bytes/frame | Result |
|---|---|---|
| `[32,48,64]` x 120 frames (3.7 MB file) | 30,894 | loads |
| `[32,48,64,96]` | 68,966 | loads |
| `[32,128]` | 71,926 | **rejected** |
| `[128]` alone, `[256]` alone | 67k–270k | loads |

The ceiling is **per frame, not per file** — a 3.7 MB file loads fine — and
single-image frames are exempt. Smallest multi-image frame observed rejected:
67,110 bytes, so the budget is set at 65,536.

Consequence: **animated** roles are budgeted to 32/48/64 (exactly what Windows'
own animated cursors ship, which is now explicable); **static** `.cur` files are
unaffected and keep all five sizes. This is enforced in code and locked by a test
that fails loudly if a future Windows build moves the boundary.

---

## 3. The pack format ("Template")

A pack is a folder (or a zipped `.cmpack`) containing `pack.json` + art files.

```json
{
  "format": 1,
  "name": "Hollow Knight",
  "author": "you",
  "version": "1.0.0",
  "baseSize": 32,
  "sizes": [32, 48, 64, 96, 128],
  "roles": {
    "Arrow": { "source": "art/needle.svg", "hotspot": [3, 2] },
    "Wait":  { "source": "art/spin/", "fps": 20, "loop": true, "hotspot": [16, 16] },
    "AppStarting": { "source": "art/dream.gif", "hotspot": [3, 2] },
    "IBeam": { "inherit": "system" }
  }
}
```

- `hotspot` is in **baseSize** coordinates and is rescaled for every emitted size.
- `inherit: "system"` writes an empty registry value = Windows default.
- Any role left out simply falls back to the system default.

### Accepted art inputs (auto-detected on drop)

| You drop | Result | Notes |
|---|---|---|
| One `.svg` | Static, all 5 sizes | **Best quality.** Recommended. |
| One `.png` 128px+ | Static, all 5 sizes | Good. Downscale-only, never upscale. |
| Folder of `name_00.png … name_11.png` | Animated | Most common for game rips |
| Sprite sheet `.png` + grid `{cols, rows, frames}` | Animated | Typical Hollow Knight / Unity export |
| `.gif` or animated `.png` | Animated | Auto-split, delays read from the file |

The import wizard detects which of these you gave it, **shows a live animated preview
at real cursor size**, and blocks import with a specific message on problems
(gaps in frame numbering, mismatched frame dimensions, source smaller than 128px,
hotspot outside the canvas). This is the "make it obvious how to upload correctly" goal.

---

## 4. Architecture

```
custo-mouse/
  crates/
    cursorpack/     # pure Rust, no OS deps — the testable core
       manifest.rs      pack.json parse + validate
       raster.rs        svg/png/gif/spritesheet -> RGBA frames at N sizes
       ico.rs           multi-size ICO/CUR directory writer + reader
       ani.rs           RIFF ACON writer + reader (anih / LIST fram / rate / seq)
       xcursor.rs       Linux Xcursor writer
    winapply/       # Windows-only: registry, SPI_SETCURSORS, restore point
       registry.rs      read/write HKCU cursors, enumerate roles
       apply.rs         install pack -> disk + registry -> SPI_SETCURSORS
       restore.rs       snapshot, restore, emit offline .cmd
  app/              # egui/eframe binary: tray, autostart, pack list, preview
  packs/            # bundled demo pack(s)
```

**Stack: `egui` / `eframe` — native Rust GUI, no web layer.**

Rationale:
- **No runtime dependency at all.** One self-contained exe. No WebView2, no .NET, no
  Node. **Measured release build: 7.6 MB binary.**
- **Immediate-mode + GPU rendering is a direct fit for this app.** The live animated
  cursor preview, frame scrubbing and click-to-set-hotspot canvas are the parts that
  would be fiddly in a DOM; in egui they are a per-frame draw call.
- **Cross-platform unchanged** — same code builds for Linux (X11/Wayland) later.
- Rust also makes the binary format work precise and the encoders trivially unit-testable.

**Tray + window.** `tray-icon` crate, created on the first eframe frame so it shares the
winit event loop and message pump. Window starts `with_visible(false)` when launched
with `--tray`, so autostart means *tray icon only, no window*. New tray icons land in
the Win11 overflow ("hidden icons") flyout by default — the requested behaviour, no
extra code. Autostart via the `auto-launch` crate → per-user HKCU `Run` entry, no admin.

**Measured idle footprint, and a correction.** An earlier draft of this plan estimated
~25 MB resident. The real figure is worse, measured on the release build launched with
`--tray` (no window shown):

| | measured |
|---|---|
| binary | 7.6 MB |
| working set, tray-only | ~80 MB |
| private bytes, tray-only | ~115 MB |
| CPU while idle | ~0.05 s per 6 s (~0.8% of one core) |

The memory comes from eframe creating a window and an OpenGL context up front, even
when that window is hidden; the CPU comes from polling the tray channel every 200 ms.
Both are fixable and neither is inherent to egui:

- **Memory**: run the tray on a plain Win32 message loop and only create the eframe
  window when the user actually opens it. Idle would drop to single-digit MB. This is
  the right fix for an app that sits in the tray from boot, and is the main outstanding
  performance item.
- **CPU**: raise the poll interval, or drive tray events from the event loop instead of
  polling.

Still far below Electron (~150 MB binary, ~200 MB resident), but not yet where a
boot-time background app should be.

**Note on the background process:** once the registry is written, cursors persist across
reboots *without* anything running. The resident app earns its keep through instant
pack switching, a hotkey, per-app profiles, and re-applying after games/apps clobber
the cursor — not because the cursor needs babysitting. Worth knowing so you can decide
how much it should do.

---

## 5. Getting your default cursor back (first-class requirement)

Nothing this app does may be hard to undo. Restoring is **purely a registry operation** —
we never touch `C:\Windows\Cursors`, and all generated files live under
`%LOCALAPPDATA%\CustoMouse\`. So restore cannot fail due to a missing or corrupt file.

Seven independent ways back, in order of how broken things are:

1. **Automatic restore point.** Before the *first* apply, the complete
   `HKCU\Control Panel\Cursors` key is snapshotted to `restore-point.json` — every value
   including the empty ones, plus `Scheme Source` and `CursorBaseSize`. This original
   snapshot is written once and never overwritten. A rolling `previous.json` is also kept
   for one-step undo.
2. **Tray menu → "Restore Windows Default".** Top-level item, always visible, one click,
   no confirmation dialog. Not buried in settings.
3. **On quit, it asks.** Quitting from the tray while a pack is active offers
   *Keep cursors* / *Restore default*, with a "remember my choice" checkbox and an
   **Always restore on exit** setting for people who want the app to leave no trace.
4. **Global panic hotkey** (default `Ctrl+Alt+Shift+M`) — restores instantly even if the
   window is unresponsive or off-screen.
5. **`Restore-Default-Cursors.cmd`** written next to the app on install. Plain registry
   commands, no dependency on our exe. Works if the app won't launch, is mid-crash,
   or has already been deleted. This is the real safety net.
6. **Uninstall restores automatically**, then removes the autostart entry.
7. **Windows' own path still works** — because we only ever set standard per-role paths,
   Settings → Bluetooth & devices → Mouse → "Mouse pointer" resets us like any other scheme.

Restore also re-applies `CursorBaseSize` if we changed it, then calls `SPI_SETCURSORS`,
so the change is immediate with no logoff or reboot.

---

## 6. Tests — the ones that actually prove something

Deliberately excluding tests that only assert our own code agrees with itself.

| # | Test | What it would catch |
|---|---|---|
| 1 | **Windows loads our output.** `LoadImageW(path, IMAGE_CURSOR, LR_LOADFROMFILE)` on every generated file; assert a valid `HCURSOR` | A malformed byte anywhere. Windows is the only authority on validity. |
| 2 | **Golden parse of MS cursors.** Parse all 30+ files in `C:\Windows\Cursors`; assert frame counts, sizes, hotspots, rates | Parser drift vs real-world files, including odd ones |
| 3 | **Re-encode round trip.** Decode `aero_busy.ani` → re-encode with our writer → re-decode; assert structurally identical | Writer bugs, using Microsoft files as the reference |
| 4 | **Correct size is chosen on screen.** Set `CursorBaseSize` to 32/48/64/96/128, apply, then `GetCursorInfo` + `GetIconInfo` + `GetObject` on the bitmap; assert the live cursor is that many pixels | The whole multi-size premise. Proves sharpness rather than assuming it. |
| 5 | **Backup/restore is lossless.** Snapshot HKCU cursors → apply pack → restore → assert **byte-identical** to the snapshot | Leaving your system altered. Highest-stakes test here. |
| 6 | **Bad packs fail loudly.** Frame-number gaps, mixed frame sizes, 16px source, hotspot out of bounds, missing file, bad JSON → each yields its own actionable error, never a panic | The import UX, the feature most likely to frustrate |
| 7 | **Animation timing.** 20 fps request → assert `iDispRate == 3` jiffies and total duration within one jiffy of target | Silent timing drift from the 1/60 s quantisation |
| 8 | **Hotspot scales.** hotspot (3,2) at base 32 → assert (6,4)@64, (12,8)@128 in the emitted directories | Cursor clicking in the wrong place at non-default sizes |
| 9 | **The offline restore script works.** Run `Restore-Default-Cursors.cmd` as a subprocess against an applied pack; assert the registry matches the original snapshot byte-for-byte | The safety net silently not being a safety net |
| 10 | **Restore survives a deleted pack.** Apply, delete `%LOCALAPPDATA%\CustoMouse\` entirely, then restore; assert success | Restore depending on files that may be gone |
| 11 | **Restore-on-exit actually runs.** Launch with a pack applied and `restore_on_exit=true`, signal quit, wait for exit; assert cursors are back | The quit path being skipped or racing shutdown |

Tests 1, 4, 5, 9, 10 and 11 are Windows-integration tests and run against the real OS.
They snapshot the registry first and always restore it, so running the suite cannot
leave your cursors altered.

---

## 7. Build order

| Phase | Deliverable | Testable at end? |
|---|---|---|
| **1** | `cursorpack` crate: manifest, raster, ICO/CUR, ANI + tests 2,3,6,7,8 | CLI: build a pack, inspect output |
| **2** | `winapply`: registry, SPI_SETCURSORS, restore point, offline .cmd + tests 1,4,5,9,10 | **Yes — real cursors on screen** |
| **3** | egui app: tray, autostart, pack list, one-click apply, restore-on-exit + test 11 | **Yes — the actual app (v1 stop point)** |
| **4** | Import wizard: drag-drop, auto-detect, live preview, click-to-set hotspot, validation | **Yes — author a pack end to end** |
| **5** | Linux Xcursor export + `index.theme` + alias symlinks | Export, verify on Linux |
| *later* | Per-app profiles, hotkeys, trails, pack sharing | — |

Phase 2 is the first point worth stopping to test hands-on.

---

## 8. Bundled art

No third-party game assets ship in this repo — game sprites are copyrighted, and
importing your own is the supported path.

The bundled pack is **Inverted Default**: this machine's own Windows pointers with
their colours inverted. Static pointers are re-rendered from the Windows SVG
sources (`C:\Windows\Cursors\*.svg`) at all seven sizes, so they are *sharper*
than the stock bitmaps; the two spinners are the `aero_*.ani` files with their
pixels inverted, preserving frame count and 50ms timing. It is reproducible with
`cargo run -p cursorpack --example gen_inverted -- packs/inverted-default`, and a
test verifies the output really is the pixel-inverse of the Windows source.

**Worth deciding before publishing:** that pack is a derivative of Microsoft's
cursor artwork and weighs ~9 MB. Generating it on first run from each user's own
`C:\Windows\Cursors` would avoid redistributing it and would match whatever
Windows version they run. The generator already supports that; only the decision
is outstanding.

### Known gap: Xcursor (Linux) themes

Confirmed against Bibata, one of the most widely used cursor themes: an Xcursor
theme is a folder of extension-less files with the `Xcur` signature (chunk type
`0xfffd0002` per image, carrying size, hotspot, delay and ARGB pixels), named both
by hash alias and by human-readable name (`left_ptr`, `xterm`, `watch`, …). The
importer currently rejects these outright. Cursory supports the conversion, so
this is a real competitive gap — and the reader is the mirror image of the
Xcursor *writer* already planned for Linux export.

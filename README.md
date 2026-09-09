# CustoMouse

A Windows tray app for installing, switching, and authoring themed cursor packs, including fully animated ones, with sharp rendering at every pointer size Windows supports.

CustoMouse is a **pack manager and build pipeline**, not a drawing tool. You supply artwork (SVG, PNG, GIF, or a sprite sheet), and it rasterizes, encodes, and applies correct `.cur`/`.ani` files for every role in the Windows pointer scheme.

## Why

Most third-party cursor packs look blurry because they ship a single 32px bitmap and let Windows stretch it for every other size. CustoMouse renders each size directly from the source art, vector when available, so pointers stay crisp from 32px up through the large accessibility sizes.

## Features

- **Multi-size `.cur`/`.ani` encoding**: every role embeds all standard sizes (32/48/64/96/128 and up) in one file, verified byte-for-byte against Windows' own stock cursors.
- **Animated cursors**: `.ani` sources with per-frame timing, respecting Windows' per-frame byte limits.
- **Ready-made upscaling**: for packs whose source art only has small bitmaps, a per-role `upscale` flag synthesizes the missing larger sizes with a premultiplied Lanczos resample, sharper than Windows' on-the-fly stretch.
- **Safe apply/restore**: writes to `HKCU\Control Panel\Cursors`, calls `SystemParametersInfo` to make Windows pick up the change immediately, and can restore the previous scheme (including offline, via a generated restore script).
- **Tray application**: gallery of installed packs, per-pointer previews, import, delete, and autostart, built with `egui`.

## Project layout

```
crates/
  cursorpack/   pack manifest, rasterization (SVG/PNG/GIF/sprite-sheets), .cur/.ani encoding
  winapply/     registry apply/restore, offline restore script, autostart
app/            egui tray application (gallery, import, delete, settings)
packs/          bundled cursor packs (e.g. "Inverted Default")
```

`cursorpack` and `winapply` are plain Rust libraries with no UI dependencies, so the encoding and registry logic can be tested and reused independently of the tray app.

## Pack format

A pack is a directory with a `pack.json` manifest describing its name, the pointer sizes it supports, and a source file per Windows cursor role (`Arrow`, `Hand`, `Wait`, `AppStarting`, and so on):

```json
{
  "name": "Inverted Default",
  "base_size": 32,
  "sizes": [32, 48, 64, 96, 128, 192, 256],
  "roles": {
    "Arrow": { "source": "art/Arrow.cur", "fps": 20.0, "loop": true }
  }
}
```

Sources can be static (`.cur`, `.png`, `.svg`) or animated (`.ani`, `.gif`, sprite sheets). CustoMouse rasterizes each declared size from the source and packs the result into the Windows binary formats.

## Building

Requires a recent stable Rust toolchain (edition 2021, Rust 1.82+) and Windows for the registry-apply and tray functionality.

```bash
cargo build --release
```

The workspace has three members: `crates/cursorpack`, `crates/winapply`, and `app` (binary name `custo-mouse`).

## Running

```bash
cargo run -p custo-mouse
```

This starts the tray app, which lists installed packs from the `packs/` directory, lets you preview and apply one, and offers an "Upscale the low-res pointer(s)" action when a pack's source art is too small for its declared sizes.

## Testing

```bash
cargo test --workspace
```

The suite includes Windows-integration tests that apply and restore the real registry and confirm every generated `.cur`/`.ani` file loads via `LoadImageW`.

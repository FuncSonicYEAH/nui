# nui

A Qt Quick-style UI framework for Rust. UIs are described in a standalone
declarative language (`.nui`), the engine is written in Rust, and rendering
goes through wgpu.

[![CI](https://github.com/FuncSonicYEAH/nui/actions/workflows/ci.yml/badge.svg)](https://github.com/FuncSonicYEAH/nui/actions/workflows/ci.yml)

[English](README.md) | [简体中文](readme/README.zh-CN.md) | [繁體中文](readme/README.zh-TW.md)

**Status: pre-release.** Nothing is published to crates.io yet, and both the
language and the Rust APIs will change without notice.

## Highlights

- **Runtime-compiled documents with first-class hot reload.** Slint resolves
  its markup at compile time with code generation; Makepad ships its own live
  DSL. nui takes a third position: editing a `.nui` file never recompiles your
  Rust program. `nui-preview` recompiles the document and rebuilds the element
  tree; if the edit does not compile, the window keeps showing the last good
  frame with the diagnostic overlaid on it.
- **Explicit data binding.** Three operators define how properties get values:
  `p = v` (static, written once), `p <- expr` (reactive, re-evaluated when
  dependencies change), `p <=> q` (two-way). Unlike QML, where every
  `name: value` is a binding and reassignment silently destroys it, the
  semantics are visible at the call site and checkable statically.
- **State machines are nodes**, not a layer on conditional styling; animation
  is expressed inline on the binding (`tween` / `spring`); sizes default to
  `dp`, so the same document holds across displays.

A complete document — this is `examples/counter.nui`, unedited:

```qml
component Counter {
    property count: Int = 0

    Window(id = root) {
        Column(id = content, spacing = 12dp, padding = 24dp) {
            Rectangle(id = panel, fill = #336699, radius = 12dp, opacity = 0.25)
            Text(id = label, content <- "已点击 {count} 次", width = 200dp, height = 40dp)
            Button(id = increment, label = "+1", width = 120dp, height = 44dp) {
                on click => count += 1
            }
        }
    }
}
```

## Getting started

Requires a recent stable Rust toolchain (edition 2024). A software GPU adapter
works: lavapipe or llvmpipe on Linux, WARP on Windows.

```sh
cargo run -p nui --example counter     # minimal counter
cargo run -p nui --example showcase    # scroll, group opacity, blur
cargo run -p nui --example playground  # everything at once

# offscreen mode: no window needed, writes PNGs to snapshots/
cargo run -p nui --example playground --release -- --snap

# hot reload: edit the .nui file while it runs
cargo run -p nui-preview -- examples/counter.nui

# fixed-size window (no user resizing; best-effort on Wayland)
cargo run -p nui-preview -- examples/counter.nui --no-resize
```

`NUI_BACKEND` forces a wgpu backend: `vulkan`, `gl`, `dx12`, `metal` or
`primary` (e.g. `NUI_BACKEND=gl cargo run -p nui --example counter`).

## Documentation

`plan.md` (written in Chinese) is the design document and the most detailed
source of truth in this repository: decision log, language semantics,
architecture, and milestone acceptance criteria.

## License

Dual-licensed under either of Apache License, Version 2.0 (`LICENSE-APACHE`)
or MIT (`LICENSE-MIT`) at your option. Contributions are dual-licensed the
same way unless stated otherwise.

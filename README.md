# nui

A Qt Quick-style UI framework for Rust. The interface is described in a
standalone declarative language (`.nui`), the engine is written in Rust, and
rendering goes through wgpu.

[![CI](https://github.com/FuncSonicYEAH/nui/actions/workflows/ci.yml/badge.svg)](https://github.com/FuncSonicYEAH/nui/actions/workflows/ci.yml)

**Status: pre-release.** Milestones M0 through M10.5 of `plan.md` are complete
and the test suite passes locally. M11 (cross-platform compile and smoke tests)
is in progress. Nothing is published to crates.io yet, and both the language and
the Rust APIs will change without notice.

## Why another Rust UI framework

Slint resolves its markup at compile time with code generation. Makepad ships
its own live DSL. nui takes a third position: **the document is compiled at
runtime, and hot reload is a first-class feature.** Editing a `.nui` file does
not recompile your Rust program. `nui-preview` recompiles the document and
rebuilds the element tree; if the edit does not compile, the window keeps
showing the last good frame with the diagnostic overlaid on it.

## The language

`nui-lang` is strongly typed and has no scripting layer. Expression bindings
cover the common cases, and anything that does not fit belongs in Rust, exposed
to the document through a registered host function or component.

Three data operators are the core of the language's identity:

| Operator | Meaning |
| --- | --- |
| `p = v` | Static assignment. Written once, never recomputed. |
| `p <- expr` | Reactive binding. Dependencies are tracked dynamically and the expression is re-evaluated when they change. |
| `p <=> q` | Two-way binding. A write to either side propagates to the other. |

In QML, `name: value` is always a binding, and assigning the same property from
a handler silently destroys it, which is the most common source of bugs in QML
code. nui makes the distinction explicit, so it is visible at the call site and
checkable statically.

A complete document. This is `examples/counter.nui`, unedited:

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

State machines are a first-class node rather than a layer on top of conditional
styling, and conditional property blocks are plain syntax. This document is
taken from the runtime test suite:

```qml
component Counter {
    property count: Int = 0
    property limit: Int = 3
    property resetPressed: Bool = false
    signal reset
    signal bump

    machine mode {
        state idle
        state overflow {
            enter => resetPressed = true
            exit => resetPressed = false
        }
        on bump from idle when count < limit => overflow
        on reset from overflow => idle
    }

    Window(id = root) {
        Text(id = label, content <- "count: {count}")
        Text(id = flag, flagged <- mode.overflow)
        when mode.overflow {
            label.opacity = 0.5
        }
    }
}
```

Animation is expressed inline on the binding instead of wrapping the node:

```qml
width   <- tween(80dp + progress * 6dp, duration = 300ms, easing = ease-out)
opacity <- spring(target.opacity, stiffness = 120, damping = 14)
```

Sizes default to `dp` (density-independent pixels) and are converted to physical
pixels only when the renderer submits, so the same document holds across
displays. Literals cover `420dp`, `50%`, `auto`, `200ms`, `#336699` and enum
values such as `bold` or `ease-out`.

## What works today

| Area | State |
| --- | --- |
| Compiler | Call-style syntax, strong typing, scope and `id` resolution, static binding-cycle detection, rustc-style span diagnostics, expression bytecode, Document IR |
| Runtime | Element tree, explicit binding operators, D10 semantics, signals, state machines with guards and enter/exit effects, timers, animation, property-change notifications |
| Layout | taffy flex via `Column` / `Row` / `padding` / `spacing`, `dp` / `%` / `auto` sizes, geometry write-back readable from bindings, text intrinsic size |
| Text | cosmic-text shaping with a glyph atlas, single line, embedded DejaVu Sans as an offline fallback |
| Rendering | Rounded-rect SDF and borders, glyph quads, async-decoded images with tint and nine-slice, soft SDF shadows, offscreen clip / blur / opacity layers |
| Input | Hit testing that accounts for scroll offsets, focus chain and Tab navigation, `TextInput` with caret, selection and clipboard, IME preedit and commit |
| Data | `Model` protocol with `VecModel`, `For` rows, virtualized `ListView` |
| Tooling | `nui-preview` hot reload, `include_ui!` compile-time validation and embedding, offscreen PNG snapshot mode |

## Crates

| Crate | Role |
| --- | --- |
| `nui-core` | Values, geometry, color, length, duration, normalized events, typed errors. No dependencies. |
| `nui-syntax` | Lexer, parser, AST, span diagnostics. |
| `nui-compiler` | Type checking, scope resolution, binding-cycle detection, Document IR, expression bytecode. |
| `nui-runtime` | Element tree, binding engine, signals, state machines, animation, focus, Model protocol, host registry. |
| `nui-layout` | taffy adapter, `dp` conversion, geometry write-back. |
| `nui-text` | cosmic-text shaping and the glyph atlas. |
| `nui-render` | wgpu scene graph with rect, text, image and compositing pipelines. |
| `nui-winit` | Windows, input, IME and clipboard. The only place platform differences are allowed to appear. |
| `nui-macros` | `include_ui!`. |
| `nui` | Facade: `Application`, `Window`, run loop and the built-in component set. |
| `nui-preview` | Hot-reload previewer binary. |

## Building and running

Requires a recent stable Rust toolchain (edition 2024) and a working GPU
adapter. Software adapters work: lavapipe or llvmpipe on Linux, WARP on Windows.

```sh
cargo run -p nui --example counter      # minimal counter, M3
cargo run -p nui --example todo         # For rows over a host VecModel, M4
cargo run -p nui --example form         # two-way bindings and IME, M7
cargo run -p nui --example gallery      # images, nine-slice and shadows, M8
cargo run -p nui --example showcase     # scroll, group opacity, blur, M9
cargo run -p nui --example biglist      # 10,000-row virtualized list, M10
cargo run -p nui --example playground   # everything at once
```

`playground` also has an offscreen mode that needs no window and writes PNGs to
`snapshots/`, which is useful on machines without a display server:

```sh
cargo run -p nui --example playground --release -- --snap
```

Hot reload:

```sh
cargo run -p nui-preview -- examples/counter.nui --width 420 --height 300
```

Edit the `.nui` file while it runs and the window updates. If the document does
not compile, the window keeps the last good frame and overlays the diagnostic.

GPU backend selection. The engine tries the default backends first and falls
back to OpenGL with a display handle, which is what non-functional Vulkan
drivers need. `NUI_BACKEND` forces one path:

```sh
NUI_BACKEND=gl cargo run -p nui --example counter
```

Accepted values are `vulkan`, `gl`, `dx12`, `metal` and `primary`.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The suite covers parser snapshots, headless runtime integration tests that run
the full pipeline without a window, hit testing, and offscreen pixel-readback
tests for the rect, text, image, shadow and layer pipelines. The offscreen tests
need any usable wgpu adapter, so they also run on CI with a software renderer.

CI additionally enforces that `#[cfg(target_os)]` appears nowhere outside
`crates/nui-winit`, so platform differences cannot leak into the engine.

## Roadmap

| Milestone | Content |
| --- | --- |
| M11 | Cross-platform compile and smoke tests on Windows, macOS and Linux |
| M12 | AccessKit accessibility tree, LSP completion and diagnostics for `.nui` |
| M13 | Native integration (file dialogs, menus, tray) and packaging for all three desktops |

## Known limitations

These are deliberate v1 boundaries, not oversights.

- Text is single line: no wrapping, no bidi, no emoji.
- `Image` needs an explicit size because decoding is asynchronous; mipmaps are
  not generated and textures are never evicted.
- Layer content is clipped to the element rect, layer draw order is fixed after
  rects, images and texts, and wheel scrolling only clamps to zero.
- `ListView` assumes a uniform row height and has no scrollbar.
- The caret does not blink, and some IMEs may deliver character keys alongside
  preedit.
- The engine is single-threaded. Only image decoding and the file watcher run
  off-thread.
- Desktop only. wasm and mobile are not attempted yet, though the architecture
  leaves room for them.

## Documentation

`plan.md` is the design document: decision log, language semantics, architecture,
milestone acceptance criteria, and an engineering log of the bugs found while
building each milestone. It is written in Chinese and is the most detailed
source of truth in this repository.

## License

Dual-licensed under either of

- Apache License, Version 2.0 (`LICENSE-APACHE`)
- MIT license (`LICENSE-MIT`)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual-licensed as above, without any additional terms or conditions.

# nui

一个面向 Rust 的 Qt Quick 风格 UI 框架。界面用独立的声明式语言（`.nui`）描述，
引擎由 Rust 编写，渲染走 wgpu。

[![CI](https://github.com/FuncSonicYEAH/nui/actions/workflows/ci.yml/badge.svg)](https://github.com/FuncSonicYEAH/nui/actions/workflows/ci.yml)

[English](../README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md)

**状态：预发布。** 尚未发布到 crates.io，语言和 Rust API 均可能随时变更，恕不另行通知。

## 特性

- **文档在运行时编译，热重载是一等公民。** Slint 在编译期解析标记并生成代码；
  Makepad 自带 live DSL。nui 采取第三条路线：修改 `.nui` 文件不会重新编译你的
  Rust 程序。`nui-preview` 会重新编译文档并重建元素树；如果修改无法编译，窗口
  会保留最后一帧正常画面，并把诊断信息叠加在上面。
- **显式数据绑定。** 三个运算符决定属性如何取值：`p = v`（静态，只写一次）、
  `p <- expr`（响应式，依赖变化时重新求值）、`p <=> q`（双向）。QML 中任何
  `name: value` 都是绑定、重新赋值会悄悄销毁绑定；nui 把语义直接写在调用处，
  可见且可静态检查。
- **状态机是节点本身**，而不是条件样式之上的一层；动画直接内联写在绑定上
  （`tween` / `spring`）；尺寸默认 `dp`，同一文档可跨显示器使用。

一份完整的文档 —— 原样取自 `examples/counter.nui`：

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

## 快速开始

需要较新的 stable Rust 工具链（edition 2024）。软件 GPU 适配器即可运行：
Linux 上的 lavapipe 或 llvmpipe，Windows 上的 WARP。

```sh
cargo run -p nui --example gallery  # 全部功能，一个带侧边栏的窗口

# 离屏模式：无需窗口，每页一张 PNG 输出到 snapshots/
cargo run -p nui --example gallery --release -- --snap

# 热重载：运行时直接编辑 .nui 文件
cargo run -p nui-preview -- examples/counter.nui
```

`NUI_BACKEND` 可强制指定 wgpu 后端：`vulkan`、`gl`、`dx12`、`metal` 或
`primary`（例如 `NUI_BACKEND=gl cargo run -p nui --example gallery`）。

## 文档

`plan.md`（中文撰写）是本仓库的设计文档，也是最详尽的事实来源：决策记录、
语言语义、架构，以及各里程碑的验收标准。

## 许可证

在 Apache License, Version 2.0（`LICENSE-APACHE`）或 MIT（`LICENSE-MIT`）下
双重许可，任选其一。除非另有说明，贡献内容按相同方式双重许可。

# nui

一個面向 Rust 的 Qt Quick 風格 UI 框架。介面以獨立的宣告式語言（`.nui`）描述，
引擎由 Rust 撰寫，算繪走 wgpu。

[![CI](https://github.com/FuncSonicYEAH/nui/actions/workflows/ci.yml/badge.svg)](https://github.com/FuncSonicYEAH/nui/actions/workflows/ci.yml)

[English](../README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md)

**狀態：預發布。** 尚未發布到 crates.io，語言和 Rust API 均可能隨時變更，恕不另行通知。

## 特性

- **文件在執行時編譯，熱重載是一等公民。** Slint 在編譯期解析標記並產生程式碼；
  Makepad 自帶 live DSL。nui 採取第三條路線：修改 `.nui` 檔案不會重新編譯你的
  Rust 程式。`nui-preview` 會重新編譯文件並重建元素樹；如果修改無法編譯，視窗
  會保留最後一幀正常畫面，並把診斷資訊疊加在上面。
- **顯式資料繫結。** 三個運算子決定屬性如何取值：`p = v`（靜態，只寫一次）、
  `p <- expr`（響應式，依賴變化時重新求值）、`p <=> q`（雙向）。QML 中任何
  `name: value` 都是繫結、重新賦值會悄悄銷毀繫結；nui 把語義直接寫在呼叫處，
  可見且可靜態檢查。
- **狀態機是節點本身**，而不是條件樣式之上的一層；動畫直接內聯寫在繫結上
  （`tween` / `spring`）；尺寸預設 `dp`，同一文件可跨顯示器使用。

一份完整的文件 —— 原樣取自 `examples/counter.nui`：

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

## 快速開始

需要較新的 stable Rust 工具鏈（edition 2024）。軟體 GPU 介面卡即可執行：
Linux 上的 lavapipe 或 llvmpipe，Windows 上的 WARP。

```sh
cargo run -p nui --example gallery  # 全部功能，一個帶側邊欄的視窗

# 離屏模式：無需視窗，每頁一張 PNG 輸出到 snapshots/
cargo run -p nui --example gallery --release -- --snap

# 熱重載：執行時直接編輯 .nui 檔案
cargo run -p nui-preview -- examples/counter.nui
```

`NUI_BACKEND` 可強制指定 wgpu 後端：`vulkan`、`gl`、`dx12`、`metal` 或
`primary`（例如 `NUI_BACKEND=gl cargo run -p nui --example gallery`）。

## 文件

`plan.md`（中文撰寫）是本儲存庫的設計文件，也是最詳盡的事實來源：決策記錄、
語言語義、架構，以及各里程碑的驗收標準。

## 授權條款

在 Apache License, Version 2.0（`LICENSE-APACHE`）或 MIT（`LICENSE-MIT`）下
雙重授權，任選其一。除非另有說明，貢獻內容按相同方式雙重授權。

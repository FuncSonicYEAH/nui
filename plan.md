# nui — Qt Quick 式 Rust UI 框架 · 项目规划

> **状态**：规划定稿（2026-09-19），workspace 骨架已建，**未开工**。
> **工作名**：`nui`（node ui，可改）；前端语言暂名 **nui-lang**，文件扩展名 `.nui`。
> **一句话定位**：Qt Quick 的 Rust 重制版——UI 用独立的声明式语言描述（节点树 + 显式响应式绑定），引擎与内置组件用 Rust 实现，渲染走 wgpu。

**差异化锚点**：Slint 走编译期 codegen，Makepad 走自家 live DSL；我们选**运行时加载的编译产物 + 热重载为一等公民**——改 `.nui` 不重编译 Rust 程序，开发体验对标 QML，发布时用 `include_ui!` 宏兜底。

---

## 1. 决策日志（已拍板）

| # | 决策点 | 结论 | 日期 |
|---|---|---|---|
| D1 | 执行模型 | 运行时加载 Document IR + VM 解释（热重载优先）；远期 `include_ui!` 编译期嵌入兜底，**不做** Slint 式 codegen | 2026-09-19 |
| D2 | 语法外形 | **调用风格**：`Node(prop = v) { .. }` 构造调用 + 子节点块 | 2026-09-19 |
| D3 | 响应性 | 显式三运算符：`=` 静态 / `<-` 响应式 / `<=>` 双向 | 2026-09-19 |
| D4 | 脚本 | **无 JS**：纯表达式 + 受限效果块（let / if-else / 属性赋值 / 函数调用 / emit），复杂逻辑下沉 Rust | 2026-09-19 |
| D5 | 状态管理 | 一等状态机 `machine`（状态/迁移/守卫/enter-exit）+ 通用 `when` 条件属性块 | 2026-09-19 |
| D6 | 类型系统 | 强类型（Int/Float/Bool/String/Length/Color/Duration/枚举/组件引用），编译期检查 | 2026-09-19 |
| D7 | 单位 | 默认 **dp**（密度无关像素），`420dp / 50% / auto` | 2026-09-19 |
| D8 | 动画 | 绑定行内包装器 `tween(..)` / `spring(..)`，替代 `Behavior on` | 2026-09-19 |
| D9 | 布局 | taffy（flex/grid）优先；QML 式 anchors 远期适配层 | 2026-09-19 |
| D10 | 赋值语义 | 效果块中的赋值**清除该属性上的 `<-` 绑定**（沿用 QML，因运算符显式而可解释；文档明示） | 2026-09-19 |
| D11 | 平台 | v1 仅三大桌面（Windows / macOS / Linux）；wasm/移动端架构预留、不进验收 | 2026-09-19 |
| D12 | 文本 | 全押 cosmic-text，不自研 shaping | 2026-09-19 |
| D13 | 平台隔离 | `#[cfg(target_os)]` 仅允许出现在 `nui-winit` 与打包层，CI 做 grep lint | 2026-09-19 |
| D14 | 渲染路线 | Qt Quick 式场景图 + 4 实例化管线（rect / text / image / 后效） | 2026-09-19 |
| D15 | lint 配置 | 显式 return 风格：`implicit_return = "warn"` 且 `needless_return = "allow"`（两 lint 互斥，以 rust-quality §2 为准） | 2026-09-19 |

**节点思想**（设计基座）：一切皆节点——可视节点（Rectangle/Text/Column…）、逻辑节点（Timer/State/Model，不绘制但参与树与绑定）、资源节点（Font/Image）。属性绑定构成数据流 DAG，引擎 = 节点树 + 响应式依赖图 + 每帧脏传播管线（绑定 → 布局 → 绘制）。

## 2. 总体架构

```
┌────────────────────────────────────────────────────┐
│ 宿主 App（Rust）：注册组件/模型/函数，持有窗口句柄      │
├────────────────────────────────────────────────────┤
│ DSL 层                                              │
│   *.nui 源码 → nui-syntax（词法/语法/AST + span诊断） │
│             → nui-compiler（类型检查/作用域解析/      │
│                表达式字节码 → Document IR → bundle）  │
├────────────────────────────────────────────────────┤
│ 运行时 nui-runtime                                  │
│   元素树 · 属性系统 · 绑定/依赖图 · 信号槽 · 状态机    │
│   布局 · 动画时钟 · 焦点/IME · Model 协议            │
├────────────────────────────────────────────────────┤
│ 渲染 nui-render                                     │
│   渲染树 · 分桶批处理 · rect/text/image 管线         │
│   字形图集 · 纹理缓存 · 后期特效                     │
├────────────────────────────────────────────────────┤
│ 平台层 nui-winit（窗口/输入/IME）+ wgpu 30 后端链     │
└────────────────────────────────────────────────────┘
```

每帧数据流：**输入事件 → 绑定/动画求值（脏集）→ 布局（脏子树）→ 构建渲染树 → wgpu 提交**。无脏数据且无活动动画时不请求重绘（省电）。

## 3. 前端语言 nui-lang 设计

### 3.1 定稿语法（调用风格）

```qml
// counter.nui —— 文件即组件，文件名 = 组件名
component Counter {
    property count: Int = 0          // 强类型属性；= 为静态默认值
    signal resetRequested            // 对外 API 显式声明

    Window(id = root, width = 420dp, height = 300dp) {
        title <- "已点击 {count} 次"   // 块内也可写属性

        Column(spacing = 8dp, padding = 16dp) {
            Text(
                content <- "计数: {count}",   // <- 响应式绑定
                font.size = 20dp,
                font.weight = bold,
            )

            Button(label = "+1", enabled <- count < 10) {
                onClick => count += 1
            }

            TextField(placeholder = "名字", text <=> root.userName)  // <=> 双向

            For(item in root.items) {
                Row(key = item.id) {
                    Text(content <- item.label)
                }
            }
        }

        when count >= 10 {            // 条件属性块（视觉状态）
            Column.opacity = 0.6
        }
    }
}
```

**三个数据运算符**（语言的身份标识）：

| 运算符 | 语义 |
|---|---|
| `p = v` | 静态赋值，一次写入，永不随依赖变化 |
| `p <- expr` | 响应式绑定，依赖动态追踪，依赖变化自动重算 |
| `p <=> q` | 双向绑定，任一侧写入同步另一侧 |

**状态机与处理器**：

```qml
machine playback {
    state stopped
    state playing {
        enter => timer.start()
        exit  => timer.stop()
    }

    on play  from stopped, paused => playing
    on pause from playing         => paused
    on stop  from playing, paused when canStop => stopped
}

// 效果块：只有 let / if-else / 属性赋值 / 函数调用 / 发信号，无循环无闭包
on countChanged => {
    if count >= limit {
        emit resetRequested
    }
}
```

**动画内联在绑定上**：

```qml
opacity <- tween(target, duration = 200ms, easing = ease-out)
x       <- spring(target.x, stiffness = 120, damping = 14)
```

### 3.2 与 QML 的差异对照（本设计的核心卖点）

| 维度 | QML | nui-lang |
|---|---|---|
| 响应性 | 冒号即绑定（隐式），事件里赋值**悄悄杀死绑定**，头号坑 | 显式 `=` / `<-` / `<=>`，读写语义一眼可辨，工具可静态检查 |
| 脚本 | 内嵌完整 JS，图灵完备，逻辑散落 UI 里 | 无脚本：纯表达式 + 受限效果块 |
| 状态管理 | `states` + `when` + `Transitions` 三件套补丁 | 一等状态机 `machine` + 通用 `when` 属性块 |
| 类型 | 弱类型 `var` + JS 隐式转换，错误晚到运行时 | 强类型，编译期检查运算符与类型匹配 |
| 单位 | 默认 px，跨 DPI 自己操心 | 默认 dp，跨平台一致 |
| 字符串 | 靠 `+` 拼接 | 内建插值 `"已点击 {count} 次"` |
| 动画 | `Behavior on x {}` 包装节点 | 绑定行内 `tween(...)` / `spring(...)` |
| 组件声明 | 文件隐式为组件 | `component Name` 显式声明，`property/signal` 构成显式 API 面 |
| 属性书写 | 只能 `name: value` | 构造参数（信息密度高）与块内书写（多属性对齐）双模式 |

### 3.3 语义规则

- 绑定表达式必须**纯**（无副作用）；副作用只允许写在 `onXxx` 效果块里。
- 效果块赋值清除该属性的 `<-` 绑定（D10）；`<=>` 属性写入走值通道。
- `id` 是伪属性，作用域为当前组件子树；`root` / `parent` 为内置引用。
- 附加属性（`font.size`）由宿主类型声明，编译期校验。
- `when` 块是属性组语法糖，编译为条件绑定；状态机的状态值可被 `when` 键控。
- 依赖图成环：编译期静态检查 + 运行时求值深度上限双保险。
- 字面量类型：`420dp`、`50%`、`auto`、`200ms`、`#336699`、`bold`（枚举变体）。

### 3.4 编译与执行模型

```
*.nui 源码
  → nui-syntax:   lexer / parser / AST + span 诊断
  → nui-compiler: 类型检查 → 作用域与 id 解析 → 绑定环静态检测
                  → 表达式编译为字节码 → Document IR（类型化组件定义集合）
                  → （发布可选）bundle 序列化
  → nui-runtime:  实例化元素树 → 统一解析 id → 统一求值绑定 → 运行
```

热重载：文件监视 → 重新编译 Document → v1 全量重建元素树（丢状态），v2 按 `id`/路径 diff 保留状态。

## 4. Workspace 结构

```
nui/
├── Cargo.toml            # workspace：成员、共享依赖、统一 lints（unsafe forbid 等）
├── plan.md               # 本文件
├── crates/
│   ├── nui/              # 门面：Application/Window/运行循环 + 内置组件集（用户唯一入口）
│   ├── nui-core/         # Value/Length/Color/Duration/几何/事件/typed error（零依赖）
│   ├── nui-syntax/       # lexer/parser/AST/span 诊断
│   ├── nui-compiler/     # 类型检查/作用域/Document IR/字节码/bundle
│   ├── nui-runtime/      # 元素树/属性/绑定/信号/状态机/动画/焦点/Model/Registry
│   ├── nui-layout/       # taffy 适配层 + dp 换算 + 几何回写
│   ├── nui-text/         # cosmic-text shaping + 字形图集
│   ├── nui-render/       # wgpu 渲染树 + rect/text/image 管线 + 纹理缓存
│   ├── nui-winit/        # 窗口/输入/IME/剪贴板（平台差异唯一隔离点）
│   └── nui-macros/       # include_ui!（proc-macro）
└── tools/
    └── nui-preview/      # 热重载预览器（bin）
```

crate 间路径依赖已在骨架 Cargo.toml 中连好；统一 lints（`unsafe_code = forbid`、clippy `implicit_return`/`unwrap_used`）已按 rust-quality 规范配置在 `[workspace.lints]`。

## 5. 运行时引擎关键设计

- **元素树**：slotmap 世代 arena，`ElementIndex` 句柄；实例化按文档顺序创建，`id` 构造完成后统一解析、再统一求值绑定（避免 QML 初始化顺序坑）。
- **属性系统**：组件类型带 `ComponentDesc`（属性描述符表：名字/类型/读写器），实例侧紧凑属性存储；`Value` 枚举（f64/int/bool/string/color/length/brush/enum/model/element-ref）。
- **绑定引擎**：thread-local「当前求值栈」做动态依赖追踪；变更传播两阶段——沿依赖边标记 dirty，帧首按拓扑序求值全部 dirty 绑定；求值重入即判定为环，报诊断错误。
- **信号/事件**：信号表 + 连接；handler 两种来源（DSL 效果块字节码 / Rust 闭包）。
- **状态机**：信号驱动机器实例；状态值暴露为可绑定枚举；enter/exit 动作在迁移时执行。
- **动画**：属性存「绑定目标值 + 动画当前值」两层，渲染读动画层；`tween/spring` 包装器拦截目标值变化，从当前显示值插值。
- **布局**：taffy 承担 flex/grid；`Column/Row/padding/spacing` 映射 flex 样式；布局结果回写 `x/y/width/height` 供绑定；绑定依赖布局结果时迭代上限做环路保护。
- **输入**：winit 事件 → 归一化 → 命中测试（变换矩阵栈）→ capture/grab → bubble；键盘焦点链 + Tab 导航 + IME；`TextInput` 在 M7（难点 IME + 光标/选区）。
- **宿主互操作**：`Registry` 注册自定义组件（trait + 描述符，后续 derive 宏）、注册可从表达式调用的函数、`Model` trait（行数/取行/变更通知，内置 `VecModel` 自动 diff）。跨线程更新走 event-loop proxy 合并到 UI 线程；整体单线程（图片解码等后台线程）。

## 6. wgpu 渲染设计

- **四个管线，全部实例化批处理**：
  1. **Rect**（承担 ~80% UI）：每实例一个四边形，片元 SDF 画圆角矩形/边框/线性渐变，`fwidth` 抗锯齿；
  2. **Text**：字形图集四边形（cosmic-text shape 后栅格化进 etagere 图集，灰度 AA 起步）；
  3. **Image**：纹理数组 + UV，tint 与九宫格；后台线程解码，按「路径+内容哈希」缓存 GPU 纹理并生成 mipmap；
  4. **特效**（M6+）：阴影 SDF 近似、blur 两 pass 高斯 + 离屏纹理、opacity layer。
- 实例带 `transform(2×3)` + 双层 clip 矩形（圆角裁剪 shader 内做，Flutter 式取最紧裁剪）；按「管线 → 材质 → 层序」排序合并，典型界面 draw call 两位数。
- 渲染树构建时剔除不可见 / alpha=0 / 屏幕外子树。
- 每窗口一个 `wgpu::Surface`（共享 Device/Queue），Bgra8UnormSrgb，Fifo present，正确处理 resize 与 DPI。
- 帧调度：按需重绘（输入/动画/脏属性才 `request_redraw`）；v1 每帧重建渲染树（万级节点无压力），v2 增量。
- Path/SVG 远期用 lyon 瓦片化或集成 vello，v1 不做。

## 7. 跨平台支持（v1 = 三大桌面）

| | Windows | macOS | Linux |
|---|---|---|---|
| 目标 | Win10+ x64（ARM64 可选） | macOS 12+，universal2（Intel + Apple Silicon） | X11 + Wayland，x64 / aarch64 |
| wgpu 后端链 | Vulkan → D3D12 | Metal | Vulkan → GL |
| 软件兜底 | WARP（D3D12 回退自带） | 无需（Metal 全覆盖） | lavapipe / llvmpipe |
| 窗口/DPI | winit，per-monitor DPI v2 | winit，backing scale | winit，Wayland 分数缩放 |
| IME | winit IME | winit IME | winit IME（ibus/fcitx 走 wayland text-input / xkbcommon） |
| 剪贴板/拖放 | arboard + winit | 同 | 同（Wayland data-control） |

工程规则：

1. 平台差异全部隔离在 `nui-winit` 与打包层；`nui-runtime/render/layout/text` 禁止 `#[cfg(target_os)]`，CI grep lint 强制（D13）。
2. 引擎内部坐标一律 **dp**，仅渲染提交时乘 scale factor 换算物理像素；文本基线取整规则统一。
3. 系统能力（文件等）走引擎能力接口，不散落 `std::fs`——为未来 wasm/移动留门。
4. 原生集成 M6：文件对话框（rfd）、菜单栏（muda）、托盘；无架构障碍。
5. CI 自 M0 起三 OS 矩阵：fmt/clippy/test 全平台；渲染黄金图只在 Linux + lavapipe（像素确定性），Win/mac 冒烟。
6. 打包（M6）：Windows Inno Setup / zip；macOS .app bundle（Info.plist 高 DPI）+ 签名；Linux AppImage + .desktop。

## 8. 工具链

- **nui-preview**：notify 监视 → 重编译 → 重建树，编译错误叠加显示在预览窗口；本项目相对 Slint 的体验卖点，M5 优先落地。预览器重点呈现 `<-` 显式运算符带来的类型错误定位能力。
- **LSP**（M6+）：补全节点类型/属性、跳转、诊断；复用 nui-compiler 的解析与类型检查。
- **Inspector**（M6+）：调试覆盖层——元素树、属性值、重绘区域。
- **nui-macros `include_ui!`**（M5）：编译期编译校验 + bundle 嵌入。

## 9. 测试与质量

- parser 快照测试；绑定/依赖图无头单测（不渲染）；headless 引擎脚本化事件驱动集成测试。
- 渲染黄金图：offscreen 渲染读回像素比对，CI 用 lavapipe 保证可复现。
- 全工作区统一 lints 已就位（`unsafe_code = forbid`、clippy `implicit_return` / `unwrap_used`）；交付循环 `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`。
- 库错误一律 thiserror typed error；公共 API 不出现 `Box<dyn Error>` / `Err(String)`。

## 10. 依赖清单（开工时锁定当日最新版本）

| 依赖 | 用途 | 引入里程碑 |
|---|---|---|
| thiserror | 库 typed error | M0 |
| slotmap | 元素 arena | M2 |
| taffy | flex/grid 布局 | M2 |
| cosmic-text | 文本 shaping/布局/字体 | M2–M3 |
| wgpu（当前 30.x） | 渲染 | M3 |
| winit | 窗口/输入 | M3 |
| bytemuck | GPU buffer 转换 | M3 |
| etagere | 图集打包 | M3 |
| image | 图像解码 | M3 |
| notify | 文件监视 | M5 |
| arboard | 剪贴板 | M7 |
| rfd / muda | 原生对话框 / 菜单 | M13 |

## 11. 里程碑

| 里程碑 | 内容 | 验收标准 |
|---|---|---|
| **M0**（~1 周） | nui-core 基础类型（Value/Length/Color/Duration/几何/typed error）；三平台 CI 矩阵 | fmt/clippy/test 全绿 × 3 OS |
| **M1**（2–3 周） | nui-syntax + nui-compiler：调用风格语法、类型检查、Document IR、表达式字节码 | 错误用例产出 rustc 级诊断；parser 快照测试 |
| **M2**（3 周） | nui-runtime（树/属性/绑定/信号/状态机/动画时钟）+ nui-layout（taffy）+ 内置 Rectangle/Text/Timer | 无头逻辑测试：含状态机的 counter |
| **M3**（3–4 周） | nui-render（rect/text/image 管线）+ nui-winit + nui 门面 | 三平台窗口可点击 counter demo |
| **M4**（3 周） | 动画完善 / `when` 视觉状态 / For+Model / 自定义 Rust 组件注册 | todo-list demo |
| **M5**（2 周） | nui-preview 热重载 + nui-macros `include_ui!` | 改 `.nui` 即时生效 |
| **M6**（2026-09-25 完成） | 文本渲染管线（cosmic-text shaping + 字形图集 + taffy 内在尺寸 + WGSL 管线；M3 缺口补齐） | 离屏 golden：字形像素点亮；preview 诊断窗口内叠加 |
| **M7**（3 周） | 焦点/键盘路由 + `TextInput`（光标/选区/编辑/剪贴板）+ IME Commit | 登录表单 demo：`text <=> 属性` 双向输入 |
| **M8**（2–3 周） | image 管线（异步解码/路径+哈希纹理缓存/tint/九宫格，M3 缺口）+ 阴影 SDF（rect 管线内做软阴影） | 图片 + 阴影视觉 demo |
| **M9**（2–3 周） | 合成管线：离屏渲染目标 + 裁剪矩形（plan §6 clip 栈）+ blur 两 pass + opacity layer（三者共用同一套离屏基础设施，故合并）+ 滚动 | 圆角裁剪 + 毛玻璃 demo |
| **M10**（2 周） | `ListView` 虚拟化 + 滚轮/ScrollArea 接入模型行 | 万行大列表流畅滚动 demo |
| **M11**（1 周） | **多端编译与冒烟（Windows / macOS）**——见下节 M11 实施清单 | 三平台 CI 全绿 + 冒烟记录 |
| **M12**（2 周） | AccessKit 无障碍树 + LSP（补全/跳转/诊断，复用 nui-compiler） | 编辑器里补全 `.nui` 属性 |
| **M13**（1–2 周） | 原生集成（rfd 文件对话框 / muda 菜单 / 托盘）+ 三平台打包（Inno / .app / AppImage；依赖 M11） | 三平台可分发安装包 |


### M11 实施清单：多端编译与冒烟（Windows / macOS）

**现状（2026-09-25 盘点）**：代码层面平台隔离已由 D13 强制（`cfg(target_os)` 只允许在 nui-winit）；本地交叉 `cargo check --workspace --all-targets` 对 `x86_64-pc-windows-msvc` 与 `aarch64-apple-darwin` **一次通过**——编译层已干净。真正的空白是「从未在非 Linux 环境运行过」：仓库 0 commit、无 remote,三平台 CI 矩阵（M0 就写好的 `.github/workflows/ci.yml`）从未执行。

1. **推送仓库,让现有 CI 先跑起来**：fmt/clippy/test × (ubuntu/windows/macos) + D13 lint。第一轮预期会有失败,逐个修（多半是路径分隔符、行尾、例子需要显示等琐碎问题）。
2. **离屏测试上三平台**：现有 offscreen 测试不需要窗口,只需要一个可用的 wgpu adapter——Windows 用 DX12（CI runner 有 WARP 软件兜底）、macOS 用 Metal、Linux 用 lavapipe。像素断言只留在 Linux（plan §9 确定性）,Win/mac 断言放宽为「能渲染 + 无 panic」。
3. **GUI 冒烟 bin**（`examples/smoke.rs` 或工具 bin）：开窗 → 跑若干帧 → 主动关闭,退出码 0。windows/macos runner 有图形会话可直接跑;Linux CI 用 xvfb。
4. **本地验证手段（无 CI 时）**：`cargo check --target <target>` 交叉检查（本里程碑已验证可用的第一道闸）;真机/虚拟机跑 demo;公共仓库的 Actions 免费额度是最省事的远程验证通道,macos-14 顺带覆盖 Apple Silicon。
5. **产出**：三平台 CI 全绿 + smoke 运行记录 + 平台差异清单（若有,回写 nui-winit 文档）。

**M11 部分落地（2026-09-25）：渲染 fallback 已实现**——用户实测 Ivy Bridge（Vulkan 驱动残废）上 `request_adapter` 全失败,根因是 GL 后端需要 display handle 而 M3 用了无头式 `new_without_display_handle()` 创建实例。现在 `init_gpu` 按回退链尝试:① 默认后端（Vulkan/DX12/Metal,无 display handle）→ ② GL + winit `owned_display_handle`（GLES 展示必需）,逐个创建 instance+surface 并 request_adapter,选中后打印适配器名与后端;`NUI_BACKEND=vulkan|gl|dx12|metal|primary` 可强制指定。待用户在 Ivy Bridge 实机验证 GL 路径后 M11 才算完成。

**交互失效修复（2026-09-25）**：用户实测报告按钮无效、输入无反应——根因是 `EventTranslator` 对 `MouseInput` 发出 `position: Point::ZERO`（注释声称 facade 跟踪位置但从未实现）,且 translator 每事件重建导致修饰键状态丢失、`ModifiersChanged` 根本未处理。修复:① translator 持久化于 WindowHost（修饰键/光标跨事件保留）;② 增加 `cursor` 字段,`MouseInput` 复用前次 `CursorMoved` 位置;③ 新增 `ModifiersChanged` 分支;④ translate 的分支拆成可测方法（`cursor_moved/mouse_input/modifiers_changed`）,绕开 DeviceId 无法安全构造的问题;⑤ 字形缺像素:`TextInstance` 绘制原点取整到物理像素（分数坐标+线性采样涂抹细笔画）。新增 3 个 translator 状态测试。

**字体抗锯齿修复 + fcitx5 输入法支持（2026-09-25）**：
  - AA 根因:字形 UV 映射的『半纹素内缩』公式错误（`uv_size=(w-1)/S` 使采样步长 progressively 偏移,到字形右侧偏出一整个纹素,细笔画与空白邻居混色→丢点）。改为精确 1:1 映射（`uv_origin=x/S, uv_size=w/S`）,整数对齐 quad 的片元中心正好落在纹素中心,线性采样零模糊
  - fcitx5/ibus/XIM 支持:①`window.set_ime_allowed(true)`（winit 默认关闭 IME,不开则组合事件永不到达）;②`Event::ImePreedit` 新事件（winit `Ime::Preedit` 此前被丢弃）;③输入框内联渲染组合文本:光标处插入 preedit、下划线标记组合跨度、光标移至组合末尾;commit 提交后清空 preedit
  - 已知限制:preedit 光标偏移（字节→字符）未使用、组合期间按键仍走 handle_key 路径（X11 `is_composing` 会抑制,Wayland text-input 同理）

（时长为单人业余节奏粗估，可调。M7–M13 顺序按依赖排：输入完整性 → 视觉完整性 → 大规模数据 → **多端地基** → 可达性/工具链 → 交付。）

（时长为单人业余节奏粗估，可调。）

## 12. 风险与对策

- **绑定成环**：编译期静态检查 + 运行时求值深度上限，双保险。
- **解释器性能**：字节码 + 运行时常量池起步，热点可加内联缓存；UI 表达式普遍很小，预计非瓶颈。
- **文本是最大泥潭**：不自研 shaping，全押 cosmic-text；Editable text 的 IME 单独立项。
- **wgpu 大版本跟进**（30.x，下游常滞后）：渲染层收敛在 nui-render 单 crate，升级面可控。
- **范围蔓延**：DSL v1 严格不碰 JS/闭包/循环；复杂逻辑一律下沉 Rust。

## 13. 未决事项

- 项目与语言命名：工作名 `nui` / `nui-lang`，发布前需查 crates.io 占用情况。
- License 假定 `MIT OR Apache-2.0`（Cargo.toml 已按此填写），如有其他偏好需确认。

## 14. 当前状态

**2026-09-19 · M0 完成**

- [x] workspace 骨架：11 个 crate 成员 + 统一 lints + 路径依赖连线
- [x] git 仓库已初始化（main 分支，尚未提交）
- [x] **M0 完成**：`nui-core` 7 个模块——`error`（thiserror typed error）、`color`（含 `#RGB/#RGBA/#RRGGBB/#RRGGBBAA` 解析）、`length`（dp/%/auto + resolve）、`duration`（f64 毫秒）、`geometry`（Point/Size/Rect）、`value`（动态值 + 类型化读取 + Display 插值）、`event`（指针/键盘/滚轮/窗口归一化事件）
- [x] CI：`.github/workflows/ci.yml`——fmt/clippy/test × (ubuntu/windows/macos) + D13 cfg 隔离 grep lint（本地已验证，推送 GitHub 后生效）
- [x] M0 验收：`cargo fmt` / `clippy --all-targets -D warnings` / `test`（38 单测 + 3 doc 测试）本地全绿；D13 grep 检查通过
- [x] **M1 完成**（2026-09-24）：nui-syntax + nui-compiler 全部落地
  - `nui-syntax`：lexer（单位/颜色/字符串插值/kebab 标识符）、错误恢复 parser、AST、rustc 风格 span 诊断渲染（`render_diagnostic`）
  - `nui-compiler`：类型检查（含 `Enum` 类型与枚举字面量 `bold`/`ease-out`）、作用域与 id 解析、**绑定环静态检测**（DFS 找环，报 `a <- b <- a` 链）、Document IR、TypedExpr 表达式字节码（Effect/Interp/Builtin）
  - M1 验收：错误用例产出 rustc 级诊断 ✓；parser 快照测试 ✓（`tests/snapshot.rs`，token 流 + AST s-表达式 + 诊断 + 空白不敏感性，5 个快照）；`cargo fmt` / `clippy --all-targets -D warnings` / `test`（114 测试全绿）；D13 grep 通过
- [x] **M2 完成**（2026-09-24）：nui-runtime + nui-layout 全部落地
  - `nui-runtime`（5 模块）：`element`（slotmap 世代 arena + 属性槽 + id 表 + D10 绑定清除 + `<=>` 记账）；`binding`（TypedExpr 求值器、thread-local 求值栈依赖追踪、拉取式帧首传播——读 dirty 依赖即先求值，拓扑序免费；EVAL_STACK 重入即环，运行时环检测为第二道防线；信号 emit → handler + 状态机迁移扇出；enter/exit 效果；D10 语义）；`machine`（MachineInstance = MachineIr + current_state，守卫引擎侧求值）；`animation`（双层值模型：binding 写目标层，时钟 tick 推进显示层；tween 三缓动曲线 + spring 半隐式欧拉积分、从当前显示值起步、retarget 不重复）；`instantiate`（DocumentIr → 树：文档序建元素、id 表后建、静态默认值字面量求值、`<-` 全部登记为脏、`<=>` 延迟到 id 表建好再连）
  - `nui-layout`：taffy 0.9 适配——Column/Row → flex 方向、spacing → gap、padding → 四边、`%`/dp/auto 尺寸映射；容器宽度 auto 默认撑满（百分比子项可解析）；结果回写 `x/y/width/height` 供绑定读取
  - 编译器配套：`AssignmentIr` 新增解析后的 `target: PropertyTarget`（修复构造参数 `Text(content <- …)` 被误判为组件属性的 bug；`font.size` 类附加属性以 `<self>` 前缀存活）
  - M2 验收：无头集成测试 ✓（`tests/integration.rs` 8 例：含状态机 counter 全管线、守卫阻断、绑定链、D10 端到端、双向同步、Timer 重复触发、运行时环防护、动画推进）；`cargo fmt` / `clippy -D warnings` / `test`（148 测试全绿）；D13 grep 通过
  - **M2.5 追加**（2026-09-24）：属性变更通知 `notify` 模块——`PropertyChange`（element/property/new_value/source）+ `ChangeSource`（Binding/Effect/WhenBlock/TwoWay/Animation/Host 六源头）+ `PropertyObserver` trait + 句柄式订阅；Engine 在五个写路径（binding 写回 / 双向同步 / effect 赋值 / when 块 / `set_direct` 宿主直写）缓冲变更，`take_changes` 帧尾排空并通知订阅者；动画时钟 `tick_with_notify` 记录 Animation 源变更。为 M4 `Model` 协议（行变更通知）与 Inspector 铺路。新增 4 个集成测试（订阅顺序、宿主直写、退订静默、动画缓冲），全库 154 测试
- [x] **M3 完成**（2026-09-24）：nui-render（rect 管线）+ nui-winit + nui 门面 → 可点击 counter demo
  - `nui-render`（3 模块）：`rect`（WGSL SDF 圆角矩形 + fwidth AA + premultiplied blend；`RectInstance` 64B `repr(C)` Pod storage buffer；CameraUniform 16B）+ `scene`（SceneBuilder 走树收 rect，opacity 乘 alpha，零尺寸/透明剔除）+ `lib`（Renderer，`render<'pass>` 生命周期签名）
  - **WGSL 对齐教训**：56B 实例布局中 `vec4<f32>` 隐式 16B 对齐使 fill 偏移错位（红色读回成通道错乱）；`_pad0: [f32; 2]` 补到 64B（fill @ offset 32），单测锁死布局
  - `nui-winit`：EventTranslator——winit 0.30 ApplicationHandler 事件 → nui-core 归一化事件（坐标 ÷ scale_factor 转 dp，Back/Forward→Other(4/5)，3 单测）
  - `nui` 门面（2 模块）：`app`（Application 运行循环 + AppConfig + 矩形 hit_test，最深元素胜出）+ `host`（WindowHost：gpu init → compile → instantiate → surface FIFO sRGB → press+release 同元素 emit "click" → 帧管线 → resize 重配）
  - 帧管线接入 nui-runtime：`run_frame_pipeline(delta)` = tick_timers → clock.tick_with_notify → 绑定传播 → when 块 → layout → needs_redraw → take_changes（M4 observer 挂点已就位）
  - wgpu 30 适配全记录：`InstanceDescriptor::new_without_display_handle()`、surface 需 `Arc<Window>`、`request_adapter` 返回 Result、`get_current_texture` 返回枚举（非 Result）、present 移至 Queue、`PollType::Wait{..}` struct 变体、`BufferSlice::map_async` 回调式、PipelineLayout `bind_group_layouts: &[Option<_>]>` + immediate_size
  - M3.6 离屏验收：`tests/offscreen.rs` 3 例像素读回（lavapipe）——rect 覆盖区域、圆角角落透明、后实例盖前实例；`bytes_per_row` 256 对齐 + `map_async`/mpsc + `poll(Wait)` 模板
  - M3.7 验收：`cargo fmt` / `clippy --workspace --all-targets -D warnings` / `test`（单元/集成 161 + doc 6 = 167 全绿）；`cargo build -p nui` 零警告；`examples/counter.rs`（内联 .nui，点击 Button → count += 1 → Text 重渲染）编译通过
- [x] **M4 完成**（2026-09-25）：动画完善 / `when` 视觉状态 / For+Model / 自定义 Rust 组件注册 → todo-list demo
  - **For+Model**（核心）：`Value::Model(u32)` 新变体 + 编译器 `Type::Model`（无字面量，宿主赋值；未赋值读出 `UNSET_MODEL` 哨兵）；`model.rs`——`Model` trait（row_count/field/as_any_mut）+ 内置 `VecModel`，变更全部走 Engine API（`add_model / model_push / model_remove / model_set_field`）；`For` 节点实例化只存 `ForBinding`（variable + iterable + prototype）并把 iterable 注册成 `@for` 属性绑定，行元素由 `Engine::sync_for_nodes` 按模型行数拉起/重建（pull 式，结构变更免失效）；行级作用域 `RowScope{variable, model, row}` 挂在行子树每个元素上，`item.field`（`TypedExpr::Local` 携带变量名后运行时按名解析）经祖先链提升到行根，依赖键 `(row root, "@field.<name>")` 与失效键一致——`model_set_field` 单键失效整行；行重建时旧行绑定 `retire_bindings` 标记 dead（索引稳定，指针不悬空）；`ElementTree::remove_subtree` 支持子树移除（含 id 表清点）；布局把 `For` 当纵向容器
  - **动画完善**：`tween/spring` 从透传改为真正拦截——绑定求值写入动画目标而非显示值（D8 两层值模型落地），Engine 接管 `AnimationClock`（`tick_animations / has_active_animations`）；参数（duration/easing/stiffness/damping）每次求值重取，缺省 200ms/EaseInOut/120/14；目标等于当前值即取消动画；**弹簧修复**：M2 版每帧从截断后的整型显示值读回状态导致量化死锁（卡在 98），改为动画内部保存连续 position/velocity，显示值只是投影；Int 目标收敛阈值放宽到 0.5
  - **帧循环修复**（M3 遗留）：`RedrawRequested` 从未接到渲染、帧管线只以 `ZERO` delta 运行（计时器/动画在真实窗口不走）；现在 `render_frame` = 真实墙钟 delta + 管线 + draw，动画/计时器期间 Poll，静止后 Wait；帧管线加入 `sync_for_nodes`（重建行 >0 时二次 propagate）
  - **when 视觉状态**：两个修复——① 条件离开时静态值不还原（只 mark_dirty），现在覆盖时捕获旧值、离开时按「有绑定则重求值 / 静态则还原旧值」处理，且不再每帧重复 push 覆盖记录；② `panel.opacity` 类点名赋值曾以拼接名 `"panel.opacity"` 写入（M2 测试锁死的错误行为，SceneBuilder 读不到），改为 `target_property_name` 按写入目标取属性名
  - **Registry（宿主互操作）**：`registry.rs`——`ComponentDesc/PropertyDescriptor`（描述符默认值在实例化时应用）、`ElementBehavior` trait（信号到达时经 `BehaviorContext` 读写属性/发信号/操作模型行：`row_scope / model_field / set_model_field / remove_model_row`）、`HostFunction`（效果单名调用 + 表达式 `TypedExpr::HostCall` 均可调用）；编译器 `check_with/compile_with_functions` 按宿主函数名校验（typo 仍是编译期错误）；`Registry::on_attach` 一次性钩子是宿主注册 Model/播种状态的缝；`instantiate_with(document, registry)` + 门面 `Application::with_registry`
  - 顺带实现：效果块 `let` 局部变量（M2 是空操作）——求值期 locals 栈、语句列表级作用域
  - M4 验收：`examples/todo.rs`（VecModel 驱动 For 行 + Checkbox/RemoveButton 自定义组件改模型 + tween 透明度动画）编译运行；无头端到端测试 `todo_demo_pipeline_end_to_end` 走完整管线（建行 → 点击切换 done → 填充色重绑定 → 动画推进到目标 → 删行重建）；`cargo fmt` / `clippy --workspace --all-targets -D warnings` / `test`（177 全绿）；D13 grep 通过
- [x] **M5 完成**（2026-09-25）：nui-preview 热重载 + nui-macros `include_ui!` → 改 `.nui` 即时生效
  - **热重载缝**：`nui_runtime::reload_from_source(&mut Instance, source)`——重编译（宿主函数名对照当前 registry 校验）→ 原地重建 tree/engine → 重跑 `Registry::on_attach` 钩子（模型在新引擎上重新注册；钩子从一次性改为每次 attach 重跑）；v1 全量重建、丢元素状态（plan §3.4 合同）；编译失败时实例原样保留并返回渲染好的诊断 → 窗口保持最后一帧好画面；`WindowHost::reload` 包装之（清 timer 累计、置脏重绘）；无头测试覆盖「失败保旧 / 成功换新（新绑定 + 新默认值 + 模型重注册）」
  - **运行循环泛化**：`DocumentWatcher` trait（`attach(ReloadWaker)` + `poll_reload`）+ `ReloadWaker`（EventLoopProxy 包装——文件变更不产生 winit 事件，Wait 模式必须从后台线程唤醒循环）；`Application::with_registry/with_watcher` 改为 builder；about_to_wait 里轮询 → reload，失败则 stderr 报诊断 + 窗口标题标记「compile error」（v1 无文本管线，窗口内叠加诊断留待 M6 文本渲染）
  - **nui-preview**（tools/nui-preview）：notify 8 监视 + EventLoopProxy 唤醒；按内容去重（多事件保存/瞬时半写不触发）；CLI `<file.nui> [--width/--height dp]`；`examples/counter.nui` 为演示文档；本容器无 GPU 适配器（lavapipe 未装）无法起窗，桌面端正常
  - **nui-macros `include_ui!`**：宏展开期读取文件 → `nui_compiler::validate_source`（下沉到 nui-compiler 供工具复用）→ 有诊断则 `compile_error!` 输出 rustc 风格错误（坏文档 = 宿主构建失败，错误定位到 .nui 源）；干净文档经 `include_str!(绝对路径)` 嵌入为 `&'static str`（cargo 重建跟踪随文件变更触发）；bundle 序列化仍按 plan §3.4 留作发布可选项；集成测试锁「嵌入源运行时可编译」
  - M5 验收：`cargo fmt` / `clippy --workspace --all-targets -D warnings` / `test`（185 全绿）；D13 grep 通过；nui-preview CLI 三路径（缺参/文件不存在/正常启动）冒烟通过
- [x] **M6 完成**（2026-09-25）：文本渲染管线（M3 缺口补齐——M3 实际只交付了 rect 管线，Text 此前是 0 尺寸黑块）
  - 范围说明：M6+ 为按需排期开放清单，本轮选定文本渲染（TextInput/preview 叠加/一切真实 UI 的前置）；特效、TextInput+IME、ListView 虚拟化、AccessKit、LSP、原生集成、打包仍留待后续
  - `nui-text`（cosmic-text 0.19 + swash + fontdb + etagere）：`TextSystem` 持有 FontSystem + SwashCache + 字形图集；内置 DejaVu Sans（`assets/fonts/`，含 LICENSE）——`with_embedded_font()`（确定性、离线，golden 测试用）与 `with_system_fonts()`（系统字体 + 内置兜底，应用默认）；`shape`（Buffer 单段、无换行，line_height = 1.2×）产出基线原点 + CacheKey；`measure` 按 (text, size) 缓存（taffy 每帧多次回调）；`glyph_quad` 首次栅格化入图集、之后查表，placement（基线偏移）与槽位同存
  - 字形图集：etagere shelf 打包，1024² R8 alpha 页按需增长（探测分配即归还，避免泄漏）；脏页快照上传一次（预热后零上传）；超大/空白/彩色字形拒绝（emoji v1 跳过，文档注明）
  - 布局内在尺寸：`nui-layout::layout_with_text`——Text 叶子用 taffy `new_leaf_with_context` + `compute_layout_with_measure` 提供内容尺寸（读 `content` 与 `font.size`，缺省 16dp）；`layout()` 保持旧签名向后兼容
  - nui-render 文本管线：`TextInstance`（48B：origin/size/uv_origin/uv_size/color，WGSL vec4 16B 对齐已核）+ 专用 WGSL（图集 R8 采样、半 texel UV 内缩防串色、premultiplied blend）；`SceneBuilder::build_from_with_text` 提取字形四边形（Text 元素不再画黑底矩形）；Renderer 按 atlas 页分桶、每页一次 draw（bind group 内嵌实例缓冲，容量增长时重建——rect 管线同款隐患在 text 侧已处理，rect 侧注释仍在）
  - **preview 诊断窗口内叠加**（M5 遗留升级）：重载失败 → `WindowHost::set_error_overlay` 把渲染好的 rustc 风格诊断以红字画在最后一帧好画面上（24 行封顶），成功后清除；stderr 报告保留
  - M6 验收：离屏 golden 测试 `offscreen_text`（内嵌字体 + lavapipe 确定性）——字形像素点亮、背景保持黑色；`cargo fmt` / `clippy --workspace --all-targets -D warnings` / `test`（193 全绿）；D13 grep 通过；counter/todo 示例的 Text 现在真实渲染
- [x] **M7 完成**（2026-09-25）：焦点/键盘路由 + `TextInput`（光标/选区/编辑/剪贴板）+ IME Commit → 登录表单 demo
  - `text_input.rs`：`TextInputState` 纯编辑核心——**char 索引**（多字节安全）、selection 语义（anchor/cursor、扩展时从折叠处锚定）、insert/backspace/delete/move/select_all/cut；`ChangeSource::Input` 新源头
  - 引擎路由（`impl Engine` 在 text_input.rs，沿用 model/registry 分文件先例）：`focus/blur/focus_next`（Tab 按文档序循环）、`handle_key`（Tab/字符插入/Backspace/Delete/方向键/Home/End + shift 选区、Ctrl+A 全选、Enter 发 `accepted` 信号、Escape 失焦）、`handle_text_input`（IME Commit 走这里——普通字符经 handle_key 插入，桌面无 IME 时不会丢字）、`copy_focused/cut_focused`、preedit 状态位；编辑前从 `text` 属性 reconcile（外部写入以属性为准），写回走 D10 清 `<-` 绑定 + `<=>` 值通道
  - **修复 M2 遗留真 bug**：`prop <=> root.partner` 的自动连线从未生效——partner 在赋值的 **value 表达式**里，而 `link_two_way_pairs` 却在 target 路径里找三段路径（M2 测试是手工 `set_two_way` 绕过的）；现在按 `TypedExpr::Property` 的 target 解析 partner（Root/Component/Id 通用），M2 集成测试的语义真正成立
  - TextInput 渲染（SceneBuilder）：背景 rect（缺省深灰 + 圆角）、文本/placeholder（灰、空态）、选区高亮、聚焦光标（2dp 竖线，`measure` 前缀宽度定位）；hit_test 命中即聚焦、点空白失焦
  - host 接线：PointerPressed 聚焦、KeyPressed/TextInput 分发（处理过则跑帧管线）、arboard 剪贴板（懒创建）Ctrl+C/X/V
  - M7 验收：`examples/form.rs` 登录表单（双输入 `<=>` 绑定 + Enter 提交 + when 视觉态）；`cargo fmt` / `clippy --workspace --all-targets -D warnings` / `test`（202 全绿）；D13 grep 通过
  - 已知 v1 限制：preedit 只存不显示（下划线/候选窗随 M8+）、光标不闪烁、IME 活跃时与字符键理论上有双入风险（平台相关，后续按 `event.text` 消歧）
- [x] **M8 完成**（2026-09-25）：image 管线（异步解码/路径+哈希纹理缓存/tint/九宫格）+ 阴影 SDF → gallery demo
  - `nui-render/image.rs`：`decode_file`（image 0.25，png/jpeg/gif/webp/bmp）+ FNV 内容哈希 cache key（「路径+内容哈希」，plan §6.3）；`ImagePipeline`——Rgba8UnormSrgb 纹理（采样线性化、sRGB surface 还原，比 rect 的直通路径色彩正确）、实例化 textured quad、tint、按纹理 key 分桶绘制（bind group 内嵌实例缓冲，容量增长重建）；`nine_slice_quads` 纯函数九宫格拆分（slice 过大 clamp 到纹理半宽，角落 UV 恒定像素尺寸）；无纹理 key 的 draw 静默跳过（解码在途）
  - host：`Image` 元素扫描 → 后台线程解码（mpsc 回填）→ `image_keys/image_store/image_inflight` 三本账，失败路径记空 key 不重试；解码落地置脏重绘
  - 阴影 SDF（rect 管线内，plan §6.4）：`RectInstance` 64→96B（shadow_color/blur/offset），WGSL 软阴影 = 偏移矩形 SDF + smoothstep 模糊带，premultiplied 合成于 fill 之下；**quad 按出血量（blur+|offset|）扩张**——阴影在原 quad 外根本没有片段可着色（调试定位：顶点 `local` 双加 bleed 的坐标 bug，GPU 探针法逐步逼近）
  - M8 验收：离屏 `offscreen_image`（解码 PNG → 纹理 → tint 绘制 + 九宫格角部保真）与 `offscreen_shadow`（阴影区变暗、远角保持背景）全绿；九宫格 UV 纯单测 3 例；`examples/gallery.rs`（生成 BMP 资产 + 阴影卡片 + 九宫格图）；`cargo fmt` / `clippy --workspace --all-targets -D warnings` / `test`（208 全绿）；D13 grep 通过
  - 已知 v1 限制：Image 需要显式 width/height（异步解码，内在尺寸随 M9）、mipmap 未生成（线性采样近距无碍）、纹理无淘汰策略
- [x] **M9 完成**（2026-09-25）：合成管线——离屏渲染目标 + 裁剪矩形 + blur 两 pass + opacity layer + 滚动
  - **clip 栈（plan §6）**：三管线实例统一携带 clip_bounds/clip_radius（rect 96→128B、text/image 48→80B；WGSL `array<f32,3>` 填充——vec3 是 16B 对齐,曾致布局错位）,共享 `CLIP_WGSL.clip_mask`（圆角 SDF + smoothstep）在片元裁剪;SceneBuilder 重写为统一递归 walk,clip 沿祖先链取交集（最紧裁剪）;`clip = true` 属性开启
  - **Scroll**：布局纵向容器;场景 walk 平移子树（-scroll_y）+ viewport 裁剪;host 滚轮 → 命中元素的最近 Scroll 祖先 `scroll_y` set_direct（绑定可见）;hit_test 重写为携带滚动偏移的递归（滚动后内容按平移位置命中）——M3 版 visit_pre_order 无法携带层级偏移
  - **离屏 layer**：`layer.opacity < 1` / `layer.blur > 0` 的元素把子树捕获进子 Scene（纹理恰好覆盖元素矩形,溢出内容被纹理边界裁掉;`layer_root` 标志防同元素递归捕获）;`Renderer::render_to_view` 递归渲染嵌套 layer → blur → 主 pass;**顺序关键**：外层 prepare 必须在嵌套 layer 渲染之后（共享实例缓冲会被内层 prepare 覆盖）
  - **blur 两 pass 高斯（plan §6.4）**：9-tap 二项式核 ping-pong（水平/垂直）,REPLACE blend;踩坑两个——step 必须是 UV 单位（传纹素被 ClampToEdge 拉边成污渍）,且两 pass 共用 uniform buffer 必须逐 pass 提交（批量提交会都读到第二次写入）
  - M9 验收：离屏测试 4 例（clip 裁剪溢出/无 clip 溢出可见/组透明度线性空间合成/blur 扩散与远角干净）+ hit_test 滚动偏移测试 + scene 滚动平移单测;`examples/showcase.rs`（Scroll+For 模型列表 + opacity 卡片 + blur 药丸）;`cargo fmt` / `clippy --workspace --all-targets -D warnings` / `test`（214 全绿）;D13 grep 通过
  - 已知 v1 限制：layer 内容超出元素矩形被纹理裁掉（无外溢 padding）、layer 绘制次序固定在 rects/images/texts 之后（兄弟交叠的 z 边角情况）、滚轮无内容边界钳制（只钳 0）
- [x] **M10 完成**（2026-09-25）：`ListView` 虚拟化 → 万行大列表 demo
  - 语法：`ListView(item in root.rows, id = …, row_height = 36dp, …)`——复用 `For` 的 `(item in iterable)` 子句；解析器支持**子句与属性参数混写**（`ident in` 前瞻识别子句，逗号后续接普通参数列表；纯参数列表不受影响）；checker 的 for 作用域对任意深度子树生效（验证发现此前测试只覆盖直接子节点,孙节点引用其实一直正常——期间排查出的是自建测试变量名笔误,非 checker bug）
  - 虚拟化引擎（model.rs `sync_list_view`）：只实例化可见窗口的行——`row_height`（缺省 40dp）× viewport 高度算窗口 `[first, first+count)`（+1 overscan）;**前导 Spacer 元素**承载 `first × row_height` 偏移,使 taffy 把窗口行排到绝对位置;窗口移动才重建（滚动不重建,重建时旧行绑定 retire + 子树移除）;`list_windows` 簿记随 sync 清理;`first` 钳到 `total - 1`（滚到底窗口不空）
  - 行失效适配:`model_set_field` 对 ListView 站点按 `row - first + 1` 定位窗口内行根（窗口外跳过）
  - 场景/输入接入:ListView 与 Scroll 同享场景平移+裁剪、滚轮路由、hit_test 偏移;Spacer 不绘制
  - M10 验收:集成测试——窗口有界（100 行/10dp/50dp 视口 → 7 个子元素）、滚动换窗后行作用域与绑定值正确、窗口内字段写入精确失效、**万行滚动窗口 ≤12 元素、总元素 <30**;`examples/biglist.rs`（10,000 行滚轮浏览）;`cargo fmt` / `clippy --workspace --all-targets -D warnings` / `test`（219 全绿）;D13 grep 通过
  - 已知 v1 限制:行高均匀（可变行高虚拟化需测量缓存）、无滚动条、滚轮只钳下界（内容总高未知）
- [x] **M10.5 追加（2026-09-25）**：playground 演示 + 离屏截图模式（`--snap` 写 PNG 到 snapshots/,无显示环境可验证渲染）+ 点击**冒泡**（`Engine::emit_bubble`,按钮文字不再挡点击）。截图审查暴露并修复 **4 个真实渲染 bug**——此前 demo 全是浅嵌套+高饱和色,全部漏检：
  1. **布局坐标父相对**：taffy location 是相对父级的,场景/hit_test 却当绝对坐标——嵌套元素整体错位;layout 回写改为沿树累加绝对坐标
  2. **颜色空间双重编码**：sRGB 颜色被当线性值写入 sRGB target,整体变亮;新增 `linear_rgba`(sRGB→线性→premultiplied)统一用于 rect/text/image 提交;clear 色同理（wgpu::Color 对 sRGB target 是线性）
  3. **swash placement 符号**：placement.top 是 Y 向上为正（基线→字形顶）,屏幕坐标 Y 向下需取反——否则全部字形画到基线下方
  4. **flex-shrink 挤压**：taffy 默认 flex_shrink=1,虚拟化列表的 30000dp Spacer 把整容器内容压缩归零;改为 `flex_shrink: 0`（Qt-Quick 语义:元素保持自然尺寸,溢出容器）
  - 教训:像素截图审查是渲染管线的『验收测试』,.offscreen 断言（红 255/通道占优）对伽马与嵌套错位完全不敏感

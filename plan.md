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
| D16 | 组件实例化的作用域 | **按实例改写 IR + id 命名空间**（`iN::` 前缀），不给求值器加词法作用域链：id 表保持一张平表，属性读取热路径不变 | 2026-09-27 |
| D17 | 缓动曲线 | **具名曲线表**（设计系统的固定曲线集），不做通用 `bezier(x1,y1,x2,y2)` builtin——四个数字要有地方放进动态类型 `Value`；通用 builtin 留作后续，且届时无需迁移既有名字 | 2026-09-27 |
| D18 | 组件交互声明 | 由 `ComponentDesc::interaction` 声明，而非扩充 `WIDGET_TYPES` 硬编码表：组件实例化后其类型名是 `Rectangle`，名字只留在元素上，下游无从分辨 | 2026-09-27 |
| D19 | `emphasized` 半段曲线的来源 | 表中 `emphasized-first-half` / `emphasized-last-half` 是**推导值**（`emphasized` 两段各自 rescale 到整条时间轴），非照抄设计系统的同名 token——设计系统那两个 token 一个是 8 个数的畸形列表（照读只走到 x=1/6），另一个是未 rescale 的重锚定结果。推导过程由测试从 `emphasized` 现算，不再只是注释里的断言 | 2026-09-27 |
| D20 | 组件体的名字解析 | 组件体内**可以**引用文档中任一「入口组件」的元素 id（`DocumentIndex.document_ids`，在 `referenced` 算完之后收集）；组件自身的 id 优先。入口组件由运行时直接实例化、id 不被改写，所以只有它的 id 是文档级的；被实例化的组件 id 改写成 `iN::` 命名空间，裸名不指向任何东西。**不传递**：A 读到 `shell` 不等于 B 能经 A 读到。裸的未知标识符仍是枚举字面量（`check_ident`），本条只影响 `a.b` 的成员访问。动机：设计系统的明暗开关在窗口上，约 80 处控件都要读——作为参数传下去会把组件埋进一个不属于它的参数里 | 2026-09-30 |
| D21 | 矩形描边 | `Rectangle` 新增 `border.width` / `border.color` 两个属性，描边画在**框内**（等价 CSS `border-box`），不参与布局。关键在顺序：描边必须在 `fill` 之前读取——只有描边没有填充的矩形（outlined button、text field、divider）是真实形状，不能走「透明容器」提前返回；顺序写反的后果是整个设计系统的描边**静默消失**（元素不画了，不是画错）。GPU 侧 `RectInstance` 早有 `border_width`/`border_color` 字段、`push_rect_outline` 早被 widget part 用着，缺的只是元素属性这一个接线 | 2026-09-30 |
| D22 | 组件属�� id 与文档级 id 的改写 | 实例化改写只改写**组件自己声明的** id（`ComponentIr.ids`，由检查器在 `bind()` 末尾填齐）；文档级 id 原样保留。`ComponentIr.ids` 这个字段一直存在、也一直被文档描述，但**从来没有人填过**——之前改写无条件给所有 `PropertyTarget::Id` 加前缀，所以空集合无所谓；一旦改成「只改写自己的」，空集合就等于「什么都不改写」，两个实例共用一个命名空间 | 2026-09-30 |
| D23 | 入口组件也要改写 | 入口组件（`referenced == false`）同样要过 `mangle_node`，用 `entry<Name>::self` 作为 `self_id`。否则入口组件的 `Property(name)` 解析到**文档的第一个 root**——而一旦文档不止一个入口组件，那已经是另一个元素、甚至是不存在的属性。一个自己声明 `property name = ""` 的组件会读到 `0`，无任何诊断 | 2026-09-30 |
| D24 | `=` 读属性即绑定 | `=` 的值是字面量时仍是静态，是属性读时提升为绑定。运行时静态路径 `eval_literal` 只认字面量，属性读会被**静默丢弃**——而这正是「组件包着组件」的通用写法（`Wrapper(kind = kind)`），组件拿到声明默认值，看起来像渲染 bug。检查器的边记录同步改用 `init_kind` 的结果，否则 `Text(content = content)` 这种自环会绕过环路检查、在 `propagate` 里死循环 | 2026-09-30 |
| D25 | `offset_x` / `offset_y` | 新增两个属性，把元素在布局结果上平移。`x` / `y` 是布局的**输出**（每次布局都被覆写），文档无法用它移动子元素——于是没有办法把东西放在容器流动没给出的位置上，而滑块手柄、开关滑块、进度环都需要。偏移加在写回处而不是渲染器，所以 bounds、命中测试、绘制三者一致 | 2026-09-30 |
| D26 | 组件的 `Slot` | 组件用 `Slot` 元素声明「调用点内容放哪」，调用点的子节点填进去。组件的**内容**在调用方的作用域里绑定（id 属于调用点，不加实例前缀）——这正是它有用的原因。两条硬规则：**一个组件至多一个 `Slot`**（否则要有名字，而匿名的第二个就有歧义），**内容必须有 `Slot` 可去**（否则内容被静默丢弃，恰好是这个特性要防的失败）。`when` 块仍然禁止：它的条件只在实例化时求值一次 | 2026-09-30 |
| D27 | 引用体内的成员 | 组件引用的 body 现在接受：handler（不变）、子节点（即 slot）、以及仍然拒绝的属性赋值与 `when` 块。属性赋值需要「落在子树的哪里」的规则；`when` 没有答案 | 2026-09-30 |
| D28 | 环图的三个键 | 绑定环图按名字索引，于是三种「同名但不同槽」的情况被误报成自环。**嵌套节点的属性**（`Column(width = width)`：值读实例、目标写 Column，不同元素）、**组件实参的目标**（落在被调用方）、**根节点**（这里两者确实是同一槽，`Text(content = content)` 是真的自环，必须保留）。前两者各用一个没人写的哨兵名，根节点沿用原名 | 2026-09-30 |
| D29 | 字型是一个值 | `font.family` / `font.weight` / `font.italic` 三者都改变**输出哪些字形**，而字形只对产生它的字型正确。所以三者必须都到达 shaper，且两者之间的每个缓存都必须能区分它们。把三个参数穿过六个入口、四个缓存键，是这件事出错的版本：改签名不改键（错一次）、改键不改值（再错一次）。**一个自有值**让这两种错误写不出来。weight 保留为数字而非枚举——CSS 是数字、可变字体轴也是数字，`font.weight = 550` 因此可表达。默认（400 / 无 family / 不倾斜）逐字节等于此前的 `Attrs::new()`，所以既有快照一个像素都不动 | 2026-09-30 |
| D30 | 组件 `extends`：编译期摊平 | 派生在 `bind()` 里摊平成「父成员 + 自己成员」，所以运行时的 `own` 集合自动含父的 id，`mangle.rs` / `instantiate.rs` 无需改动。**运行时多一条派生概念**要在命名空间、绑定覆盖、id 解析三处各教它一遍，而摊平让这三处都不用知道派生存在——这正是选摊平的理由。覆盖**就地替换**（`ComponentIr::property` 是线性首次匹配，两条同名条目会递回父的默认值，这是最容易写出的半对版本）；派生体节点**追加进继承树的 `Slot`**（复用调用点填槽的同一机制，不引入第二条组合语义）；派生**不让父成为"被引用"**（`extends_edges` 与 `edges` 分开，否则派生入口组件会带出幽灵根）。`extends` 不作保留字（同 `from`），父可声明在子之后（按 `binding_order` 合并） | 2026-10-01 |
| D31 | 内建属性表：`nui-core::props` 单一来源 | 内建元素的属性面（名 + 类型 + 默认值）此前只以字符串字面量散落在渲染层与 widget 层（`f_property(element, "radius").unwrap_or(6.0)`）。三件事让它必须被写下来：① `extends Button` 让内建属性成为派生组件的**继承 API**，检查器得知道 `Button` 接受什么、类型是什么、缺省是什么；② 词表就位后拼错才有报错，否则 `Text(contnt = "x")` 永远是静默空操作；③ 检查器与运行时必须对**同一个默认值**取值，两处各写一份就是两处可以漂移。放 `nui-core` 是因为 `nui-compiler` 已经依赖它，而运行时也依赖它。`PropType` 不复用检查器的 `Type`：后者还有 `Unknown` 这类文档专有概念，元素属性不可能是。**只在类型名这一半接上校验**（D32），属性名那一半仍卡在语言决策上（§13） | 2026-10-01 |
| D32 | 类型名词表校验；属性名**故意不校验** | 节点类型名没有歧义——运行时按名字查元素表，查不到就什么都不画（`Buton(...)` 不报错、不警告、不渲染，布局直接吞掉这一行）。所以「未知元素类型」是编译错误，并附带编辑距离最近的名字建议（短名 1 个字符、长名 2 个字符为上限，超出就不建议——错的建议比没有建议更糟）。属性名**不能**这样查：元素状态与属性**故意共享**同一个名空间（`Column(id = containers_page, clicks = 0)`，读 `containers_page.clicks`），编译器无从区分「拼错的内建属性」与「作者有意挂的状态」。这条不是遗漏而是待决（§13 两条出路）。宿主注册的类型从 `Registry::vocabulary()` 一并上报——缺了这一步，宿主自己的组件会被误报成未知类型，而它们此前恰好因为"什么都不校验"而能过 | 2026-10-01 |
| D33 | 元素状态必须显式声明：`state` 关键字（§13 方案 a） | §13 的两条出路里选 (a)：`state name: Type = default` 与 `property` 同族，元素自己的状态从「作者有意挂的名字」变成「作者**声明过**的名字」。于是属性名这一半也能校验了——一个不在 `TABLE` 里、也没有 `state` 声明的名字是编译错误（附最近属性名建议，或 `declare it with state ...` 的提示）。**为什么不选 (b) 前缀**：`state.clicks` 会把每处读写都改一遍（`containers_page.clicks` → `containers_page.state.clicks`），而 (a) 只在声明处多一行，读点写法不变；平台的既有惯用法（把每页状态挂在根元素上）也原样保留。声明落成一条 `Static` 赋值，所以运行时看不到新概念——`ElementTree` 的槽位、绑定、`<=>` 通道一律照旧，唯一新增的是编译器在绑定前先登记这张 `node_states` 表。检查器只在**内建类型**上查（组件引用与非内建类型跳过，否则宿主注册属性的组件会被误报），并且只查**单段名**（多段名走既有 id 解析路径，`counter.value` 那类写法不受影响） | 2026-10-01 |
| D34 | 用户定义函数 `fn`：参数、返回值、前向可见 | 文档级 `fn name(p: Type, q: Type = default) -> Type { ... }`，可在表达式与效果块里调用。三处约束是有意为之：① **只能调用声明在自己之前的函数**（`collect_function_signatures` 顺序建表），于是循环调用在语法上就写不出来，不需要环检测；② **Void 函数不能当值用**（`fn bump(n: Int) { ... }` 用作 `Text(content <- bump(1))` 报错，因为那些写法的类型无法定），但可以作语句调用；③ 具名实参会被重排进形参声明序（`defaulted` 掩码记录哪些是补的默认值），所以 `f(b = 2, a = 1)` 与 `f(a = 1, b = 2)` 等价。新增字节码 `TypedExpr::UserCall` / `Effect::UserCall`（**不复用** `Effect::Call`：宿主函数走 `run_method_call`，而用户函数的实参要按 `defaulted` 补默认值、并且要能 `return`）/ `Effect::Return`；运行时 `Engine::call_user_function` 压一帧 locals、`run_effect_list` 让 `return` 提前收束并把 `If` 分支的返回值透传上来。函数体在绑定期就把形参声明为局部变量，因此 `item.field` 那类行作用域提升天然不适用（函数不在行里）——这是既有限制而非新缺口 | 2026-10-01 |
| D35 | `For` / `ListView` 体内可以引用组件 | 行（`instantiate_row`）由引擎每帧拉起，而展开组件需要文档里的 `ComponentIr` 表，实例化期那份目录是局部变量、早已销毁。补法不是"传个表进去"，而是三件事一起做：① **目录挂 `Engine`**（`Engine.catalog`，与既有 `Engine.functions` 同形——同样是"求值期需要文档数据"的先例），`instantiate_with` 填好；② **前缀计数器的连续性**：`iN::` 是实例 id 的唯一化手段，行每帧可能重建，若每次重建都从新的计数器开始，第二帧的第 2 行会再拿一次 `i1::`，而第一帧的 `i1::` 元素可能还在树里——两个实例共用一个命名空间，一个实例的绑定写到另一个实例的元素上，**静默错渲染**。用两段式：实例化期从 0 编号（`i1..iN`）、结束时把总数记进 `Engine.prefix_high_water`；行车期从高水位继续，发一个抬高一次、单调不减，于是"发过的前缀永不重发"。**验收标准是 `cargo test -p nui-runtime` 零断言修改**（那 9 处 `i1::` 字面值断言正是实例化段的行为），实测通过；③ **挂载点**：`instantiator` 借用 `catalog`、`body` 借用 `Engine` 其余部分，两处是 `self` 的不相交借用，所以 `Engine::with_instancing(body)` 把目录 `mem::take` 出来、构造 `Instantiator`、再把目录放回。顺带修掉一个既有缺口：`id` 原由 `build_id_index` 在实例化末尾统一扫一遍注册，而行元素是实例化**之后**才插入的，所以行内组件解析不到自己声明的 id——改成**元素入树时即注册**，`build_id_index` 保留为不变量的显式陈述。检查器侧的 `reject_component_in_for` 随之删除（`For` 与 `ListView` 一起，不留不对称，二者共用 `instantiate_row` 与 `binding.prototype`）。**仍在的限制**：行变量在组件体内不可见（组件独立编译，`item` 未声明）——字段须经调用点实参传入。行内的 `Slot` 填空未支持（本次不做） | 2026-10-01 |
| D36 | 滚动条是**渲染期叠加**，不进元素树 | M10 留下的最深一条 V1 缺口：没有滚动条，用户看不出内容还能滚。思路有两条——**A** 引擎按需生成 `Scrollbar` 元素塞进树，**B** 场景期在容器上直接叠加。**选 B**（详案见 D36 本条，那里记作 A'/渲染期叠加，二者同一方案），因为滚动条的全部输入在 walk 到容器时都已在手：容器盒子（lane）、`scroll_y`（当前进度）、以及唯一需要外求的 `max_scroll_y`。选 A 要付出的代价反而是本质性的：滚轮 / hit_test / 布局 / hit_test 偏移 / `Spacer` 不绘制……每一处都得多一个"这是引擎自己塞的元素"的分支，而它**不是一个可交互的节点**（本版不做拖动），没有 id、没有绑定、没有属性，进树只会污染所有按节点语义工作的代码。**唯一接口问题**：`max_scroll_y(engine, tree, id)` 要 `&Engine`（`ListView` 要数模型行），而 `SceneBuilder` 原本够不到 engine——所以 `SceneContext` 加 `engine: Option<&Engine>`，`None` = 不画滚动条（手工建树、无模型的 draw-list 断言就是这个值，`testkit` 走 `SceneContext::without_engine`）。几何与配色抽到 `nui-render/src/widget/scrollbar.rs`（纯函数，13 例单测）：`content = viewport + max_scroll_y`、`ratio = viewport/content`、`thumb_h = clamp(viewport*ratio, MIN_THUMB, viewport)`、`progress = scroll_y/max_scroll_y`，右手 `THUMB_INSET`+`THUMB_WIDTH`。**绘制顺序**：thumb 紧跟在容器背景之后、子节点之前压入，且带容器的 clip——于是它是"内容之下的背景件"而非浮层，且不会越出嵌套容器的视口。配色默认派生自 `fill` 的亮度（`Rec.601`），无 `fill` 用半透明灰。**不做**：淡入淡出、横向滚动条、`Scrollbar` 元素类型——都不改 `max_scroll_y` 语义。（拖动与 track 点击于 **D37** 补上） | 2026-10-01 |
| D38 | `scroll_y` 写入走**轻量管线**，不重排 | 用户报"拖动的时候好卡"——功能对，帧开销不对。**先量**：`crates/nui/tests/scroll_perf.rs`（测量非断言）在 gallery 虚拟列表（10 000 行 × 36dp / 560dp 视口）上得到 6.81 ms/帧，其中 `layout_with_text` 独占 6.39 ms（其余三者相加 <0.4 ms）。**根因**：`scroll_y` 不是布局输入——`grep -rn "scroll_y" crates/nui-layout/src/` **零命中**；滚动是在绘制与命中时做的视口平移（`element_bounds` 逐祖先减去 `scroll_y`，场景 walk 同理），写它改变不了任何盒子，这 6.4ms 在数学上就是白跑的。**解法**：加 `WindowHost::run_scroll_pipeline`，保留 `propagate`（文档可能绑定 `scroll_y`，如"正在显示第 N–M 行"）、保留 `sync_for_nodes`（虚拟列表的可见窗口**确实**随 offset 移动，必须重建行；它内部由 `list_windows` 守卫，不跨行边界的移动早返回 0 重建，所以每像素移动几乎免费）、保留 `widgets.update`（悬浮态写回的是文档能读的属性），**只跳掉 `layout_with_text`**。拖动（`set_scroll_y`）与滚轮共用 `set_direct_with_scroll_pipeline` 一个入口，两条路径不可能漂移。**结果**：6.810 → **0.033 ms/帧（208×）**。**代价与理由**：若某 `scroll_y` 绑定写了 `nui-layout` 会读的属性（`width` / `offset_x`…），要等下一帧完整管线才重排——但这个契约 `scroll_y` 一直就是（渲染期平移不能是布局输入，除非两遍布局，框架从未做过），且虚拟列表的行几何来自 `row_height`（`sync_for_nodes` 直接读），正是要救的场景、不受影响。**不做**：增量布局 / 局部重排（taffy 替换是大工程，见 §13）；本版只是**不跑**不需要的那一档 | 2026-10-01 |
| D39 | 抽出 `nui-tools`：**零领域类型**纯函数下沉 | 与业务无关的算术此前散落在 6 个 crate 里，且**同一表达式被写过多份**——`lerp` 三份（`nui-core::color::lerp_component`、`nui-runtime::animation::interpolate` 内的 `mix` 闭包、`nui-render::rect::linear_rgba` 内的分量换算）、比例映射两份（`nui-render::widget::slider::slider_fraction`、`scrollbar::progress_along`）、step 吸附两份（`nui-runtime::widget::spin::SpinRange::snap`、`nui-render::widget::slider` 拖动）。多份实现不是靠 review 发现的，而是**问出来的**："拖到底部差一像素"和"spin 吸附有浮点噪声"分别是两个 crate 各自修过的 bug。新 crate `nui-tools` **零依赖**（连 `thiserror` 都不要），位于 `nui-core` **之下**，准入规则一句话：**任何签名里都不许出现 nui 领域类型**（`Point` / `Color` / `Value` / `Length`）。按用途分模块（`numeric` / `text` / `hash` / `curve` / `color`），crate 根平铺 `pub use`——与既有 crate 风格一致。**刻意留在原地的**（搬迁会逼出泛型改写，违反"签名与语义不变"）：`Point` 系几何（`nui-core::path` / `earcut`）、`Color` 系调色（`nui-render::widget::state` 的 `lighten`/`darken`/`with_alpha_scale`——`blend` 的 u8 内核下沉了，外壳因收 `Color` 而留守）、`Value` 插值分派（`interpolate` 外壳留守）、dp/百分比解析（`nui-core::length`）。现职色域的 sRGB 传递函数（`rect::linear_rgba`）**未动**：它是 `u8`→线性 `f32` 的闭包，抽出来要么改签名要么只搬走一个闭包，收益不抵风险。**顺带发现的既有问题**：① `byte_to_char` 在**非字符边界**的字节偏移上会 **panic**（`text[..byte]` 切开多字节字符），原注释声称"clamped so a malformed offset cannot panic"只覆盖了"越过末尾"这一种；已记进 `# Panics` 并配 `char_indices().take_while()` 的安全写法；② `inverse_lerp` 的零跨度判据由 `span.abs() < f32::EPSILON` 归并为 `span == 0.0`（可达输入上等价，但严格说是一次判断变更，已注明）；③ 零宽 x 的 bezier 段是**退化**的——`apply_bezier` 在**每个** `t` 都返回 `segment[7]`，不是"停顿"；真正的 hold 要写成"x 跨度非零、y 平坦"的段（已用测试钉住） | 2026-10-01 |

| D40 | 动画帧**不跑布局**——tween 卡顿的根因（paint-only 判定） | 用户报"tween 功能有点卡"。**先量**：`crates/nui/tests/tween_perf.rs`（测量非断言，`--nocapture` 打印），75 元素 / 24 个 `opacity` tween 的小页面，一帧 breakdown：animation tick 0.003ms · propagate 0.000 · row reconcile 0.006 · **taffy layout + text 3.277（99.6%）** · widget states 0.002，FULL 3.290ms/帧。**根因与 D38 同型**：`render_frame → run_frame_pipeline` 每帧无条件跑 `layout_with_text`，而 tween 动画的 `opacity` 根本不是布局输入（`nui-layout` 不读它）——布局在数学上白跑。测试同时**断言**了 12 帧的全部写入都是 paint-only（把优化前提钉成契约，不止是打印）。**修法（数据驱动，比 D38 更进一步）**：D38 按"调用路径"分流（scroll 写走专用管线），D40 按"这一帧实际写了什么"判定——引擎的 change buffer（六条写路径全部流经 `record_change`：Binding/Effect/WhenBlock/TwoWay/Host/Animation）在布局决策点之前已含本帧全部写入，宿主 peek 之：全部落在 `nui-core::props::PAINT_ONLY`（22 个名字：opacity/fill/tint/color/radius/rotation/clip/shadow.\*/gradient.\*/stroke.\*/scroll_y）→ 跳过布局；否则照跑。**三个强制布局的例外**：行重建（`rebuilt > 0`）、宿主 `needs_layout` 粘滞标志（初值 true 保证首帧有盒子；reload 换树、resize 之后置回 true）。**fail-safe 方向是刻意的**：清单漏一个名字只多跑一次布局（安全），写错一个名字才会钉死旧盒子（危险）——所以 `hovered`/`pressed`/`focused` 这类交互镜像**刻意不进**清单（文档绑定可读它们间接影响 width），hover 切换帧多跑一次布局是可接受的代价。**副作用（正面的）**：受益的不止 tween——hover 换色、滚动条拖动帧（若走全管线）、纯换色绑定帧全部自动免布局；动画帧成本 3.290 → ~0.01ms（约 300×，gallery 10k 行规模下是 6.8 → ~0.4ms）。`x`/`y`/`width`/`content`/`font.\*`/`visible` 都不在清单（布局真实输入），动画它们照旧全量重排 | 2026-10-02 |

| D41 | tween **动效塌缩成跳变**：`interpolate` 的跨型数值插值 | 用户报"莫名其妙的动效丢失"。**机制链**：① 实例化给每个 `<-` 绑定属性写 `Int(0)` 占位（`apply_reactive_assignment`，"typed zero seed so reads succeed"）——槽**永远非空**；② 首次求值 target 是 `Float`（如 `opacity <- tween(x ? 1.0 : 0.4)`），`from` 是占位 `Int(0)`；③ `interpolate` **按形状精确配对**（Int/Int、Float/Float、Dp/Dp、Duration/Duration），`(Int, Float)` 落入 `_ => to.clone()` 的 hold 分支——**每一帧都直接返回目标值，整段动画塌缩成一次跳变**。命中场景：一切"无静态默认的 Float/dp 目标 tween"的**第一段**动画（todo 页 swatch 的 opacity 恰好是）；`For` 行重建（虚拟列表滚动）后占位重现，**再丢一次**。既有测试全部带 `state x: Float = tween(...)` 种子（声明落成静态赋值，槽里是 Float），两侧形状齐，所以从未暴露。**修法**：`interpolate` 改为**数值跨型插值**——`numeric_of` 两端都是数值就按数值 `lerp`，结果取目标类型（`numeric_value`）；Duration 保留配对插值；非数值仍 hold。Spring 从未有此 bug：它积分标量 `position` 再经 `numeric_value` 投影，天然跨型。**顺带钉住的防御**：绑定求值分流点对"槽完全无值"的罕见路径（`<=>` 对端清除绑定、宿主清槽）落位目标而非带伪造 `from` 进时钟。**测试**：`a_tweened_slot_without_a_seed_still_lands_and_animates`——无种子的 `opacity <- tween(panel.lit ? 1.0 : 0.4)`，断言首段从占位 `Int(0)` **数值插值**（50ms 时 0.2，而非直接 0.4）、toggle 后 0.4→1.0 正常动画。排错备注：`state` 名撞语言关键字（`on`）、裸标识符走枚举字面量分支（读状态要 `panel.lit` 限定名）| 2026-10-02 |

**节点思想**（设计基座）：一切皆节点——可视节点（Rectangle/Text/Column…）、逻辑节点（Timer/State/Model，不绘制但参与树与绑定）、资源节点（Font/Image）。属性绑定构成数据流 DAG，引擎 = 节点树 + 响应式依赖图 + 每帧脏传播管线（绑定 → 布局 → 绘制）。**不是布局输入的写入不触发重排**（D38 滚动、D40 动画/换色）：帧管线按"这一帧实际写了什么"决定跑不跑 taffy，而不是按调用路径。

**最底层**：`nui-tools`（D39）——零依赖、零领域类型的纯函数层，位于 `nui-core` **之下**。准入规则：任何签名里都不许出现 nui 领域类型（`Point` / `Color` / `Value` / `Length`）。存在的理由是**消除重复表达式**，不是"整理目录"：同一段 `lerp`、比例映射、step 吸附曾在多个 crate 各写一份，各自修过各自的边界 bug。

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

### 3.3b 组件实例化（D16）

文档内 `component` 声明的组件可以在节点位置当类型名用，即真正的组件库：

```qml
component RippleButton {
    property label: String = ""
    property tone: Color = #336699
    signal picked
    Rectangle(fill <- tone, radius = 12dp) {
        Text(content <- label)
        on click => emit picked
    }
}

component App {
    Window(id = root) {
        RippleButton(id = ok, label = "OK") { on picked => n += 1 }
    }
}
```

规则与代价：

- **实例元素就是组件的唯一根节点**，不套壳——布局看到的正是组件声明的那棵树；调用点的 `id`、声明属性、状态机与 `on <signal>` handler 全部落在同一个元素上。代价：**被实例化的组件必须恰好声明一个根节点**（未被引用的入口组件仍可声明多个，各自成为一个树根）。未被任何地方引用的组件不再实例化出多余根节点（此前"声明即产生幽灵根"）。
- **按实例改写 IR + id 命名空间**（D16）。运行时的名字解析是全局平表（`ElementTree::ids`），给求值器加词法作用域链会把树遍历放进每次属性读取的热路径。改为：每个实例取一个 `iN::` 前缀，并把该组件 IR 的副本改写成使用它（`nui_runtime::mangle`）——组件内声明的 `id` 全部加前缀；`Component`/`Root` 裸读（"本组件自己的属性"）改写为对实例元素的显式引用。后者是必须一起解决的第二个单实例假设。方法调用的目标同样改写。
- **`emit` 显式带目标**。组件的信号属于**调用点**，而触发它的 handler 可能在任意内层元素上。
- **引用节点的方法体只接受 handler**。子节点 / 方法体内赋值 / `when` 各自都需要一条"落在被引用树的哪里"的规则，而每条规则都是一个静默惊喜的温床；属性写进参数列表，那里按声明类型检查。
- **参数二义性**：名字被组件声明过就是它的 API（按声明类型检查），否则是实例根节点的普通元素属性（不检查）。同名时声明优先。
- **调用点的静态参数会顶掉组件自身的绑定**（与 D10 效果块赋值同一优先级规则）：静态值是终值，组件自己挂的 `<-` 必须一并退役（引擎侧绑定表与元素槽两侧都要清，否则下一次传播会把它写回来）。
- **自引用是编译错误**（组件引用图上的 DFS 成环检测）。
- **`For` / `ListView` 体内可以引用组件**（D35）。行由引擎实例化、而引擎不携带文档，所以组件目录随实例化交给引擎一并保存，行展开时接着用；行内的组件实例也要与实例化期**共用一套**前缀编号（`iN::`），否则每帧重建会重复发放同一个前缀。限制仍在：**行变量在组件体内不可见**——`Chip { Text(content <- item.label) }` 报 `unknown name \`item\``，因为组件是独立编译的。字段要经调用点传入（`Chip(label <- item.label)`），组件读自己的 `label` 属性。

### 3.3c 组件派生（D30）

`component Child extends Parent` 让一个组件继承另一个组件的声明，**连子树一起**，并且可覆盖：

```qml
component Card {
    property tone: Color = #336699
    signal picked
    Stack(id = self, width = 240dp) {
        Rectangle(id = head, fill <- tone, height = 32dp)
        Slot { }
    }
}

component HighCard extends Card {
    property tone: Color = #ff4444
    Rectangle(id = tail, height = 8dp)
}
```

规则与理由：

- **所有继承在编译期摊平**。派生组件的 IR 在 `bind()` 里就是「父的成员 + 自己的成员」的结果，`ComponentIr::ids` 因而天然含父的 id。于是运行时的 `own` 集合自动正确，`mangle.rs` / `instantiate.rs` **一行都不用改**——这是选摊平而非运行时链式查找的全部理由：运行时多一条派生概念，就要在命名空间、绑定覆盖、id 解析三处同时教它一遍。
- **可覆盖，且覆盖是就地替换**（property / signal / machine 同名者替换原条目）。`ComponentIr::property` 是线性首次匹配，两条同名条目会把父的默认值递回去——就地替换让这条路径写不出那个 bug。
- **派生体节点追加进继承树的 `Slot`**。与调用点填槽同一个机制，只是在编译期解析：继承树 + 派生节点的组合方式不需要第二条语义；`Slot` 的既有规则（至多一个、内容必须有处可去）跨继承链累加检查。
- **派生体节点为空 = 整棵继承**；父树为空（派生宿主已注册类型）= 派生体就是全部。派生宿主类型且不带节点是**错误**（组件必须恰好一个根，D16）。
- **`extends` 不是保留字**（与过渡里的 `from` 同一手法）：只有紧跟组件名后的那个位置才读作子句，叫 `extends` 的组件/属性照常可用。
- **派生不让父成为"被引用"**。`extends_edges` 与实例化用的 `edges` 分开记，否则派生一个入口组件会悄悄把它的幽灵根带出来。
- **`extends` 成环单独检测**（与组件引用环分开报告），消息指出成环路径。
- **父可以声明在子之后**：合并按 `binding_order`（父先于子的 DFS 序）进行，与源码顺序无关。

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
- **组件的交互声明**（D18）：`ComponentDesc::interaction` 声明该组件是控件（`Momentary` / `Toggle` / `Drag` / `Range` / `Spin`、Toggle 翻转哪个布尔、双端控件的两个属性名、是否 Tab 停留），`Engine::interaction` 依次按「元素上的组件名 → 类型名表」解析。之所以必须挂在注册表上：组件实例化后类型名就是它的根节点类型（`component RippleButton` 的实例是 `Rectangle`），名字只存在于元素上，**下游无从分辨**——这正是此前 `RippleButton` 拿不到 hover 的原因。`Toggle` 的两处约定按**属性名**而非类型名走（`selected` 是选中并成组，`checked` 是翻转），组件沿用同名即沿用约定。
- **双端控件**（`WidgetKind::Range`）：一次手势移动离指针更近的那一端，接近度在**值空间**比较（平局取低端）。不能靠两个 `Slider` 叠在 `Stack` 里代替——指针捕获只有一个归属，上面的那个永远赢，另一个够不着。
- **宿主动作**：`root.close()` 让文档自己关窗（引擎置标志、宿主消费）。宿主函数做不到，因为 `HostFunction` 是不可变闭包；`close` 是组件唯一可对自己调用的方法，且**跳过 id 查找**——"组件里没有叫 `root` 的元素"正是它要解决的场景。

## 6. wgpu 渲染设计

- **四个管线，全部实例化批处理**：
  1. **Rect**（承担 ~80% UI）：每实例一个四边形，片元 SDF 画圆角矩形/边框/线性渐变，`fwidth` 抗锯齿；
  2. **Text**：字形图集四边形（cosmic-text shape 后栅格化进 etagere 图集，灰度 AA 起步）；
  3. **Image**：纹理数组 + UV，tint 与九宫格；后台线程解码，按「路径+内容哈希」缓存 GPU 纹理并生成 mipmap；`region.x/y/width/height` 可只绘制纹理的一块（以**纹理像素**为单位——生成的图标集因此只需一次解码与一次纹理上传；九宫格在 region 自身空间内切分后再映射到纹理空间）；
  4. **特效**（M6+）：阴影 SDF 近似、blur 两 pass 高斯 + 离屏纹理、opacity layer。
  - 矩形填充另支持**线性渐变**（`gradient.from/to/angle`）与**径向渐变**（`gradient.kind = radial` + `gradient.center_x/center_y/radius`）——触控水波纹是圆不是线，没有径向渐变就没有它。两者都在 rect 局部坐标里，随元素一起旋转。
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
| **M14**（2 周） | **组件库落地能力**（驱动下游把 M3 组件库搬到 nui）：组件实例化（§3.3b）、`ComponentDesc::interaction`（D18）、具名 cubic-bezier 缓动表（D17）、径向渐变、`Image` region、`root.close()`、双端控件 | 一份纯由 `component` 声明组成的控件库可用：声明式组件 + hover/press/focus + 设计系统动效曲线 |


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

（时长为单人业余节奏粗估，可调。M7–M14 顺序按依赖排：输入完整性 → 视觉完整性 → 大规模数据 → **多端地基** → 可达性/工具链 → 交付 → 组件库落地。）

## 12. 风险与对策

- **绑定成环**：编译期静态检查 + 运行时求值深度上限，双保险。
- **解释器性能**：字节码 + 运行时常量池起步，热点可加内联缓存；UI 表达式普遍很小，预计非瓶颈。
- **文本是最大泥潭**：不自研 shaping，全押 cosmic-text；Editable text 的 IME 单独立项。
- **wgpu 大版本跟进**（30.x，下游常滞后）：渲染层收敛在 nui-render 单 crate，升级面可控。
- **范围蔓延**：DSL v1 严格不碰 JS/闭包/循环；复杂逻辑一律下沉 Rust。

## 13. 未决事项

- 项目与语言命名：工作名 `nui` / `nui-lang`，发布前需查 crates.io 占用情况。
- License 假定 `MIT OR Apache-2.0`（Cargo.toml 已按此填写），如有其他偏好需确认。
- **可变字体轴（除 `wght` 外）未开放**：nui 向 cosmic-text 要的是「一个 face + 一个 weight」，不是「轴上的一个 location」，所以 `FILL` / `opsz` / `wdth` 这类轴文档现在够不着。要开放需要一条从 `font.*` 到 shaping location 的通路，且字形缓存键要带上 location——即 `Typeface` 要再带一个坐标，而不只是三个标量。设计系统里 `FILL` 很常见（选中态图标），所以这是已知缺口而非取舍
- ~~**元素属性名没有编译期词表**~~ **已决（D33，2026-10-01）**：选了方案 (a)，`state name: Type = default` 显式声明元素状态，词表校验随之接通。`Text(contnt = "x")` 现在是编译错误并附 `did you mean content?`；未声明的元素状态（如旧的 `Column(id = p, clicks = 0)`）报 `has no property \`clicks\`` 并提示 `declare it with state clicks: <Type> = <value>`。保留下来的历史说明：
  - 曾被绕过的原因：属性名空间是**故意共享**的——平台惯用法就是把每页状态挂在元素上（`Column(id = containers_page, clicks = 0)`，读 `containers_page.clicks`），而宿主注册的组件又能带自己的属性，所以「这个名字是内建属性，还是这个元素自己的状态」编译器无从判断。(a) 用一次声明消掉这个歧义，(b) 用前缀消掉；选 (a) 的理由见 D33。
  - 已补的一小块（同一目标里没有歧义的部分）：效果语句里的单名调用现在要对着宿主词表校验，`on click => togle()` 从「运行时静默失败」变成编译期报错；宿主函数与宿主命令也在编译期分开（命令不能当值用）
  - **类型名已补完**（D31 / D32）：`Buton(...)` 是编译错误并附最近名字建议。这一半之所以先做，是因为类型名**没有**共享命名空间问题——节点类型由运行时查表，查不到就是静默消失，不存在「作者本来想表达别的」的可能。宿主注册的类型走 `Registry::vocabulary()` 上报（此前漏了这一步，宿主类型会被误报）
  - 词表本身 `nui-core::props::TABLE`（名 + 类型 + 默认值）本次一并补齐了此前缺失的真实条目：`key`（进 `UNIVERSAL`，reconciler 在任何元素上读它）、`ListView.row_height`（虚拟化的固定行高）、`Polyline.color` / `Arc.color`（`stroke_of` 优先读这个名字）、`TextInput.password` / `TextInput.reveal`（`is_password()` 是两者的合取）、`Canvas` 的 paint 面（`clip` 等）。补的是**运行时确实会读**的名字——这是这张表作为单一来源应有的样子，同时把 gallery 各页的内联状态迁移到 `state` 声明

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
  - 已知 v1 限制:行高均匀（可变行高虚拟化需测量缓存）、~~无滚动条~~ **已于 M15.2 / D36 补上**、滚轮只钳下界（内容总高未知，现已由 `max_scroll_y` 同时提供上界）
- [x] **M10.5 追加（2026-09-25）**：playground 演示 + 离屏截图模式（`--snap` 写 PNG 到 snapshots/,无显示环境可验证渲染）+ 点击**冒泡**（`Engine::emit_bubble`,按钮文字不再挡点击）。截图审查暴露并修复 **4 个真实渲染 bug**——此前 demo 全是浅嵌套+高饱和色,全部漏检：
  1. **布局坐标父相对**：taffy location 是相对父级的,场景/hit_test 却当绝对坐标——嵌套元素整体错位;layout 回写改为沿树累加绝对坐标
  2. **颜色空间双重编码**：sRGB 颜色被当线性值写入 sRGB target,整体变亮;新增 `linear_rgba`(sRGB→线性→premultiplied)统一用于 rect/text/image 提交;clear 色同理（wgpu::Color 对 sRGB target 是线性）
  3. **swash placement 符号**：placement.top 是 Y 向上为正（基线→字形顶）,屏幕坐标 Y 向下需取反——否则全部字形画到基线下方
  4. **flex-shrink 挤压**：taffy 默认 flex_shrink=1,虚拟化列表的 30000dp Spacer 把整容器内容压缩归零;改为 `flex_shrink: 0`（Qt-Quick 语义:元素保持自然尺寸,溢出容器）
  - 教训:像素截图审查是渲染管线的『验收测试』,.offscreen 断言（红 255/通道占优）对伽马与嵌套错位完全不敏感
- [x] **M14 完成**（2026-09-27）：**组件库落地能力**——由下游的 M3 组件库移植驱动,七项各自独立提交
  - **组件实例化**（§3.3b）：类型名命中同文档 `component` 即为引用,checker 按声明类型检查参数/信号,运行时按实例展开。三个决定:实例元素**就是**组件的唯一根节点（不套壳,故被实例化的组件必须恰好一个根,未被引用的组件不再产生幽灵根）;作用域用 **id 命名空间改写**而非求值器作用域链（D16）;引用体只接受 handler。连带:实例改写覆盖方法调用目标、`emit` 新增显式目标、组件内状态机一并改写、`Element::remove_subtree` 清理实例名
  - **`ComponentDesc::interaction`**（D18）：`Engine::interaction` 按「元素组件名 → 类型名表」解析,注册组件因此拿到 hover/press/armed/focus/disabled + 键盘激活 + Tab 停留;`Toggle` 的翻转/选中约定改按属性名(`checked`/`selected`),组件沿用同名即沿用约定
  - **具名 cubic-bezier 缓动表**（D17,`nui_runtime::easing`）：12 条设计系统曲线;线段显式写出**两个端点**(M3 emphasized 是两段在 (1/6, 0.4) 相接,中段起点既非原点也非终点);进度按 **x 反解**(Newton + 二分兜底——`standard-decelerate` 两个控制点都在 x=0,`x(u)=u³` 在原点斜率为零,Newton 无法起步);测试记录两条读数为 CSS 曲线的后果:spatial 曲线**过冲**(y 控制点 >1)且 `spatial-fast`/`spatial-default` 因第二控制点低于第一点而在过冲肩部**微微回落**;`emphasized` 不过冲;两半 emphasized 归一化后恰是 accelerate/decelerate(恒等式测试锁住)
  - **径向渐变**：`gradient.kind = radial` + `gradient.center_x/center_y/radius`;中心与半径占用原结构体 padding(`_pad2` 与 `gradient_params.w`),实例仍 176B、`gradient_from` 仍在 offset 128（两条都有断言——M9 的教训是 WGSL 对齐错误是静默的）;离屏像素测试 5 例
  - **`Image` region**:`region.x/y/width/height`(纹理像素,四个全有或全无),九宫格在 region 空间切分后由纯函数 `remap_uv` 映射回纹理空间;离屏像素测试 4 例
  - **`root.close()`**:引擎置标志、宿主消费;`close` 是组件唯一可对自己调用的方法且跳过 id 查找
  - **双端控件**(`WidgetKind::Range`):一次手势移动值空间里更近的一端,平局取低端;未设的两端按 `min` 读,不除以缺失值
  - M14 验收：`cargo fmt --all -- --check` / `clippy --workspace --all-targets -D warnings` / `test`（**652 全绿**,基线 566）/ `build --release` 全通过;D13 grep 通过;`examples/gallery --snap` 14 页出图与改动前一致(无回归)
  - 已知限制:~~组件引用暂不支持出现在 `For` 体内（行由引擎实例化，引擎不携带文档）~~ **已于 M15.1 / D35 支持**;缓动曲线名不做编译期校验（`easing` 是 `Enum`,拼错静默降级为默认曲线,与既有行为一致）;`Path` 填充仍无抗锯齿;未做系统调色板跟随（`nui-winit` 无 `QStyleHints::colorScheme` 对应能力）

- [x] **M14.2 追加（2026-09-30）**：为组件库落地补齐两块地基，由下游一次完整移植驱动
  - **组件体可读入口组件的元素 id**（D20）：`.nui` 没有 `import`，一个应用的所有组件都在同一文档里，而每个控件都要读窗口上的明暗开关。检查器的名字解析在组件自身 id 之后加一层文档级兜底。两处踩过的坑都留在了代码注释里：兜底集合必须在 `referenced` **之后**收集（否则被实例化组件的 id 也会进来，而那些 id 运行时已改写，裸名解析到哪个实例取决于当次实例化——这类错误编译通过、渲染错）；兜底只作用于 `a.b` 成员访问，裸标识符走 `check_ident` 的枚举字面量分支，不动。5 条新测试写死了「允许的性质」：能读到文档 id、自身 id 优先、**不传递**、拼错仍报错、实例的私有 id 不泄漏
  - **`nui_text::TextSystem::rasterise(cache_key)`**：构建期烘焙工具要拿到字形的覆盖率位图，而运行时 atlas 只给 placement。多一个方法，形状与 `glyph_quad` 同一条光栅化路径，所以「烘焙出来的图标」与「排版出来的字形」是同一批像素。另加 `with_single_font(bytes)`——烘焙工具要单一字体，多字体集合会让缺失的连字悄悄用替代字体排出字形、烘出一整张字母表进图集
  - 验收：`cargo fmt --all -- --check` / `clippy --workspace --all-targets -D warnings` / `test`（**659 全绿**，基线 654）/ `build --release` 全通过；D13 grep 通过；`examples/gallery --snap` 14 页出图**逐字节不变**
  - 教训：第一个版本把文档级 id 收集放在了 `referenced` 之前，于是「实例的私有 id 不可见」这条测试直接失败——它证明了这条性质是可测的，也证明了它一开始就是错的
- [x] **M14.3 追加（2026-09-30）**：`Rectangle` 的 `border.width` / `border.color`
  - 描边走 `push_rect_outline`（widget part 的 Surface 边框已经在用同一条路径），不新增 GPU 代码；`RectInstance` 的 `border_width`/`border_color` 字段本就存在
  - 提前返回的顺序是这件事的全部：`fill`/`gradient` 都没有时元素被当作透明容器而**什么都不画**。只有描边的元素——M3 的 outlined button、text field 描边、checkbox 边框——正好落进这个分支，所以描边不是画错而是消失。6 条测试写死：填充+描边各有一次绘制、**无填充也能画**（两种写法：显式 transparent 与根本没有 `fill` 键）、只写一半不画、`opacity` 折进描边（否则淡化的控件会留一道硬边）、既无填充也无描边仍什么都不画（常见路径不能回归）
  - 描边画在框内且不影响布局盒子，与 `shadow.*` 不影响布局一致
  - 验收：fmt / clippy `-D warnings` / `test`（**665 全绿**，基线 659）/ `build --release` 全通过；`examples/gallery --snap` 14 页逐字节不变
- [x] **M14.4 追加（2026-09-30）**：搭一套有状态的真实组件库时撞出的五个缺陷，全部是**静默**的
  - **D22 改写集合**：实例化改写原来无条件给所有 `PropertyTarget::Id` 加前缀。加了 D20 之后需要区分「组件自己的 id」和「文档级 id」，于是引入 `own` 集合——结果发现 `ComponentIr.ids` 从来没被填过。测试夹具 `component_ir()` 手工填了 `ids` 才让旧测试继续通过，这正是它没被发现的途径：**夹具比实现更配合**
  - **D23 入口组件**：入口组件的 `Component(name)` 原先落到文档第一个 root。单入口文档看不出问题（本项目之前就是），多入口就错
  - **D24 `=` 读属性**：组件引用实参读父属性（`Wrapper(kind = kind)`）这类写法被运行时静默丢弃。提升为绑定后，文档里 `Stack(id = self, width = width)` 这类「组件属性与元素属性同名」的写法变成自环，检查器的边记录若仍看语法 op 就查不出来——两处必须同源，于是边记录也改用 `init_kind`
  - **D25 偏移**：`Stack` 是单格网格，子元素全落在同一格；没有偏移就只有一个位置。顺带发现 `nui-layout` 的 `mod tests` 从未 `#[cfg(test)]`，新增模块让它的辅助函数在 lib 构建里变成死代码，`-D warnings` 报了出来——顺手补上（既有问题，但此前没暴露）
  - 三条新测试钉住的都是**不出现**的性质而非出现的：文档级 id 不被改写、自己的 id 仍然被改写、组件包装组件不循环、控件组件的三元 skin 会求值
  - 验收：fmt / clippy `-D warnings` / `test`（**676 全绿**，基线 665）/ `build --release`；`examples/gallery --snap` 14 页逐字节不变
  - 教训：这五个缺陷里有四个的失败形态是「渲染不出来」而不是「报错」。`--check`（只编译）一次都没抓到，全靠 `--snap` 出图和逐页看。**离屏出图是这类缺陷唯一的检查手段**，所以它必须真的能跑、并且真的被看
- [x] **M14.5 追加（2026-09-30）**：`Slot`——组件库一直缺的那块，以及**两个误报成「自环」的名字冲突**
  - **D26/D27 `Slot`**：组件声明 `Slot`，调用点用子节点填。「页面」「区块」这类**容器**组件因此重新写得出来，页面不再逐处复述外层的 Scroll、定宽 Column 和标题—正文结构，少掉约一百行不带信息的布局样板
  - **落点是在实例化之后按元素类型找的**，不是重读组件 IR——slot 可以在子树的任何位置（通常嵌在给内容一个框的 `Column` 里，那正是它存在的理由）。内容是**引用节点**的子节点，所以不加实例前缀：slot 里的 `id` 属于调用点
  - **D28 是搭这个特性时撞出来的**，而且不是 slot 特有的：环图按名字索引，而名字在三个地方指向不同的槽。`Column(width = width)` 这种最普通的写法被报成 `width <- width`——**这正是 slot 组件的写法**，所以没有它 slot 根本用不了。根节点那一处是真自环，保留原键；两个假的各给一个没人写的哨兵名。两条测试成对：嵌套可以同名，根节点不行
  - 另一处：`collect_node_references` 在组件引用处 `return`，于是 slot 里的组件全都算「未被引用」——而未被引用的组件会被运行时当成**树根**实例化。把整份内容塞进一个带 slot 的容器之后，每个组件都出现两次：一次在该在的位置，一次在左上角、带着声明默认值，**没有任何诊断**（那个多余实例本身是正确的组件的正确的实例，于是所有「看组件」的检查都通过，只有一个多余的形状）。现在引用节点会继续走它的 body
  - 验收：fmt / clippy `-D warnings` / `test`（**685 全绿**，基线 676）/ `build --release`；`examples/gallery --snap` 14 页逐字节不变
- [x] **M15 追加（2026-10-01）**：`fn` 用户函数（D34）+ 属性名词表校验（D33，§13 收口）
  - **`fn`（D34）**：文档级 `fn name(p: Type, q: Type = default) -> Type { ... }`，表达式与效果块里都能调。语法侧新增 `FunctionDecl` / `Parameter` / `Statement::Return` / `Statement::Expr` 与 `-> `（`Punct::RArrow`）、`fn`、`return` 三个 token；编译侧 `collect_function_signatures` 按文档序建表（**只能调前面的**，所以循环调写在语法上就不成立，不需要环检测），`bind_function` 在绑定期把形参声明为局部变量；字节码侧 `TypedExpr::UserCall` / `Effect::UserCall` / `Effect::Return`；运行时 `Engine::call_user_function` 压/弹一帧 locals，`run_effect_list` 让 `return` 提前收束并把 `If` 分支的返回值透传上来。两处刻意的类型规则：Void 函数不能当值用（`Text(content <- bump(1))` 报错，但 `bump(1)` 作语句可以），具名实参会被重排进形参声明序（`defaulted` 掩码记录哪些是补的默认值）
  - **属性名词表校验（D33）**：`state name: Type = default` 与 `property` 同族，元素自己的状态必须**显式声明**；由此属性名这一半也能查了——不在 `TABLE` 里且没有 `state` 声明的单段名是编译错误，附最近属性名建议（`did you mean content?`）或 `declare it with state ...` 的提示。声明落成一条 `Static` 赋值，运行时看不到新概念；检查器只在**内建类型**上查（组件引用与非内建类型跳过），只查**单段名**（多段名走既有 id 解析路径）
  - **词表补齐**：校验一接通就暴露了 `nui-core::props::TABLE` 的真实缺口——补的是运行时**确实会读**的名字：`key`（进 `UNIVERSAL`）、`ListView.row_height`、`Polyline.color` / `Arc.color`、`TextInput.password` / `TextInput.reveal`、`Canvas` 的 paint 面。`Waveline.color` 是原本就有的显式条目，加进 `PAINT` 会触发「同类型声明两次」的自检，所以 `color` 留在各类型自己的 extras 里（`Text`/`Button`/`CheckBox`/`Slider`/`SpinBox` 也都显式声明它）
  - **gallery 迁移**：8 个页面的内联状态（`counter.count` / `containers.clicks` / `inherit.clicks` / `widgets_page.*` / `form_page.*` / `text_fields_page.*` / `transform.spin` / `waves.tick`）全部改成 `state` 声明，读点写法一行未动——这正是选 (a) 而非 (b) 前缀的原因
  - **测试迁移**：运行时集成测试里的探针槽（`Text(x <- ...)` / `Text(id = out, value <- ...)`）改成 `state` 声明 + 保留响应式绑定（静态默认值会冻住探针，而这些测试都是在观察**变化**）
  - 验收：fmt / clippy `--workspace --all-targets -D warnings`（**零警告**）/ `test`（**804 全绿**，基线 685）；`examples/gallery --snap` 仍出图。四个新增的 vocabulary 测试钉住的是「报错要说什么」：拼错给最近名、远拼错给 `state` 提示且**不**给建议（错的建议比没有更糟）、组件自己的 `property` 不被误报、未声明的元素状态报错而声明后通过
  - 已知的既有偶发：`nui-render/tests/offscreen_image.rs` 的 atlas 测试会把 PNG 写到 `std::env::temp_dir()`，而该路径只由 region 变体命名——两个测试并发时互相覆盖，本沙箱的 `/tmp` 又是 10MB tmpfs。约 1/6 概率失败，**与本次改动无关**（`crates/nui-render/` 零 diff，干净树上也复现过）
- [x] **M15.1 追加（2026-10-01）**：`For` / `ListView` 体内的组件引用（D35，第一优先级第 3 项）
  - **问题**：行由引擎每帧拉起（`instantiate_row`），而展开组件要文档里的 `ComponentIr` 表——实例化期那份目录是局部变量，函数返回即销毁。所以缺的不只是表，还有**前缀计数器的连续性**：`iN::` 每帧从 0 重发就会与上一帧仍在树里的元素撞号
  - **目录挂 `Engine`**（`Engine.catalog`，与 `Engine.functions` 同形）；**前缀两段式**——实例化期 `i1..iN`，结束时总数记入 `Engine.prefix_high_water`，行车期从高水位续发、单调不减。`Engine::with_instancing(body)` 把目录 `mem::take` 出来给 `Instantiator` 借用、再把目录放回（`self` 的不相交借用）
  - **顺带修掉的既有缺口**：`id` 原由 `build_id_index` 在实例化末尾统一注册，而行元素是实例化**之后**插入的，因此行内组件解析不到自己声明的 id。改为**元素入树时即注册**
  - **检查器**：删 `reject_component_in_for`（`For` 与 `ListView` 一起撤，二者共用行路径，不留不对称）。留一条守护测试：`For` 体内**未知类型**仍报错——被删的是"组件引用"这一条规则，不是通配
  - **两条计划外的发现**（都是写测试才暴露的，都写进了测试注释）：① `find_row_scope` 的向上提升循环在**行内组件**这一形状下是空转——行变量在组件体内不可见（组件独立编译，`item` 未声明），所以 `item.field` 只可能写在调用点，而调用点实参的绑定就挂在实例元素上，实例元素**就是**行根，读者与行根同一，提升无事可做。手工把该循环禁用后相关测试仍全绿，据此把测试的说明改成陈述这个事实，而不是假装它覆盖了那个风险；② 由此得出一条必须记下的语言限制：**行变量在组件体内不可见**，字段须经调用点实参传入
  - **验证测试的有效性**（而不是只看它绿）：把 `with_instancing` 的起点从高水位改回常量 0，`for_components.rs` 立刻 4 例失败，症状正是 `i1::self` 被两行共用；恢复后 11 例全绿。证明前缀连续性这条机制是**承重**的
  - 验收：`cargo fmt --all --check` / `clippy --workspace --all-targets -D warnings`（**零警告**）/ 逐 crate `test`（**815 全绿**，基线 804；新增 `tests/for_components.rs` 11 例，`components.rs` 替换 1 例故总数不变）；**`cargo test -p nui-runtime` 零断言修改**——D35 §2.2 的验收标准达成，那 9 处 `i1::` 字面值一字未动
  - 未做：行内组件的 `Slot` 填空、行级增量复用（keyed diff）、`ComponentIr` 的 `Arc` 优化
- [x] **M15.2 追加（2026-10-01）**：滚动条（D36，第一优先级第 1 项）
  - **缺口**：`Scroll` / `ListView` 早已能滚（`max_scroll_y` 9 例单测、滚轮路由、场景平移+裁剪都在），唯独没有滚动条——用户看不出内容还能滚。这是 M10 留下的最深一条 V1 限制
  - **选址**：滚动条做成**渲染期叠加**（在容器 walk 到时直接 push 一个 rect），而不是引擎生成 `Scrollbar` 元素塞进树。理由是后者要为"引擎自己塞的元素"在滚轮 / hit_test / 布局 / `Spacer` 不绘制等每一处加分支，而它本版并不可交互（无 id、无绑定、无属性），进树只污染按节点语义工作的代码
  - **几何与配色**：抽成 `crates/nui-render/src/widget/scrollbar.rs` 的纯函数 `scrollbar_metrics` + `thumb_color`（13 例单测）。`content = viewport + max_scroll_y`；`ratio = viewport/content`；`thumb_h = clamp(viewport*ratio, MIN_THUMB(24), viewport)`；`progress = scroll_y/max_scroll_y`；`thumb_left = right - THUMB_INSET(2) - THUMB_WIDTH(4)`。thumb 色默认派生自 `fill` 亮度（`Rec.601`），无 `fill` 用半透明灰
  - **唯一的接口问题**：`max_scroll_y` 要 `&Engine`（`ListView` 数模型行），而 `SceneBuilder` 原本拿不到。故 `SceneContext` 加 `engine: Option<&Engine>`；`None` = 不画（手工建树 / 无模型的 draw-list 断言就是这个值，`testkit` 走 `SceneContext::without_engine`）。`App::draw()` 传 `Some(&self.engine)`
  - **绘制顺序**：thumb 紧跟在容器背景之后、子节点遍历之前压入，并带容器自身的 clip —— 于是它是"内容之下的背景件"，不是浮层，且嵌套容器里的滚动条仍被每一层祖先视口裁住
  - 验收：`cargo fmt --all --check` / `clippy --workspace --all-targets -D warnings`（**零警告**）/ 逐 crate `test`（**828 全绿**）
  - 端到端测试（`scene.rs`）：能滚的 `Scroll` 有 thumb 且位置/高度/裁剪/来源元素都断言到位；**正好装下**的 `Scroll` 不多出 rect（回归：别让每个 `Scroll` 都长一条）；`ListView` 按模型行数算 thumb（100 行×10dp / 50dp 视口 → 精确比例 2.5dp 被 `MIN_THUMB` 钳到 24）；无 engine 时不画
  - **写测试暴露的两条事实**（都写进了测试注释）：① 手工建树**没有布局回写**，子元素的 `y` 不设置就是 unset，而 `content_height` 按 `child.y + child.height` 量——于是内容高读数 0、不出 thumb。真实管线永远先跑布局，所以这是测试建树的注意点而非代码缺陷；② `max_scroll_y` 的 `limit` 确实需要 `&Engine`，无法在无 engine 时伪造，`Option` 是诚实的形状
  - 未做（D36 明列的非目标）：~~拖动 thumb~~（**已于 M15.3 / D37 补上**）、淡入淡出、横向滚动条、`Scrollbar` 元素类型——且都不改 `max_scroll_y` 语义
- [x] **M15.3 追加（2026-10-01）**：滚动条可鼠标操作（D37）
  - **做了什么**：拖动 thumb、track 点击跳转、拖出容器仍跟随（指针捕获）。`nui-render/src/widget/scrollbar.rs` 从 13 例单测增到 25 例；新增 `crates/nui/tests/scrollbar_drag.rs` 12 例端到端
  - **几何正反同源**：私有 `thumb_travel` 被 `scrollbar_metrics`（画）与 `scroll_y_from_thumb_top`（拖）共用。不变量测试 `the_drag_mapping_round_trips_the_painter_placement` 扫了 3 个视口高 × 4 个 limit × 11 个位置，断言"画出 thumb 再读回位置 == 原 offset"
  - **hit 与画同源**：`scrollbar_hit` 内部调 `scrollbar_metrics`，所以抓取区**永远**包含画出的 thumb；`the_bar_the_user_grabs_is_the_bar_that_is_drawn` 在 9 个滚动位置取样 thumb 的顶/中/底边，全部必须判为 `Thumb`
  - **host 侧**：`WindowHost.scrollbar_drag`（容器 id + `grab_offset`），与 `PointerGesture` 并列——滚动条不是元素，无法用元素级捕获。按下时 `begin_scrollbar_drag` 抢在元素命中之前；移动时不做命中判定（活拖动拥有指针）；释放时消费掉不触发 `click`
  - **一个真 bug（写测试时抓到的）**：track 点击原先 `return write_scroll_for_thumb_top(...)`，于是当跳转值**恰好等于**当前 offset 时返回 false → 按下穿透到内容、arm 了滚动条后面的按钮。改成无条件消费。已加 `a_press_on_the_bar_is_always_consumed` 钉住
  - **变异验证**（不是只看绿）：① 把 `scroll_y_from_thumb_top` 的 progress 反写成 `1-p` → 4 例失败，`the_drag_follows_the_pointer_monotonically` 报"step 1: 300 -> 265 went backwards"；② 把 thumb 命中区缩窄 20dp（模拟"画的与抓的不一致"）→ 3 例失败，`the_bar_the_user_grabs_is_the_bar_that_is_drawn` 报"scroll_y 0: the drawn thumb at y=12.5 is not grabbable"。两次都精准命中
  - **`travel == 0` 的处理是我改过一次的判断**：最初在 `thumb_travel` 里把 travel==0 当退化情形返回 `None`，**破坏了两个既有单测**（`the_minimum_thumb_never_exceeds_a_short_container`、`a_thumb_is_never_taller_than_its_lane`）。正确语义是：容器比 `MIN_THUMB` 矮时 thumb 就是整条 lane，滚动条**仍该画**（内容确实溢出），只是无路可走、进度恒 0。改为保留 `Some` + `progress_along` 在 travel≤0 时返回 0
  - 验收：`cargo fmt --all --check` / `clippy --workspace --all-targets -D warnings`（**零警告**）/ 逐 crate `test`（**846 全绿**，M15.2 时 824）
- [x] **M15.4 追加（2026-10-01）**：滚动帧**不跑布局**——拖动卡顿的根因（D38）
  - **症状**：M15.3 的拖动功能正常，但"拖动的时候好卡"
  - **先量再改**：新增 `crates/nui/tests/scroll_perf.rs`（**测量而非断言**，`--nocapture` 打印）。它按 gallery 的方式建文档，再按侧栏的方式把 `root.page` 切到 list 页（否则页面 `visible=false`、布局跳过、`max_scroll_y` 读 0，什么也量不到），扫 60 帧逐个阶段计时。**数字**（gallery 虚拟列表，10 000 行 × 36dp / 560dp 视口，225 元素）：

    | 阶段 | ms/帧 |
    |---|---|
    | binding propagate | 0.077 |
    | ListView 行重建 | 0.265 |
    | **taffy layout + text** | **6.387** |
    | widget states | 0.051 |
    | **合计** | **6.810** |

  - **根因**：`scroll_y` **根本不是布局输入**——`nui-layout` 一次都没读过它（`grep -rn "scroll_y" crates/nui-layout/src/` 零命中）。滚动是**渲染期/命中期的视口平移**（`element_bounds` 减去各祖先的 `scroll_y`，场景 walk 同理），写它改变不了任何盒子。可拖动每移动一次鼠标就付一遍完整的 6.4ms 重排 + 全字符串重排版
  - **修法**：`WindowHost::run_scroll_pipeline`（D38）——保留 `propagate`（文档可能绑定 `scroll_y`）、保留 `sync_for_nodes`（虚拟列表的可见窗口**确实**随 offset 移动；且它内部由 `list_windows` 守卫，不跨行边界的移动直接早返回重建 0 行）、保留 `widgets.update`（悬浮态是文档能读的绑定），**跳掉 `layout_with_text`**。拖动与滚轮共用 `set_direct_with_scroll_pipeline` 一个入口
  - **结果**：6.810 ms/帧 → **0.033 ms/帧（208×）**
  - **代价（写进代码注释）**：若某文档的 `scroll_y` 绑定写了 `nui-layout` 会读的属性（`width` / `offset_x`…），要到下一帧完整管线才会重排。但 `scroll_y` 一直以来就是这个契约——渲染期平移无法成为布局输入（除非两遍布局，框架从未做过）；虚拟列表的行几何来自 `row_height`（`sync_for_nodes` 直接读），正是本优化要救的场景，不受影响
  - 验收：`cargo fmt --all --check` 干净 / `clippy --workspace --all-targets`（**零警告**）/ 逐 crate `test`（**846 全绿**）
  - 未做：逐帧动画翻页、拖动中的高亮态（原生工具条有，本版不做）、横向滚动条
- [x] **M15.5 追加（2026-10-01）**：抽出 `nui-tools`——零领域类型纯函数下沉（D39）
  - **新增 crate**：`crates/nui-tools/`，零依赖，位于 `nui-core` 之下；`Cargo.toml` 保留空 `[dependencies]` 段（与既有 crate 风格一致，且空段本身就是"零依赖"的显式声明）
  - **模块划分**（按用途，5 个文件 1119 行）：
    - `numeric.rs`（295）`lerp_f64` / `lerp_f32` / `inverse_lerp` / `progress` / `step_grid` / `decimal_places` / `round_to_grid`
    - `text.rs`（282）`edit_distance` / `is_integer` / `is_float` / `is_email` / `char_to_byte` / `byte_to_char`
    - `curve.rs`（294）`BezierSegment` / `BezierCurve` / `apply_bezier` / `solve_segment_x` / `bezier_x` / `bezier_y` / `bezier_axis` / `bezier_x_slope`
    - `color.rs`（138）`hex_value` / `component_to_u8` / `blend_u8`
    - `hash.rs`（72）`content_hash`（FNV-1a）/ `cache_key`
  - **搬迁的 12 处调用点**：`nui-core::color`（3 个本地 helper 删除，`Color::lerp` 4 处改调 `lerp_f32`）· `nui-runtime::easing`（6 个 bezier 函数 + 两个类型定义下沉，原地留 `pub use` 转口，`Easing`/`CURVES` 不动）· `nui-runtime::animation`（`mix` 闭包改调 `lerp_f64`，`Value` 分派留守）· `nui-runtime::text_input`（4 个校验函数下沉，`validates` 留守）· `nui-runtime::widget::spin`（`snap` 改调 `step_grid` + `round_to_grid`，`format_value` 改调 `decimal_places`）· `nui-compiler::check`（`edit_distance` 下沉，顶层 + 两个测试模块各加一行显式 import）· `nui-render::image`（`cache_key`/`content_hash` 换成 `pub use`，`nui_render::cache_key` 路径对 gallery/host/offscreen 三处调用方仍然解析）· `nui-render::widget::slider`（`slider_fraction` 外壳留守，体改调 `inverse_lerp`）· `nui-render::widget::scrollbar`（`progress_along` 删除，改调 `progress`）· `nui-render::widget::state`（`blend` 删除，`lighten`/`darken` 改调 `blend_u8`）· `nui-text::system`（`byte_to_char` 下沉）
  - **合并的三处重复**（用户明确选择接受此处的行为风险）：`lerp` 三份 → `lerp_f64`/`lerp_f32`；比例映射两份 → `inverse_lerp`（`slider_fraction` 原用 `span.abs() < f32::EPSILON` 判零跨度，归并为 `span == 0.0`，可达输入上等价，已注明）；step 吸附两份 → `step_grid`
  - **刻意留守的边界**（搬迁会逼出泛型改写，违反"签名与语义不变"）：`Point` 系几何（`nui-core::path` / `earcut`）· `Color` 系调色（`state::lighten`/`darken`/`with_alpha_scale`）· `Value` 插值分派（`interpolate` 外壳）· dp/百分比解析（`nui-core::length`）· sRGB 传递函数（`rect::linear_rgba`，收益不抵风险）
  - **顺带指出并钉住的三条既有问题**：`byte_to_char` 在**非字符边界**的字节偏移上 panic（原注释只覆盖"越过末尾"；已记 `# Panics` + `catch_unwind` 测试 + 安全写法）· `inverse_lerp` 零跨度判据变更 · 零宽 x 的 bezier 段是退化的（每个 `t` 返回 `segment[7]`，不是停顿；真正的 hold 由 `a_flat_y_segment_is_a_real_hold` 钉住）
  - 验收：`cargo fmt --all` 干净 / `clippy --workspace --all-targets`（**零警告**）/ 逐 crate `test --release`（**899 全绿，0 失败**；基线 846，净增 53 = `nui-tools` 42 例 + `nui-compiler` 新增 2 例 − 替换 1 例 + 其余既有例数不变）
- [x] **M15.6 追加（2026-10-02）**：tween 卡顿——动画帧不跑布局（D40，paint-only 判定）
  - **症状**：用户报"tween 功能有点卡"。动画时钟本身每 tick 只需微秒级，嫌疑在帧管线
  - **先量**：新增 `crates/nui/tests/tween_perf.rs`（测量 + 一个契约断言）。75 元素 / 24 个 `opacity` tween，逐阶段计时：animation tick 0.003ms · propagate 0.000 · row reconcile 0.006 · **taffy layout + text 3.277ms（99.6%）** · widget states 0.002；FULL 3.290ms/帧。**根因与 D38 同型**：`run_frame_pipeline` 每帧无条件跑 `layout_with_text`，而 `opacity` 不是布局输入——布局白跑
  - **契约断言**：测试钉住"12 帧的全部写入都是 paint-only"——优化前提成为被测试守护的不变量，而非一次性的观测
  - **修法（数据驱动）**：引擎的 change buffer 本就汇聚六条写路径（Binding/Effect/WhenBlock/TwoWay/Host/Animation），宿主在布局决策点 `pending_changes()` peek 之：全部落在 `nui-core::props::PAINT_ONLY`（22 名字，含 `scroll_y`）→ 跳过布局。与 D38 的差别：D38 按调用路径分流（scroll 写走专用管线），D40 按帧内容判定——**同一条全量管线自动对 tween 帧、hover 换色帧、纯换色绑定帧免布局**，无需每类交互一条专用管线
  - **强制布局的例外**：`rebuilt > 0`（新行需要盒子）；宿主 `needs_layout` 粘滞标志（初值 true 首帧布局；`reload` 换树、`resize` 后置回）
  - **fail-safe 取向**：清单**漏**名只多跑一次布局（安全）；**错**名才钉死盒子（危险）。故 `hovered`/`pressed`/`focused` 刻意不进（文档绑定可读它们间接影响 width）——hover 切换帧多跑一次布局，可接受
  - **效果**：tween 帧 3.290 → ~0.01ms/帧（约 300×；gallery 10k 行规模为 6.8 → ~0.4ms）。`x`/`y`/`width`/`content`/`font.*`/`visible` 不在清单，动画它们照旧全量重排——正确性优先
  - 验收：`cargo fmt --all` 干净 / `clippy --workspace --all-targets`（**零警告**）/ 逐 crate `test --release`（**900 全绿，0 失败**；基线 899 + tween_perf 1 例）
- [x] **M15.7 追加（2026-10-02）**：tween 动效塌缩成跳变——`interpolate` 跨型数值插值（D41）
  - **症状**：用户报"莫名其妙的动效丢失"。根因是 `interpolate` 按形状精确配对：实例化的 `Int(0)` 占位 × `Float` 目标落入 hold 分支，每帧直接返回目标值，整段动画塌缩成一次跳变
  - **修复**：`interpolate` 数值跨型插值（`numeric_of` 双端数值 → `lerp` → `numeric_value` 取目标类型）；Duration 配对保留；非数值 hold 不变。绑定分流点补"槽完全无值则落位"的防御
  - **测试**：无种子 tween 端到端（首段数值插值 0→0.4、toggle 0.4→1.0）
  - 验收：`cargo fmt --all` 干净 / `clippy --workspace --all-targets`（**零警告**）/ 逐 crate `test --release`（**901 全绿**，基线 900 + 1）

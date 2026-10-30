# tianyan-gui-native — GPUI 原生客户端

> **状态：可行性验证已完成，进入真实实现阶段。**
> 结论：**技术上可行，且比我预期好**——UI 层（渲染/IME/流式/虚拟列表/编辑器）与
> 发布层（MSI 打包/静默升级）两块硬骨头都已实测通过。
> 本文档记录**实测数据与踩坑**，是后续开发的最小背景材料。

## 这个 crate 是什么

`gui-native/` 是**独立 workspace**（`Cargo.toml` 顶部的空 `[workspace]` 表），
不参与主 workspace 的 `cargo clippy --workspace` / `cargo test --workspace`，
也不进 rust-analyzer 的主索引。

并存期意图：新客户端与 `gui-vite/` 同时存在，连同一个 axum server
（同一套 HTTP/SSE 协议），逐页替换、随时回退。`tauri/` 在 gui-vite 退役前必须保留。

技术选型：**`gpui-kit 0.6`**（Longbridge 门面，绑 `gpui-pre 0.3.6` 快照）。
不直接用官方 `gpui`（crates.io 停在 0.2.2、315 天未发版），也不用 `gpui-ce`（采用面小）。

## 实测量总表（本机 Windows，非调研推测）

| 指标 | 实测值 |
|---|---|
| Rust 版本要求 | 1.90+（本机 1.99.1） |
| 依赖下载 | 53.3 s（阿里云镜像） |
| **冷编译** | **195.8 s（3m16s）/ 428 个编译单元**（含 GPUI 走 release 级优化） |
| **改一行 → 重编+链接** | **2.2 s** |
| 整文件重写 | 5.9–6.7 s |
| 新增 tree-sitter 依赖后重编 | 79.4 s |
| `target/` 体积 | 2.87 GB（debug） |

> 调研阶段流传的「GPUI 首编 10–20 分钟」不适用——那是 git 依赖整个 Zed 仓库的情形。
> 走 crates.io 快照是好几个数量级的差别。

## 内存对比（关键，容易误判）

同一时刻实测：

| | 工作集 | 私有 |
|---|---|---|
| `tianyan-tauri` 主进程（任务管理器里看到的「100MB」） | 113.5 MB | 62.6 MB |
| `msedgewebview2.exe` × **6 个**（React 真正跑的地方） | **725.6 MB** | 514.8 MB |
| **天演实际总占用** | **837.0 MB** | 575.3 MB |
| **`tianyan-gui-native`**（加载 10 万行代码文件） | **269.1 MB** | 234.2 MB |

**WebView2 是多进程架构**：渲染/GPU/网络/utility 各自独立进程，任务管理器里显示为
`msedgewebview2.exe`，名字不带应用名，极易被漏算。这是 WebView 架构的固有特征。

GPUI 侧分档：

| 场景 | 工作集 |
|---|---|
| 空窗口（中文文本 + 输入框） | 76 MB |
| + 3000 行编辑器 + 5000 条列表 | 131 MB |
| + **10 万行**代码文件（2.9 MB 文本） | 269 MB |

10 万行那一档膨胀约 90 倍（tree-sitter 语法树 + rope + 字形缓存，且是 debug 构建）。
**优化方向**：只加载可见区域 ±N 页（VS Code 也这么做）、release 构建、按需高亮。

## 已验证的能力

- [x] 窗口启动、尺寸/居中/标题（`Bounds::centered` + `TitlebarOptions`）
- [x] **中文 IME 全链路**：微软拼音组合 → 候选 → 上屏；实测 `abc` 被组合成「爱彼此」、
      整句中文逐字一致；输入内容可被程序读取（`observe` + `value()`）
- [x] **原生 Markdown 渲染**：标题/列表/行内代码/链接/**表格**/代码块（tree-sitter 高亮）/引用块
- [x] **流式追加**：1287 字符 / 322 次 `push_str`，单次最大 **45–88 µs**，慢帧 0
      （60fps 帧预算 16.7ms 的 0.5%）
- [x] **流式跟随滚动**（`list_state().scroll_to_end()`）+ **渐显**（`TextViewMotion`）
- [x] **虚拟列表**：5 万条稳态只渲染 **40 项**；滚动 3000+ 行内存 131–139MB 不涨
- [x] **代码编辑器语法高亮**：tree-sitter，需显式开 feature + 设语言
- [x] **超大文件**：10 万行 / 2.9MB，`EditorState` 创建 **43.95 ms**，不崩
- [x] **SSE 常驻订阅 + 发送闭环**（`GET /events` + `POST /chat/stream`）：无人值守自测通过——
      13 个 `chat_stream` 事件、`delta` 共 9 字、"消息 2 条"、`turn_state` 回 `idle` 后
      `streaming=false`；新会话 id 由客户端本地生成、服务端据此新建（列表 49 → 50 条）
- [x] **输入框**：`Textarea`（IME / 选区 / 撤销由组件管）+ Enter 发送 / Shift+Enter 换行
- [x] **停止**：`streaming` 时按钮切「停止」→ `POST /chat/streams/{id}/cancel`
- [x] **MSI 打包 + 静默升级**（见文末）

## 踩坑记录（复用价值最高）

| # | 坑 | 现象 | 正解 |
|---|---|---|---|
| 1 | **样式不是 CSS 继承制** | 父 `div` 设 `text_color` 不穿透子组件 → 黑字压深底，几乎看不见 | 用 `Theme` 语义色，别手写颜色 |
| 2 | **窗口第一层必须是 `Root`** | 背景/弹层/焦点管理都不对 | `cx.new(\|cx\| Root::new(view, window, cx))` |
| 3 | **主题默认浅色** | 深色背景下表格、代码块白得刺眼 | `Theme::change(ThemeMode::Dark, None, cx)` |
| 4 | **`Bounds::centered` 要 `&App`** | `cx.spawn` 闭包里只有 `&mut AsyncApp`，编译不过 | 先在 `run` 回调（`&mut App`）里算好 bounds 再进 spawn |
| 5 | **`TextViewMotion` 默认全 `ZERO`** | 渐显"看不到"——源码注释：*every duration defaults to zero* | 显式 `set_motion(...with_stream_fade(...))` |
| 6 | **流式跟随要手动做** | 内容变长，滚动条停在原地 | 每次追加后 `state.list_state().scroll_to_end()`；`AutoScroll` 是**拖拽选择**用的，不是流式跟随 |
| 7 | **语法高亮要开 feature** | `Editor` 静默无高亮、不报错 | `gpui-kit` 开 `tree-sitter` + `tree-sitter-<lang>`，并 `EditorState::language("rust")` |
| 8 | **`App` 命名冲突** | 自定义 `struct App` 与 `gpui::App` 撞，`items_count(&App)` 报类型不匹配 | 自己的根视图别叫 `App`（本项目用 `DemoApp`） |
| 9 | **`POST /chat/stream` 的响应体不是 SSE** | 按「边发边读响应流」实现，永远收不到增量（只有一个 `{status:started}`） | ADR-028：它是**开关**，输出全走 `GET /events` 常驻流；两个连接必须分开建 |
| 10 | **事件按 `session_id` 路由，新会话 id 必须提前确定** | 等 `POST` 响应再拿 id，会丢掉最先到的几条增量 | 发起前就本地生成 id（服务端接受前端传入的 id 并据此新建） |
| 11 | **SSE 不能按 `String` 攒缓冲** | 中文被 chunk 切断 → 半截 UTF-8 → 替换字符乱码 | 用 `Vec<u8>` 攒、按 `\n\n` **字节**分帧，每帧再 `from_utf8_lossy` |
| 12 | **GPUI 内置 `list()` 不自带尺寸** | `gpui::list` 的 `ListSizingBehavior` 默认 `Auto`：`request_layout` 只把空 style 交给内核，宽高都取「内容尺寸」。放进 `flex_1` 容器里**高度不会被拉伸**（实测 `bounds w=636 h=0`），消息区一片空白但标题/条数正常 | 显式 `.size_full()`（或 `.flex_1()`）；排查用 `on_children_prepainted` 打印真实 bounds，别靠眼睛猜 |
| 13 | **图标要注册资产源** | `IconName::X` 是完整 Lucide 目录（`gpui-kit-assets` 内嵌 1800+ SVG，与 Tauri 版 `lucide-react` 同一套），但没注册就**静默空白、不报错** | `gpui_kit::application().with_assets(gpui_kit::assets::AllAssets::new(""))`；`assets` 是 gpui-kit 的默认 feature |
| 14 | **`flex_1` 只在 flex 父容器里生效** | 设置页某栏目的根用 `flex_1().min_h_0()` 想撑满，但父容器只是普通 `div` → 高度退化成内容高度：人设编辑器被压成一行、长栏目**滚不动**（内容被裁切） | 父容器同时给 `.flex().flex_col()`；子栏目的根再给 `.flex_1().min_h_0()`。判据：`flex_1` 的父元素 `display` 必须是 flex |
| 15 | **PowerShell 5.1 会把 JSON 里的中文写坏**（本次事故：把人设写坏两次） | ① `Invoke-RestMethod -Body`（字符串）按 ANSI 处理非 ASCII → 服务端 400 `invalid unicode code point`；② 更隐蔽：**响应头没有 `charset` 时按 ISO-8859-1 解码**，读到的中文本身已是乱码，再写回就把数据彻底写坏 | 脚本必须跑在 `pwsh`（7+，默认 UTF-8），并在文件头写 `#requires -Version 7`；发送中文一定「落 UTF-8 无 BOM 文件 → `-InFile`」，不要塞 `-Body`。已被写坏时可用 `latin1.GetBytes → UTF8.GetString` 反向解（每次解掉一层） |
| 16 | **`render_to_image` 取的是「最近一次绘制的帧」** | 窗口被遮挡 → GPUI 跳过重绘 → 截到旧帧（连切 3 个栏目 sha256 全同）；`show=false` 创建的窗口 → 1×1 | 截图前提前台（`tray::show_main_window()`，`screenshot` 动作内已调）；**视觉核对用可见窗口跑，断言用隐藏窗口跑** |
| 17 | **后端校验要求「提供商至少有一个模型」** | 只加 provider（`models: []`）→ `PUT /config` 被 422 拒绝（`提供商 'x' 至少需要一个模型`），UI 上表现为「点了没反应」 | 新增提供商表单必须**一次带首个模型名 + 能力标签**（`core/src/config/model.rs::validate`） |
| 18 | **`flex_1` 撑满 + 编辑器回填不能依赖 `render`** | ①栏目根用 `flex_1` 但父容器不是 flex → 高度塌陷；②「置标志、由 render 消费回填」在隐藏窗口下永不执行 → 编辑器全空，一保存就把所有文本字段写成空值（报一片「不能为空」） | ①父容器 `.flex().flex_col()`；②回填统一走 `window_handle.update` 现取 `&mut Window`，并用 `settings_backfilled` 标志保证「有配置就补回填」 |
| 19 | **`window_close` 只是「藏到托盘」，不是退出** | 验收脚本用它收尾 → 进程一直活着占着 exe，下一次 `cargo build --release` 报「拒绝访问 (os error 5)」，而且每跑一轮就攒一个隐形实例（托盘里越堆越多） | 自动化新增 `quit` 动作（= 托盘「退出天演」：`tray::begin_quit()` + `remove_window()`）；脚本收尾改用它，并额外用 `-PassThru` 记 PID 兜底强杀 |
| 20 | **MSI 是 32 位包，ARP 落在 `HKLM\WOW6432Node`** | 按 perUser 装完却在 `HKCU\...\Uninstall` 找不到条目，容易误判「没注册、没法卸载」 | 这是 MSI 规范行为（`ALLUSERS=2` 时 ARP 跟 ALLUSERS 走）。条目在 `HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{ProductCode}`，控制面板可见、卸载不需要提权。要挪到非 WOW 位置就显式 `<Package Platform="x64">` |
| 21 | **流式多步：一轮对话 = 多条 assistant 消息** | 流式期能看到「🔧 工具名」，流一结束（或切换会话重载）前面几步的工具调用全不见；工具没有参数/结果/耗时 | 后端一轮是**多步**（LLM→工具→LLM…），库里每个 assistant 响应是独立一条消息（实测 4 步=4 条）。前端早期把整轮堆在一条占位里，`tool_call` 还被塞成正文文本 `` `🔧 name` ``、`observation` 空实现。正解=对齐 vite：`tool_call` → `Seg::Tool` + `ToolCall` 占位；`observation` → 按 `tool_call_id` 回填结果/耗时；步边界→新开一条（判据 `step_closed`：已有正文段 或 工具都已出结果），并配 5 个单测钉住 |
| 22 | **bin crate 里写 `#[test]` 别用 `use super::*`** | `recursion limit reached while expanding #[test]`；更狠时 rustc 直接 `STATUS_STACK_OVERFLOW`（0xc00000fd） | bin crate 的根是 `main.rs`，`super::*` 把整棵模块树 + 巨大泛型拉进宏展开。显式 `use crate::types::{MsgRow, Seg, ToolCall}`；必要时 `$env:RUST_MIN_STACK = "67108864"` 给 rustc 加栈 |
| 23 | **剪贴板图片可直接读，不用落盘** | 想「截图→粘贴」，以为得走 Win32 `CF_DIB` + 手工转 PNG | GPUI 的 `cx.read_from_clipboard()` 返回 `ClipboardItem.entries`，其中 `ClipboardEntry::Image(Image { format, bytes, .. })` 直接给原始字节 + 格式，复用 `bytes_to_data_url` 即可（8MB 上限） |

## API 备忘（实测签名，省一次翻源码）

```rust
// 窗口
cx.open_window(WindowOptions { window_bounds, titlebar, ..Default::default() }, |window, cx| {...})
WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(840.)), cx))  // cx 必须是 &App

// 文本视图（流式）
TextViewState::markdown("", cx)                 // 可变 state
state.push_str(chunk, cx) / set_text("", cx)
state.list_state().scroll_to_end()
state.set_motion(TextViewMotion::default().with_stream_fade(d).with_stream_fade_stagger(d))
TextView::new(&entity).scrollable(true)         // 渲染可变 state
TextView::markdown(id, text)                    // 一次性静态渲染（2 个参数，无 window/cx）

// 代码编辑器
EditorState::new(window, cx).language("rust").default_value(&code)
Editor::new(&entity).h_full()

// 虚拟列表（ListDelegate 必须实现：items_count / render_item / set_selected_index）
impl ListDelegate for D {
    type Item = ListItem;
    fn items_count(&self, _section: usize, _cx: &gpui_kit::App) -> usize { ... }
    fn render_item(&mut self, ix: IndexPath, _w: &mut Window, _cx: &mut Context<ListState<Self>>) -> Option<ListItem> {
        Some(ListItem::new(ix.row).h(px(52.)).child(div().child("...")))
    }
    fn set_selected_index(&mut self, _ix: Option<IndexPath>, _w: &mut Window, _cx: &mut Context<ListState<Self>>) {}
}
List::new(&entity)                              // IndexPath 从 gpui_kit::component 导入

// 输入框（Textarea = 多行 + IME + 选区 + 撤销，全由组件管，不要自造）
let input = cx.new(|cx| TextareaState::new(window, cx).placeholder("输入消息…"));  // placeholder 在 State 上
Textarea::new(&entity).h(px(110.))              // 渲染；.h() 收 DefiniteLength
state.value() -> SharedString
state.set_value("", window, cx)                 // 清空（窗口参数不可省）
// Enter 提交：订阅 InputEvent（多行框自己已插入换行，发送后清空即可）
cx.subscribe_in(&input, window, |this, _input, ev: &InputEvent, window, cx| {
    if let InputEvent::PressEnter { shift, .. } = ev { if !shift { /* 发送 */ } }
}).detach();

// 异步
cx.background_executor().timer(Duration::from_millis(25)).await
entity.update(&mut app, |state, cx| {...})      // 返回 ()，不是 Result
```

## 开工途中挖出的产品级问题：MCP 客户端忙循环（已修）

原生客户端空闲时有一个线程接近满载——**功能完全正常**（25 个 MCP 工具全部可用），
所以此前一直被当作「UI 渲染开销」。用 GPUI 侧的干净窗口做 CPU 时间二分（5 秒窗口的
进程 CPU 时间）才定位到真正的源头：

| 分组 | 5 秒窗口 CPU 时间 |
|---|---|
| 只跑 GPUI（不启 server） | 0.14s |
| 建 tokio runtime、不启 server | 0.08s |
| 完整 server | 5.36s |
| `full()` 装配（11 插槽） | 5.45s |
| `policies()` 装配（去掉 MCP 动态工具 + LanceDB） | 6.22s ← 反而更高，排除这两块 |
| 禁用 MCP（同 `full()`） | **0.14s** |
| 启用 MCP（playwright，连接成功 25 工具） | **5.73s** |
| 单独跑 playwright MCP 子进程 | 不忙 ← 排除子进程自身与「连接失败重试」 |
| **升级 `rust-mcp-sdk` 0.9.0 → 1.1.0 后** | **0.19s（−96.4%）** |

结论：忙循环在 `rust-mcp-sdk 0.9.0` 内部（与连接成败无关，连上了也忙）。
修法是升级到 1.1.0，**天演自身代码零改动**（stdio / streamable-http 传输、
client handler、工具列表在 1.x 保持兼容），`cargo test -p tianyan-mcp` 18/18 通过。
**这条修复对现有 Tauri 桌面版同样生效**（同一份 MCP 客户端代码）——用户可感知的变化
是「应用常开时风扇不再一直转」。变更记录见根 `CHANGELOG.md` 的 `[Unreleased]`。

> 方法学备忘：**「server 慢」不等于「server 代码有问题」**。这次二分先按「是不是
> runtime」「是不是某个插槽」推进，两轮都空手——真正的判据是「把 MCP 关掉就干净」。
> 遇到「功能正常但白烧 CPU」时，先按**外部依赖**（子进程 / SDK / 网络客户端）分叉，
> 比按自己的模块分叉收敛得快。

## 页面覆盖（11 / 11）

| 入口 | 关键接口 |
|---|---|
| 会话 | `/sessions` · `/chat/stream` · `/usage/*` · `/todos` · `/goals` · `/tasks` |
| 文件 | `/workspace/tree` · `/read` · `/dirs` · `/diff` · `PUT /sessions/{id}/workspace` |
| 技能 | `/skills` · `/skills/stats` · `/skills/{id}`（只列 `category=custom` 的方法论） |
| 子智能体 | `/roles` · `/roles/stats` · `/roles/{name}` · reset / delete |
| 工具 | `/tools`（与 LLM 收到的 tools 同源；右侧只读 schema，不做表单） |
| 知识 | `/knowledge/entries` · `/read` · `/delete` · `/ingest`(multipart) · `/search` |
| 记忆 | `/memory`（`memory/` + `user/` + `agent/patterns` 聚合，L0/L1/L2 三层） |
| 审批 | `/approval/status` · `/approval/respond` |
| 定时任务 | `/scheduled-tasks` GET / POST / DELETE |
| 洞察 | `/stats` · `/usage/stats?days=` · `/scheduler/status` |
| 设置 | 14 个二级栏目（见下节）；`/config`(GET/PUT 整份) · `/config/models` · `/config/soul` · `/config/mcp/servers` · `/usage/stats` · `/health` · `/config/migrate-data-dir` |

验证：`pwsh -NoProfile -File .\scripts\automation-pages.ps1`（只读冒烟，含「11 个入口
全部可达」；做法见上文「自动验证」——全程不截图、不点坐标）。

## 设置页：14 个二级栏目，全部可用（无「暂未实现」）

栏目 id / 顺序 / 文案 / 图标与 `gui-vite` 的 `TABS` 逐项一致，但**内容实现方式不同**：
vite 是 14 份手写受控表单，这里是「字段声明成数据 + 一套渲染/保存」。

| 类别 | 栏目 | 实现 |
|---|---|---|
| 字段引擎（7） | 数据存储 · Agent 行为 · 安全 · 日志 · 记忆 · 检索 · Web 搜索 | `settings/fields.rs` 的 `FIELDS` 表（**52 个字段**）：开关 / 分段枚举 / 数字 / 单行 / 多行 / 只读六种控件，逐字段带说明与「需重启」徽标 |
| 自带接口（3） | 模型服务 · 人设编辑 · MCP | provider 编辑+测连、模型切换 / soul 全文编辑+填入默认 / 服务器列表+启停+测试连接+增删 |
| 只读展示（4） | 用量统计 · 连接 · 关于 · 外观 | 用量（时段×分组×占比条）/ `/health` 实测（版本·数据目录·pid·一致性告警）/ 版本与构建形态 / 三套色板 |

**保存语义（比 vite 更保守）**：`PUT /config` 是整份替换，所以保存时先 `GET` 最新配置、
只覆盖**真正改动过**的字段、再整份提交——把「读-改-写」窗口压到一次往返，不会冲掉
期间被别处改过的东西（例如 agent 刚切换的模型）。顶部状态行给「已修改 N 项」与
「保存 N 项 / 放弃」，保存后汇总「其中 M 项需重启客户端生效」。

**数据目录搬迁的闭环**：`POST /config/migrate-data-dir` 只写请求文件并触发服务关停；
真正「复制 → 校验 → 改配置 → 删源」在服务停掉之后做（SQLite/LanceDB 的文件锁才释放）。
Tauri 版由监督循环在进程内完成；原生端在上一次进程退出后，由**下一次启动**在起 server
之前执行（`main.rs` 的 `run_pending_migration`），所以 UI 明说「需关闭并重新打开客户端」。

## 已知限制与行为

- ~~没有单实例保护~~ **已解决（ADR-051）**：抽了 `desktop/` 启动编排段，`ensure_server`
  先发现「服务同一数据目录」的已有实例并复用（判据 = `/health` 的 `service` **且**
  `data_dir`），未命中才自起。当初实测到的问题：同时跑桌面版（3000）与 gui-native（3001）
  时两个内嵌 server 共享同一个数据目录（`D:\tianyan`）——定时任务各跑一遍、SQLite 并发写。

- **变高列表启动有 O(n) 测高**：5 万条 → `render_item` 5080 次 / 2.9s CPU（一次性）。
  稳态渲染量与总数无关（40 项）。改造方向：消息行高设上限，或改用 `v_virtual_list`。
- **表格在流式中会有一次重排**：Markdown 必须解析到 `|---|` 才能确定列结构，
  越过该点必然重排一次。代码块无此问题（``` 一出现即可确定边界）。属正常解析行为。
  UI 上可在流式期对表格做「占位延迟渲染」抹平。
- **gpui-component 没有 diff 视图组件**（顶层模块无 diff/merge/compare）。
  但底层能力齐备：`core` 已有 `similar` + `build_hunks`（`snapshot/mod.rs`），
  `SyntaxHighlighter` / `LanguageRegistry` / `HighlightTheme` 是公开可复用组件。
  → 天演的 `DiffView` / `WorkspaceDiffPanel` 需自组装双栏 List，约 100–200 行。

- **关闭按钮 = 藏到托盘，不是退出**：这是托盘应用的常规语义，但第一次遇到会以为
  "关不掉"。真退出走**托盘右键菜单 → 退出天演**。自动化里 `window_close` 动作
  （发 `WM_CLOSE`）走的是同一条路径，可用来确认行为。
- **`Ctrl+Alt+T` 与 tauri 版冲突**：一个组合键只能属于一个进程，两个客户端同跑时
  后注册的那个失败（日志写明原因）。用 `TIANYAN_GUI_HOTKEY` 覆盖即可。

## 未验证项

| 项目 | 备注 |
|---|---|
| 编辑器：搜索/替换、多光标、代码折叠、LSP | 天演现在用 CodeMirror 的功能较基础，大概率够用 |
| 文件拖放 | 天演现有能力 |
| IME 候选窗定位 | 上屏已验正确，选字框位置只能人眼确认 |
| 代码签名 | 现状 `certificateThumbprint: null`（未签名 → SmartScreen 警告），换方案不改变 |
| 多语言 MSI | 现状 Tauri 出 `zh-CN` / `en-US` 两份，WiX 需配 culture 文件 |
| 托盘右键菜单的人工交互 | 菜单项动作与自动化走**同一对** `tray::*` 函数（已断言），但"右键弹出菜单、点菜单项"这一步只能人眼确认 |
| 系统集成：托盘 / 通知 / 全局快捷键 / updater 编排 | tauri/ 那 1163 行的 B 类职责需逐个接管 |

## MSI 打包与自动更新（已实测）

### 1.1.0 修的七处（用户实测反馈）

| # | 症状 | 根因 / 修法 |
|---|---|---|
| 1 | 输入框没法**粘贴截图** | 只有「附件→翻文件夹」。新增 **「粘贴图片」按钮**（`IconName::ClipboardPaste`，在附件图标右边）：读剪贴板 `ClipboardEntry::Image` 的原始字节 → data URL（8MB 上限），不落盘、不进 VFS |
| 2 | **流式时工具渲染不对** | `tool_call` 事件曾被塞成正文文本 `` `🔧 name` ``（没有参数/结果/耗时，还和正文混在同一个 Markdown 段里）。改为结构化 `Seg::Tool` + `ToolCall` 占位，渲染走与历史消息**同一条** `render_tool_card` |
| 3 | **流式结束后前面的工具调用全不见** | 一轮是**多步**（LLM→工具→LLM…），库里每步一条 assistant 消息，而前端把整轮堆在一条占位里 → 任何一次「用权威消息替换最后一条」都会把前面几步抹掉。改为**步边界新开消息**（判据 `step_closed`），并让 `observation` 按 `tool_call_id` 回填结果（此前是空实现，结果永不显示） |
| 4 | **新建会话后输入框还留着上一段文字**（"在别的会话输入的字跟着过去了"） | 草稿的键取 `current_session.unwrap_or_default()`：新会话（首条消息未发）的键是**空串**，于是所有「新建会话」共用一个桶；而 `start_new_session` 是「先存本会话草稿（空键）→ 再按新键恢复（同一个空键）」= **自存自读**，内容原地不动。改为：`save` 在 `current_session == None` 时直接不存，`restore` 此时**必须清空**（不查任何桶）。实测（隔离实例 + `/state` 的 `usage.composer_text`）：输入 `AAA`→点新建→仍是 `AAA`；输入 `BBB`→再点新建→仍是 `BBB`；修后两次都为空 |
| 5 | **轮次进行中切走再切回，"停止"按钮退回"发送"** | `open_session` 里无条件 `self.streaming = false`（"本客户端只渲染当前会话的增量"），而服务端这一轮**还在跑**——切回后既没有停止入口，也看不出它在生成。新增 `running_sessions: HashSet<String>`：`turn_state` 事件带 `session_id`，因此在 `handle_event` 的**会话过滤之前**记账（非当前会话也记），`open_session` 改为 `streaming = running_sessions.contains(&id)`；发送时乐观 insert、失败/删会话时 remove。实测：发送后 `streaming=true` → 切到新会话（false）→ 切回（**修前 false，修后 true**） |
| 6 | **会话里图片上下有大片空白** | 只写了 `.max_w(240).max_h(240)`。GPUI 的 `img()` 在 `size` 为 Auto 时会把元素尺寸设成**图片原始像素**（`img.rs:352-376`：`style.size = image_size`），随后 `max_w`/`max_h` 是**两次独立钳制**（各自取 min，不是等比）→ 得到 **240×240 的正方框**，图片再 `ObjectFit::Contain` 居中画成 240×150，上下各留 45px 空白。修法：新增 `image_dimensions`（`ImageReader::into_dimensions`，只读 header 不解全图）+ `fit_image_size`（等比缩放、小图不放大），渲染时显式 `w`/`h`。1024×640 → **384×240**（与 vite 的 `max-h-[240px]` 观感一致），6 个单测锁住 |
| 7 | **待发送图片缩略图在输入框下方**（vite 在上方） | `render_composer` 里 `.child(Textarea)` 之后才 `.children(attachments)`。修法：附件挪到 Textarea **之前**；缩略图同时对齐 vite 的 `w-16 h-16 object-cover`（原来 `.max_w(64).max_h(64)` + Contain，非正方形图同样有白边）→ `.w(64).h(64)` + `ObjectFit::Cover` |

回归：`step_boundary_tests` 5 个单测（空占位/只有思考/已有正文/工具未回结果/工具已回结果）+ 77 项界面断言全绿。
第 4/5 条的复现与回归都用**隔离实例**（`TIANYAN_CONFIG` 指独立配置、`storage.data_dir` 指临时目录、
`TIANYAN_GUI_AUTOMATION_PORT` 指独立端口），全程 `/act` + `/state` 断言，不碰 `D:\tianyan`。
⚠️ 教训：隔离实例与已装实例**进程同名**（都叫 `tianyan-gui-native`），收尾**禁止**
`Get-Process tianyan-gui-native | Stop-Process`——那会把用户正在用的实例一起杀掉
（实测踩过）。只按 `Start-Process -PassThru` 拿到的 **PID**（或按 `Path`）精确操作。

### 出包一条链（产品级，已实测 1.1.0）

```powershell
cd gui-native
cargo build --release                 # build.rs 会把 tauri/icons/icon.ico 嵌进 exe 资源
pwsh -NoProfile -File ..\scripts\verify-gui-native.ps1 -Release   # 77 项断言（验的是要发的那份）
cd installer
wix build -o tianyan-native-1.1.0-x64.msi tianyan.wxs
msiexec /i tianyan-native-1.1.0-x64.msi /qn      # perUser：2.8 秒，无 UAC
```

`tianyan.wxs` 是**产品级**清单，与 `main.wxs` / `peruser.wxs`（早期验证用，Source 指向 `target\debug`）
的区别：

| 项 | 验证包 | 产品包（tianyan.wxs） |
|---|---|---|
| 名称/版本 | 「天演原生客户端（验证）」0.1.3.0 | 「天演」1.0.1.0（与 server / tauri 版对齐，ARP 里看起来是同一个产品） |
| 二进制 | `target\debug` | `target\release`（PE `Subsystem=2`，无控制台窗口） |
| 图标 | 无 | `build.rs` 嵌 exe 图标（复用 `tauri/icons/icon.ico`）+ ARP/快捷方式图标 |
| 安装位置 | `%LOCALAPPDATA%\Programs\TianyanGuiNative` | `%LOCALAPPDATA%\Programs\Tianyan`（**不碰** tauri 的 `D:\Program Files\Tianyan`——那里已被历史产物污染） |
| 快捷方式 | 无 | 开始菜单「天演」 |
| 数据目录 | 不动 | 不动（`D:\tianyan` 由配置决定，卸载不删） |

体积：release exe 275MB（`[profile.release] strip = "symbols"` 后；不 strip 是 288MB），
MSI **61MB**。GPUI 的体积大头是静态数据（LanceDB/DataFusion、gpui-kit-assets 的 1800+ SVG、
tree-sitter 语法），不是符号表——strip 只省 13MB。

### 打包路径：WiX v6（不是 cargo-wix）

| 方案 | 结论 |
|---|---|
| `cargo-wix` | ❌ 依赖 WiX **v3** 的 `candle.exe`/`light.exe`，最新版停在 2025-03 |
| `cargo-dist` / `dist` | ⚠️ 仍用 WiX v3；其 updater `axoupdater` 依赖 install receipts，与 MSI 分发不匹配 |
| **WiX v6（dotnet tool）** | ✅ `dotnet tool install --global wix --version 6.0.2` → `wix build -o x.msi x.wxs` |
| WiX v7 | ⚠️ **引入 OSMF 收费条款**：年收入 > $10,000 的组织必须赞助；低于此额度免费但需显式 `-acceptEula wix7`。**用 v6 可彻底绕开这个法律承诺** |

实测：`wix build` 打 42.9MB 的 debug exe → **9.34MB MSI，18 秒**。

### 静默安装 / 升级实测

| 场景 | 结果 |
|---|---|
| perMachine（现状，`ALLUSERS=1`） | 需管理员提权 → 更新必弹 UAC |
| **perUser（`Scope="perUser"`，装到 `%LOCALAPPDATA%\Programs\`）** | ✅ **`msiexec /i /qn` 静默安装成功，1.3 秒，无 UAC** |
| MajorUpgrade（0.1.1 → 0.1.2 → 0.1.3） | ✅ 正确替换，注册表恒为单条目，**无新旧并存** |
| **同版本号重新构建**（1.1.0.0 → 1.1.0.0 的新 build） | ✅ 开了 `AllowSameVersionUpgrades="yes"` 后走 MajorUpgrade 序（先卸旧、再装新），ARP 恒单条目，实测 3.3 秒；**不开则报 1638**（已安装此产品的另一个版本） |
| 查询已安装版本 | ✅ `WindowsInstaller.Installer` COM 的 `ProductInfo(..,'VersionString')` |
| **应用运行中升级** | ⚠️ msiexec 退出码 0、版本已升，**但进程被 Restart Manager 直接杀掉**（无提示，也不帮你重启） |

> 现状：生产版 `Tianyan 1.0.1` 是 perMachine，装在 `D:\Program Files\Tianyan\`。
>
> **为什么需要 `AllowSameVersionUpgrades`**：WiX 未显式指定 `ProductCode` 时
> **每次 `wix build` 都生成新 GUID**（实测同一份 `tianyan.wxs` 连续三次构建 =
> `{CDCA811D…}` / `{632C1702…}` / `{ADBD5FD1…}`），而 MSI 把「同版本 + 不同
> ProductCode」判成**另一个产品** → 默认直接拒绝（错误 1638）。改 bug 但版本号
> 仍保持 1.1.0（不擅自升版本）就会踩到，因此 `tianyan.wxs` 里**固定开着**它。
> 不同版本号的升级行为不受影响：`Upgrade` 表的同版本行是
> `VersionMax=当前版本`、`Attributes=513`，降级检测行是 `VersionMin=当前版本`、
> `Attributes=2`。
> 改 perUser 可换来「更新零 UAC」，代价是多用户机器上需各自安装。

### 自研 updater 必须做的编排（约 200–400 行）

1. 检查更新（HTTP 拉 release 清单，比对版本）
2. 下载 MSI + 校验（sha256，或沿用 minisign 签名）
3. **主动优雅关闭自己**（保存状态；否则会被 Restart Manager 强杀）
4. 以分离进程调 `msiexec /i <msi> /qn /norestart`
5. 成功后**重新启动新版本**；退出码 `3010` 表示需重启
6. 失败要可见（日志 + 提示），MSI 自身有事务回滚

## 怎么跑

```powershell
cd gui-native
cargo run            # 首次约 3 分钟，之后增量 2-6 秒
```

## 自动验证

### 一条命令跑完（推荐）：`scripts/verify-gui-native.ps1`

```powershell
.\scripts\verify-gui-native.ps1              # 复用或隐藏启动，跑 66 项断言
.\scripts\verify-gui-native.ps1 -Restart     # 改过 Rust 代码后：先关旧实例再起
.\scripts\verify-gui-native.ps1 -KeepAlive   # 跑完不关，下次复用（零启动等待）
```

它做三件事，都是为了**不在用户桌面上折腾**：

1. **复用**：3010 上已有实例就直接用，不重启；
2. **隐藏**：需要新起时带 `TIANYAN_GUI_START_HIDDEN=1`——窗口建好但不显示（任务栏
   不闪、不抢焦点），而 `/state`、`/act` 全部照常；
3. **一次跑完**：14 个栏目切换 + 字段引擎（改→脏→保存→后端落库→恢复）+ 用量 +
   人设 + MCP + 存储 + 连接，**跑完自动把改动恢复原值**（配置与人设都校验回基线）。

> 反复启动客户端验证是这次被用户点名的干扰源（「任务栏图标反复出现又消失」）。
> 现在验证只起一个进程，且默认不显示窗口。

### 首选：内建自动化端口（ADR-052）—— 不截图、不点坐标

界面自己提供 `GET /state`（状态快照）与 `POST /act`（语义动作），agent 直接读断言、
直接触发动作。**锁屏也能跑**（不依赖窗口可见）：

```powershell
# 终端 1：带端口启动
$env:TIANYAN_GUI_AUTOMATION_PORT = "3010"
.\target\debug\tianyan-gui-native.exe

# 终端 2：跑冒烟（要用 pwsh；PowerShell 5.1 会把无 BOM 的 UTF-8 中文读成乱码）
pwsh -NoProfile -File .\scripts\automation-smoke.ps1
```

随手用（`curl` 也行）：

```powershell
# 状态快照
Invoke-RestMethod http://127.0.0.1:3010/state | ConvertTo-Json -Depth 8
# 触发动作（返回动作**之后**的状态；settle_ms = 睡一会再读，等异步数据落地）
Invoke-RestMethod http://127.0.0.1:3010/act -Method Post -ContentType application/json `
  -Body '{"action":"switch_tab","view":"skills","settle_ms":300}'
```

能断言的东西（以前只能看图的）：`messages.last.head`（消息正文渲染出来没有）、
`sessions_rows`（分组折叠 / 查看更多剩几条）、`picker.current`（目录选择器在第几层）、
`workspace.diff.files` 与单文件 `+N −M`、`tasks.items[].activity`（子代理活动流拉到几条）。
完整动作清单见 `src/automation.rs` 的 `apply_action`。

> 端口**默认不开**：没设 `TIANYAN_GUI_AUTOMATION_PORT` 就不监听（生产 MSI 不带该变量）。
> 只绑 `127.0.0.1`，且 `state` 不含 `api_key`。

### 像素级核对：内建截图优先，抓屏兜底

配色 / 间距 / 字体这类「好看不好看」还得看像素。**状态验证不要用截图**。

**首选内建**（`render_to_image`，需 `--features screenshot` 构建）：

```powershell
cargo build --features screenshot
$env:TIANYAN_GUI_AUTOMATION_PORT = "3010"; .\target\debug\tianyan-gui-native.exe
Invoke-RestMethod http://127.0.0.1:3010/act -Method Post -ContentType application/json `
  -Body '{"action":"screenshot","path":"C:\\tmp\\shot.png"}'
```

它拿的是**渲染输出**而不是屏幕像素，所以不受 DPI 缩放影响；尺寸是纯客户区物理像素
（本机 1920×1230），不含窗口边框。

> ⚠️ **但它取的是「最近一次绘制的帧」，不是「现在的界面」**（这次实测更正了原先
> 「窗口是否可见都不受影响」的说法）：
> - **窗口被完全遮挡**时 GPUI 跳过重绘 → 截到旧帧。症状极其隐蔽：连切 3 个栏目截图，
>   三张 sha256 完全相同，很容易误判成「切 tab 没生效」。做法：截图前先
>   `SetForegroundWindow` 把它提到前台（`screenshot` 动作内部已自动调用
>   `tray::show_main_window()`）。
> - **从未显示过的窗口**（`TIANYAN_GUI_START_HIDDEN=1`，即 `WindowOptions.show=false`）
>   没有可用帧 → 截出 1×1。所以「隐藏启动跑断言」与「截图核对视觉」必须分开跑：
>   前者用隐藏窗口，后者用可见窗口。
> - 「先显示过、再 `SW_HIDE` 藏到托盘」仍能截（帧还在）——与「从未显示」是两种情形。

**兜底：进程外抓屏**（生产构建没有 `screenshot` feature 时）：

```powershell
.\scripts\ui-click.ps1 -X 260 -Y 120     # 坐标 = 截图里的像素坐标，原点是窗口左上角
.\scripts\ui-shot.ps1 -Out "$env:TEMP\shot.png"
.\scripts\ui-shot-full.ps1 -Out "$env:TEMP\full.png"   # 全屏（看系统通知/托盘这类窗口外的东西）
```

> ⛔ **必读坑：PowerShell 默认 DPI-unaware。** 本机 2560×1440 @150%，`GetWindowRect`
> 返回的是虚拟化坐标——物理 1920×1230 的窗口只报 1295×857，于是 `CopyFromScreen`
> **只抓到窗口左上角那块**。症状极像「右下角的元素没渲染」（消息区空白、输入卡片
> 消失、任务面板不见），曾据此误判 GPUI 的 flex 布局失效、白查数轮。
> `scripts/ui-shot.ps1` 已用 `SetProcessDpiAwareness(2)` 修掉，**不要再手写截图**。
>
> 第二个坑：`CopyFromScreen` 抓的是屏幕像素，窗口被别的窗口挡住会抓到遮挡物，
> 所以脚本里先 `ShowWindow` + `HWND_TOPMOST` + `SetForegroundWindow`。

GPUI 自己的 `#[gpui_kit::test]`（headless 窗口 + 真实事件 + 断言，需开 `test-support`
feature）适合**纯视图逻辑的单元级验证**（不进真实 app 状态机）；而自动化端口验证的是
**运行中的整个界面**（有后端数据、有 SSE、有异步加载）。两者互补，都不是观感。

## 系统集成（D 组：通知 / 托盘 / 全局快捷键）

三个能力都不在 GPUI 的 API 里，全部要自己接 Win32。共同的做法：**独立线程 + 自己的
win32 消息循环**（GPUI 独占主线程循环且不暴露 hook），主线程只执行动作。

| 能力 | 文件 | 关键实现 | 验证方式 |
|---|---|---|---|
| 系统通知 | `notify.rs` | `core::notification::NotificationSink` 的原生实现 + 注册 AUMID | 截图（通知在窗口外，`/state` 读不到）+ 断言注册表 |
| 系统托盘 | `tray.rs` | `tray-icon` + `EnumWindows` 找主窗口 + `ShowWindow(SW_HIDE/SW_SHOW)` | 断言 `tray.window_visible` 翻转；`WM_CLOSE` 验证"关闭到托盘" |
| 全局快捷键 | `hotkey.rs` | `RegisterHotKey` + `GetMessageW` 阻塞等待 | `simulate-hotkey.ps1` **真的按键**（`keybd_event`） |

三个坑（都实测踩过）：

1. **通知必须要 AppUserModelID**。不注册也能弹，但归属名退回 `Windows PowerShell`
   ——「PowerShell 发来一条通知」比不弹更糟。启动时写
   `HKCU\Software\Classes\AppUserModelId\Tianyan.GuiNative`（`DisplayName` + `IconUri`，
   **免管理员**）。
2. **托盘找窗口不能只看 `IsWindowVisible`**。窗口一被 `SW_HIDE`，可见性就是 false，
   于是"隐藏之后再显示"找不到窗口、永久失效且**毫无报错**。必须缓存 `HWND`
   （命中后只校验 `IsWindow`）。`automation-smoke` 里「隐藏后仍持有句柄」就是这条的回归断言。
3. **`Ctrl+Alt+T` 与 tauri 版冲突**。一个组合键只能属于一个进程，两客户端同跑时后注册
   的必然失败。用 `TIANYAN_GUI_HOTKEY` 覆盖（同时也解决"用户机器上被别的软件占用"）。

`unsafe` 只出现在 `tray.rs` / `hotkey.rs` 的 Win32 边界（枚举窗口、显隐窗口、泵消息、
注册热键），每处都有 SAFETY 注释。

打包 MSI（需先 `dotnet tool install --global wix --version 6.0.2`）：

```powershell
cd gui-native\installer
wix build -o tianyan-gui-native.msi peruser.wxs
```

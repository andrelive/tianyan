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

// 输入框
TextareaState::new(window, cx)                  // 注意 2 个参数
Textarea::new(&entity).h(px(110.)).w_full()     // 无 .placeholder()

// 异步
cx.background_executor().timer(Duration::from_millis(25)).await
entity.update(&mut app, |state, cx| {...})      // 返回 ()，不是 Result
```

## 已知限制与行为

- **变高列表启动有 O(n) 测高**：5 万条 → `render_item` 5080 次 / 2.9s CPU（一次性）。
  稳态渲染量与总数无关（40 项）。改造方向：消息行高设上限，或改用 `v_virtual_list`。
- **表格在流式中会有一次重排**：Markdown 必须解析到 `|---|` 才能确定列结构，
  越过该点必然重排一次。代码块无此问题（``` 一出现即可确定边界）。属正常解析行为。
  UI 上可在流式期对表格做「占位延迟渲染」抹平。
- **gpui-component 没有 diff 视图组件**（顶层模块无 diff/merge/compare）。
  但底层能力齐备：`core` 已有 `similar` + `build_hunks`（`snapshot/mod.rs`），
  `SyntaxHighlighter` / `LanguageRegistry` / `HighlightTheme` 是公开可复用组件。
  → 天演的 `DiffView` / `WorkspaceDiffPanel` 需自组装双栏 List，约 100–200 行。

## 未验证项

| 项目 | 备注 |
|---|---|
| 编辑器：搜索/替换、多光标、代码折叠、LSP | 天演现在用 CodeMirror 的功能较基础，大概率够用 |
| 文件拖放 | 天演现有能力 |
| IME 候选窗定位 | 上屏已验正确，选字框位置只能人眼确认 |
| 代码签名 | 现状 `certificateThumbprint: null`（未签名 → SmartScreen 警告），换方案不改变 |
| 多语言 MSI | 现状 Tauri 出 `zh-CN` / `en-US` 两份，WiX 需配 culture 文件 |
| 系统集成：托盘 / 通知 / 全局快捷键 / updater 编排 | tauri/ 那 1163 行的 B 类职责需逐个接管 |

## MSI 打包与自动更新（已实测）

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
| 查询已安装版本 | ✅ `WindowsInstaller.Installer` COM 的 `ProductInfo(..,'VersionString')` |
| **应用运行中升级** | ⚠️ msiexec 退出码 0、版本已升，**但进程被 Restart Manager 直接杀掉**（无提示，也不帮你重启） |

> 现状：生产版 `Tianyan 1.0.1` 是 perMachine，装在 `D:\Program Files\Tianyan\`。
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

打包 MSI（需先 `dotnet tool install --global wix --version 6.0.2`）：

```powershell
cd gui-native\installer
wix build -o tianyan-gui-native.msi peruser.wxs
```

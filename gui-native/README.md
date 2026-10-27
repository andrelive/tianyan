# tianyan-gui-native — GPUI 原生客户端可行性验证

> **状态：验证性 spike，不是产品代码。**
> 目的只有一个：用本机实测回答「天演是否值得把前端从 Tauri+React 换成 GPUI」。
> 结论：**技术上可行，且比我预期好**。是否采纳见文末「未验证项」与决策建议。

## 这个 crate 是什么

`gui-native/` 是**独立 workspace**（`Cargo.toml` 顶部的空 `[workspace]` 表），
不参与主 workspace 的 `cargo clippy --workspace` / `cargo test --workspace`，
也不进 rust-analyzer 的主索引。

并存期的意图：新客户端与 `gui-vite/` 同时存在，连同一个 axum server
（同一套 HTTP/SSE 协议），逐页替换、随时回退。tauri/ 在 gui-vite 退役前必须保留。

## 实测数据（本机 Windows，非调研推测）

| 指标 | 实测值 | 备注 |
|---|---|---|
| Rust 版本要求 | 1.90+（本机 1.99.1） | gpui-kit 的门槛 |
| 依赖下载 | 53.3 s | 阿里云镜像 |
| **冷编译** | **195.8 s（3m16s）** / 428 个编译单元 | 含 GPUI 走 release 级优化的 profile |
| **改一行 → 重编+链接** | **2.2 s** | 日常 UI 迭代的真实成本 |
| 整文件重写 | 5.9–6.7 s | |
| `target/` 体积 | 2.87 GB | |
| 内存（空闲） | 76–90 MB 工作集 / 94–125 MB 私有 | 对比 Tauri+WebView2 通常 150–300MB |
| exe（debug） | 42 MB | |
| **流式追加**（1287 字符，322 次 push） | 单次最大 **45–88 µs**，慢帧 0 | 60fps 帧预算 16.7ms，占 0.3–0.5% |
| 流式期 CPU | ~35%（含渐显动画持续重绘） | 无动画时 ~13% |

> **重要**：调研阶段流传的「GPUI 首编 10–20 分钟」不适用——那是 git 依赖整个 Zed 仓库的情形。
> 走 crates.io 快照（`gpui-kit` → `gpui-pre 0.3.6`）是好几个数量级的差别。

## 已验证的能力

- [x] 窗口启动、尺寸/居中/标题（`Bounds::centered` + `TitlebarOptions`）
- [x] **中文 IME 全链路**：微软拼音组合 → 候选 → 上屏；实测 `abc` 被组合成「爱彼此」、整句中文逐字一致
- [x] **原生 Markdown 渲染**：标题/列表/行内代码/链接/**表格**/代码块（tree-sitter 高亮）/引用块
- [x] **流式追加**：`TextViewState::push_str`，性能见上表
- [x] **流式跟随滚动**：`state.list_state().scroll_to_end()`
- [x] **流式渐显**：`TextViewMotion::with_stream_fade(320ms).with_stream_fade_stagger(35ms)`
- [x] 多行输入框 + 内容可被程序读取（`observe` + `value()`）
- [x] 深色主题统一（`Root` + `Theme::change`）

## 踩坑记录（复用价值最高，按命中顺序）

| # | 坑 | 现象 | 正解 |
|---|---|---|---|
| 1 | **样式不是 CSS 继承制** | 父 `div` 设 `text_color` 不穿透子组件 → 黑字压深底，几乎看不见 | 用 `Theme` 语义色，别手写颜色 |
| 2 | **窗口第一层必须是 `Root`** | 否则背景/弹层/焦点管理都不对 | `cx.new(\|cx\| Root::new(view, window, cx))` |
| 3 | **主题默认浅色** | 深色背景下表格、代码块白得刺眼 | `Theme::change(ThemeMode::Dark, None, cx)` |
| 4 | **`Bounds::centered` 要 `&App`** | 在 `cx.spawn` 闭包里只有 `&mut AsyncApp`，编译不过 | 先在 `run` 回调（`&mut App`）里算好 bounds 再进 spawn |
| 5 | **`TextViewMotion` 默认全 `ZERO`** | 渐显"看不到"——源码注释：*every duration defaults to zero, so an unstyled view adopts new content at once* | 显式 `set_motion(...with_stream_fade(...))` |
| 6 | **流式跟随要手动做** | 内容变长，滚动条停在原地 | 每次追加后 `state.list_state().scroll_to_end()`；注意 `AutoScroll` 是**拖拽选择**用的，不是流式跟随 |

## API 备忘（实测签名，省下一次翻源码）

```rust
// 窗口
cx.open_window(WindowOptions { window_bounds, titlebar, ..Default::default() }, |window, cx| {...})
WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(840.)), cx))  // cx 必须是 &App

// 文本视图
TextViewState::markdown("", cx)                 // 可变 state，用于流式
state.push_str(chunk, cx) / set_text("", cx)    // 追加 / 重置
state.list_state().scroll_to_end()              // 跟随底部
state.set_motion(TextViewMotion::default().with_stream_fade(d).with_stream_fade_stagger(d))
TextView::new(&entity).scrollable(true)         // 渲染可变 state
TextView::markdown(id, text)                    // 一次性静态渲染（只要 2 个参数，不要 window/cx）

// 输入框
TextareaState::new(window, cx)                  // 注意是 2 个参数
Textarea::new(&entity).h(px(110.)).w_full()     // 无 .placeholder()

// 异步
cx.background_executor().timer(Duration::from_millis(25)).await
entity.update(&mut app, |state, cx| {...})      // 返回 ()，不是 Result
```

## 已知行为（不是缺陷）

- **表格在流式中会有一次「突然变化」**：Markdown 必须解析到 `|---|` 分隔行才能确定列结构，
  越过该点时必然重排一次。代码块没有突变（``` 一出现即可确定边界）。
  真实聊天里可在流式期间对表格做「占位延迟渲染」抹平，属应用层策略。

## 未验证项（决定是否推进前必须补齐）

| # | 项目 | 为什么重要 |
|---|---|---|
| 1 | **长会话虚拟列表**（几千条消息） | 天演会话可能上万条；`gpui::list` 的实际表现未测 |
| 2 | **代码编辑器**（替代 CodeMirror 的 FileViewer/DiffView） | gpui-component 自称 200K 行稳定 + LSP，未实测 |
| 3 | **文件拖拽**（拖入文件到输入框） | 天演现有能力，GPUI 侧未验证 |
| 4 | **系统集成**：托盘 / 通知 / 全局快捷键 / **自动更新** / MSI 打包 | tauri/ 那 1163 行的 B 类职责需逐个接管；**updater 与打包是最大工作量** |
| 5 | 候选窗定位（IME 选字框是否贴光标） | 只能人眼验；上屏正确已验，位置未确认 |

## 怎么跑

```powershell
cd gui-native
cargo run            # 首次约 3 分钟，之后增量 2-6 秒
```

当前 `src/main.rs` 是一个验证程序：窗口内上半部分是循环重播的流式 Markdown
（每 25ms 追加 4 字符，1287 字符/轮），下半是输入框（输入内容会打印到终端）。

---

## MSI 打包与自动更新（2026-10 实测）

这一节回答「去掉 Tauri 后，发布流程还能不能成立」。结论：**能，但要自己编排一层**。

### 打包路径：WiX v6（不是 cargo-wix）

| 方案 | 结论 |
|---|---|
| `cargo-wix` | ❌ 依赖 WiX **v3** 的 `candle.exe`/`light.exe`，最新版停在 2025-03 |
| `cargo-dist` / `dist` | ⚠️ 仍用 WiX v3；其 updater `axoupdater` 依赖 install receipts，与 MSI 分发不匹配 |
| **WiX v6（dotnet tool）** | ✅ `dotnet tool install --global wix --version 6.0.2` → `wix build -o x.msi x.wxs` |
| WiX v7 | ⚠️ **引入 OSMF 收费条款**：年收入 > $10,000 的组织必须赞助；低于此额度的个人/项目免费但需显式 `-acceptEula wix7`。**用 v6 可彻底绕开这个法律承诺** |

实测：`wix build` 打 42.9MB 的 debug exe → **9.34MB MSI，耗时 18 秒**。

### 静默安装 / 升级实测

| 场景 | 结果 |
|---|---|
| perMachine（现状，`ALLUSERS=1`） | 需管理员提权 → 更新必弹 UAC |
| **perUser（`Scope="perUser"`，装到 `%LOCALAPPDATA%\Programs\`）** | ✅ **`msiexec /i /qn` 静默安装成功，1.3 秒，无 UAC** |
| MajorUpgrade（0.1.1 → 0.1.2） | ✅ 正确替换，注册表仅一条目，**无新旧并存** |
| 查询已安装版本 | ✅ `WindowsInstaller.Installer` COM 的 `ProductInfo(..,'VersionString')` |
| **应用运行中升级** | ⚠️ msiexec 退出码 0、版本已升，**但进程被 Restart Manager 直接杀掉**（无提示，也不帮你重启） |

> 现状：生产版 `Tianyan 1.0.1` 是 perMachine，装在 `D:\Program Files\Tianyan\`。
> 改成 perUser 可换来「更新零 UAC」，代价是多用户机器上需各自安装。

### 自研 updater 必须做的编排（约 200–400 行）

`msiexec` 不能单独调用——必须自己包一层：

1. 检查更新（HTTP 拉 release 清单，比对版本）
2. 下载 MSI + 校验（sha256，或沿用 minisign 签名）
3. **主动优雅关闭自己**（保存状态；否则会被 Restart Manager 强杀）
4. 以分离进程调 `msiexec /i <msi> /qn /norestart`
5. 成功后**重新启动新版本**；退出码 `3010` 表示需重启
6. 失败要可见（日志 + 提示），MSI 自身有事务回滚

### 仍未验证

- 代码签名（现状 `certificateThumbprint: null` → 未签名，SmartScreen 会警告；换方案不改变这点）
- 多语言 MSI（现状 Tauri 出 `zh-CN` / `en-US` 两份，WiX 需配 culture 文件）
- 图标 / 开始菜单 / 桌面快捷方式 / 安装目录选择

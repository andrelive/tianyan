# ADR-052: GUI 自动化端口（`GET /state` + `POST /act`）

**日期**: 2026-10-27
**状态**: ✅ 已采纳（阶段 1 已实施：状态 + 语义动作；阶段 2「截图」待实测）
**影响范围**: 新模块 `gui-native/src/automation.rs`、`gui-native/src/main.rs`（接线约 10 行）、
`gui-native/Cargo.toml`（+`axum`）、`gui-native/scripts/{ui-shot,ui-click}.ps1`（退役条件见末尾）

---

## 背景

### 1. 现状：靠进程外抓屏 + 坐标点击验证 GUI

gui-native 的功能验证一直是「PowerShell 抓屏 + `SendInput` 点坐标」
（`gui-native/scripts/ui-shot.ps1` / `ui-click.ps1`）。三个结构性缺陷，全都是实测踩过的：

| 缺陷 | 实测表现 |
|---|---|
| **DPI** | 屏幕 2560×1440 @150%，PowerShell DPI-unaware → `CopyFromScreen` 只抓到 863×571 的逻辑区域；目标控件在抓取范围外，连续三轮「探针 `sha256` 相同」全是假阴性 |
| **窗口状态** | 最小化窗口 `GetWindowRect` 返回假值（约 237×39），必须先 `ShowWindow(SW_RESTORE)`；被遮挡则抓不到内容 |
| **坐标 = 布局的耦合** | 点击点要按当前布局算，布局一改脚本全废；而且验证结论只能靠人眼看图推断——**锁屏时连看都看不了** |

### 2. GPUI 已经把原语内建（`gpui-pre 0.3.6` 源码核实）

| 能力 | API | 位置 | 门槛 |
|---|---|---|---|
| 输入注入 | `Window::dispatch_event(PlatformInput, cx)` | `window.rs:5415` | **pub，生产可用，无 `cfg` 门控** |
| 窗口截图 | `Window::render_to_image() -> RgbaImage` | `window.rs:2713` | `test-support` feature |
| 帧同步 | `AsyncWindowContext::on_next_frame` | `app/async_context.rs:324` | pub |
| 回主线程 | `AsyncApp::update` / `WeakEntity::update`（`C: AppContext`） | `async_context.rs:170` / `entity_map.rs` | pub，`AsyncApp: AppContext` |
| 窗口句柄 | `Window::window_handle()` | `window.rs:2267` | pub |

`render_to_image` 的 **Windows 实现是真的**（`gpui-pre-windows/src/window.rs:1038` →
`directx_renderer.rs:413` 读回显存），源码注释原话：

> This does not present the frame to screen — useful for visual testing where we want to
> capture what would be rendered without displaying it or **requiring the window to be visible**.

也就是说，「DPI 换算 / 窗口必须在前台 / 最小化要恢复」这三个坑，是**进程外抓屏方案的
衍生物**，换到进程内从机制上就不存在。

### 3. 但真正缺的不是截图，是「断言」

看图是最后一档手段。想知道的其实是精确值：现在在哪个页、选中了哪个会话、
消息正文渲染出来没有、目录选择器进到第几层、diff 有几个文件几行增删。
这些在 GUI 内部本来就是**精确的**，只是没出口——于是被迫降级成「截个图我看看」。

---

## 决策

### D1. 三层能力，按价值分层；阶段 1 只做前两层

| 层 | 接口 | 价值 | 依赖 |
|---|---|---|---|
| 1. 状态 | `GET /state` | **断言**，不靠看图；锁屏也能验证 | 零 |
| 2. 动作 | `POST /act` | 不依赖坐标与布局 | 零 |
| 3. 截图 | `POST /shot`（阶段 2） | 出问题时的现场存档 | `test-support`（连带 `proptest` / `leak-detection`） |

阶段 2 单独门控：`test-support` 会拉一堆测试期依赖，不能默认进构建。

### D2. 语义动作优先，坐标注入留作兜底

`dispatch_event` 能注入 `MouseDown { position }`（`MouseDownEvent` 字段全 pub，已核实可构造），
但坐标又要从布局算——把第 3 个缺陷原样搬回来。我们自己的 UI 有明确的语义入口
（`open_session` / `load_picker_dirs` / `load_workspace_diff` …），按语义暴露更稳，
断言也更强（「picker 在 `src/api` 这一层」比「点 400,320 之后屏幕上应该有个列表」可靠得多）。

阶段 1 **完全不碰 `dispatch_event`**；它留作兜底，只用于「点遮罩关模态框」
这种只有坐标意义的动作。

### D3. HTTP，不是 MCP

**概念对，协议不必。** 三条理由：

1. **项目里 `mcp/` 是客户端**（天演连外部 MCP server）。做 server 要手写 JSON-RPC 信封，
   没有现成骨架。
2. **跨进程是既有事实**：ADR-051 单 server 复用 → gui-native 可能复用**另一个进程**的
   server，而 agent 跑在 server 进程里。「agent 操作 GUI」天然跨进程，HTTP 正好；
   调用方是能执行命令的 agent，`curl` 已是最薄客户端。
3. **MCP 表达不了「等」**：动作之后要「等异步数据落地再读」，`request / response`
   模型没有这个位置。本实现用 `settle_ms` 表达（在 tokio 侧睡，不阻塞 UI）。

**形状对齐 MCP tools**（`action` + JSON 参数 + 结构化结果）：将来要给 Cursor /
Claude Desktop 这类外部 agent 操作天演界面，套一层薄适配器即可，不用重写。

准确类比：**这是「GPUI 版 CDP」**。Playwright 内部也不是 MCP——MCP 只是它的一层外壳。

### D4. 线程模型：主线程独占 + 指令队列

GPUI 的 `App` / `View` 只能在 UI 线程碰，而 HTTP 在 tokio 线程。做法：

```text
HTTP handler ──send(Envelope{cmd, oneshot})──▶ mpsc ──▶ UI 主线程消费循环
                                                        ├─ Cmd::State → automation_state()
                                                        └─ Cmd::Act   → automation_act()
     ◀───────────────── oneshot ◀──────────────────────────────┘
```

不用 `AsyncApp::update` 的 park 机制做跨线程借用。主线程独占的好处是
**天然把「动作 → 读状态」串行化**——这正是 D5 的前提。

### D5. `/act` 返回动作之后的状态

省一次往返，并保证「带回的状态一定反映了该动作」（同一个队列、同一顺序）。
`settle_ms` 让调用方表达「再等等异步数据」：动作发出 → 睡 → 再取一次状态。

### D6. 默认关闭、只绑回环、不进生产

- 只有 `TIANYAN_GUI_AUTOMATION_PORT` 能解析成 `u16` 才监听；未设 → `maybe_start`
  立即返回，**代码路径不执行**。
- 绑 `127.0.0.1`（与内嵌 server 同口径），不对外暴露。
- 生产 MSI 不带该环境变量。
- `automation_state` **不含 `api_key`**（`settings.providers` 只给 `name` / `endpoint`）：
  把界面摊开给外部读是一回事，把密钥摊开是另一回事。

### D7. 不碰 VFS（AGENTS.md 硬块约束）

这是 **IPC seam**，不是存储抽象：无持久化、无新 trait、不进主 workspace
（gui-native 自带 `[workspace]`）。`state` 是派生视图，不是第二份权威。

### D8. 动作只转发现有方法，不新造逻辑

`apply_action` 的每个分支都调既有方法（`open_session` / `load_*` / `confirm_*` / `send_text`）。
**若在这里加一条「自动化专用」捷径，真实点击路径与验证路径就分叉了**——
「用它验证过 = 用户点过也一样」这个前提随即不成立，验证结论作废。
这条纪律比代码本身重要，写在模块文档里。

---

## 接口

### `GET /state[?settle_ms=N]` → `{ok, state}`

`state` 顶层键：

| 键 | 内容 |
|---|---|
| `view` / `views` | 当前页 / 全部一级入口标识 |
| `status` / `streaming` / `port` | 状态行、是否有轮在跑、内嵌 server 端口 |
| `session` | `current` / `title` / `count` |
| `messages` | `loaded` / `total` / `last{role, chars, head(160字), segs, tool_calls, compression}` |
| `input` | 输入框当前文本 |
| `sessions_rows` | 左栏混排行（`kind: group/session/more`，含 `collapsed` / `hidden` / `selected`） |
| `sessions_data` | 会话原始数据（含 `working_directory` / `updated_at`） |
| `collapsed_groups` / `expanded_groups` | 分组折叠态 / 组内额外展开条数 |
| `dialog` | `{kind[, target]}` 或 null |
| `picker` | `open` / `current` / `parent` / `names` / `entries` / `status` |
| `workspace` | `root` / `path` / `status` / `entries[]` / `file{path,lines,chars}` / `diff{open,status,files[],current{path,lines,added,removed}}` |
| `tasks` | `collapsed` / `expanded` / `visible` / `all` / `items[]{id,kind,status,description,output_tail_chars,error,activity}` |
| `todos` / `goals` / `todo_bar_collapsed` | 会话任务与目标 |
| `approvals` / `clarification` | 待审批、待回答追问（含已选答案） |
| `roles` | `count` / `query` / `selected` / `detail{system_prompt_chars, system_prompt_head, tools}` / `stats` / `items[]` |
| `settings` | `status` / `menu_open` / `session_model` / `current_model` / `models[]` / `providers[]{name,endpoint}` |
| `usage` | `ctx_used` / `ctx_window` / `ctx_pct` / `session{input_uncached,input_cached,output}` / `request{ttft_ms,duration_ms,completion_tokens,tokens_per_sec}` |

以前只能看图才能确认的几项，现在是字段：`messages.last.head`（正文渲染）、
`sessions_rows`（分组折叠 / 查看更多）、`picker.current`（选择器层级）、
`workspace.diff.current.added|removed`（diff 染色行数）、`tasks.items[].activity`
（子代理活动流拉到了几条消息）。

### `POST /act` → `{ok, action, state[, error]}`

载荷 `{"action": "...", ...参数, "settle_ms": 300}`。

| 分类 | 动作 |
|---|---|
| 导航 | `switch_tab`(view) · `select_session`(id) · `new_session` · `reload` |
| 对话 | `send_message`(text) · `stop_turn` |
| 左栏 | `toggle_group`(key) · `expand_group`(key) |
| 目录选择器 | `open_picker`(target=`session`/`new_session`) · `picker_enter`(name 或 path) · `picker_up` · `picker_confirm` · `picker_cancel` |
| 文件页 | `ws_enter`(path) · `ws_open_file`(path) · `ws_diff_toggle` · `ws_diff_open`(path) |
| 任务 | `expand_task`(id) · `tasks_collapse`(collapsed) · `cancel_task`(id) |
| 待办/审批/追问 | `toggle_todo_bar` · `respond_approval`(decision=`approve`/`deny`[, request_id]) · `answer_clarification`(answers[]) |
| 角色 | `select_role`(name) · `reset_role`(name) · `delete_role`(name) |

未知动作返回 `ok:false` + `error`，不 panic、不静默。

### 实现落点

- `gui-native/src/automation.rs`（约 620 行含文档与注释）：HTTP 层（axum 4 路由）、
  主线程桥、`impl ClientApp` 的 `automation_state` / `automation_act` / `apply_action`。
  **放在一个文件里**是刻意的：同一 crate 的后代模块可访问 crate root 的私有字段，
  所以状态序列化不必给 `ClientApp` 的字段加 `pub`，也不会散落在 `main.rs` 里。
- `gui-native/src/main.rs`：`mod automation;` + `app.run` 里 `cx.to_async()` 取出
  `AsyncApp`，在 `open_window` 的构建闭包里 `automation::maybe_start(&async_cx, view.downgrade())`。
- `View` / `Dialog` / `PickerTarget` 各加一个 `key()`（稳定标识 = 对外契约值）。

---

## 实施与实测

**编译**：`cargo check` / `cargo build` 零警告。`automation.rs` 约 620 行（含文档与注释），
`main.rs` 接线约 20 行（`mod` + `AsyncApp` 取出 + `maybe_start` + 抽出的 `switch_view`）。

**实测**（2026-10-27，Windows 11 / 2560×1440 @150%，`TIANYAN_GUI_AUTOMATION_PORT=3010`，
`gui-native/scripts/automation-smoke.ps1` 全绿、20 项断言）：

| 项 | 结果 |
|---|---|
| `/health` | `{service:"tianyan-gui", pid}` —— 能区分「这个端口上是 GUI」而不是别的本地服务 |
| 状态 | `session.count=49`；左栏派生行 37 条（group 10 / session 22 / more 5）；11 个一级入口 |
| 消息断言 | `messages.loaded=60/72`；最后一条 `role=assistant chars=308 segs=2 tool_calls=1` + **正文头** |
| 文件页 | 根 40 项 → 进 `.cargo`(1) → 回根(40) → 开 `.env.example`(58 行) |
| diff | 34 个改动文件（与本轮改动吻合：`.gitignore` 39→42 行、`AGENTS.md` 268→275、`Cargo.lock` 12023→12038…）；单文件 unified diff 11 行 `+3 −0` |
| picker 内层 | 浏览根 → `F:\`(27) → `F:\work`(21) → `F:\work\git`(53) → `F:\work\git\tianyan`(26)；`picker_up` 逐级回退 |
| **真实绑定** | `picker_confirm` → `PUT /sessions/{id}/workspace` 生效（`working_directory` 同值），目录树按新根重拉 |
| 错误路径 | 未知动作 / 未知视图 / 缺参数 / 选择器未打开 → 全部 `ok:false` + 中文 `error`，不 panic |

**锁屏可用**：整套验证不依赖窗口可见（不截图、不取窗口矩形），锁屏状态下全部跑通 ——
这正是原方案做不到的（`ui-shot.ps1` 在锁屏时只能出假图，甚至因最小化窗口 rect 是假值
而点到空处）。

### 实测中挖出的两处分叉（D8 的价值验证）

1. **`switch_tab` 与侧栏点击分叉**：`switch_tab` 起初只赋 `self.view`，而「页内首次进入
   才补拉数据」的逻辑**内联在侧栏的 `on_click` 闭包里**，没有独立方法可转 → 点侧栏会
   拉数据、自动化切页不会，文件页树一直是空的。
   **修法**：抽 `ClientApp::switch_view(view, cx)`，点击闭包与 `switch_tab` 共用。
   —— 这正是 D8 警告的情形，**实施第一天就撞上了**。
2. **toggle 语义对自动化不友好**：`ws_diff_toggle` 第一轮打开、第二轮（想确认仍然开着）
   又关掉，`files` 恒为 0，差点被误判成「diff 拉取坏了」。
   **修法**：`ws_diff` / `todo_bar` 支持 `open` / `collapsed` 显式 set，不带参数才取反
   （与已有的 `tasks_collapse` 口径统一）。**凡是状态查询会看到的开关，都该有 set 语义。**

## 代价与遗留

1. `gui-native` 多一个 `axum` 依赖（服务端已在用同版本 0.8，编译产物可复用缓存）。
2. 状态快照是**手工序列化**，新增内部状态要同步加字段。这是故意的：哪些状态算
   「对外可断言的」需要判断，不是无脑 dump 整个 `ClientApp`。
3. 阶段 2 `/shot` 要开 `test-support`，会连带 `proptest` / `leak-detection`；
   Windows 上 `render_to_image` 的**实际可用性尚未验证**（源码有实现 ≠ 能跑通），
   所以单独实测后再定。**在此之前 `ui-shot.ps1` 保留**。
4. `ui-click.ps1` 在阶段 1 完成后即可退役（语义动作已覆盖它的全部用途）。

## 否决

- **走 MCP / JSON-RPC**：见 D3。
- **坐标注入为主**：见 D2。
- **进程内直调（不经 HTTP）**：agent 与 GUI 可能不同进程（ADR-051）。
- **把 state 做成 `#[derive(Serialize)]` 全量 dump**：等于把内部结构（含 `api_key`
  一类字段）无审查地摊开，且任何字段改名都会静默改变对外契约。

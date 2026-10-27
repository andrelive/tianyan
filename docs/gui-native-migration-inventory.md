# 天演 GUI 迁移盘点：Web（gui-vite）→ GPUI 原生（gui-native）

> 性质：**调研盘点稿**（模块盘子 + 端点核实 + 组件可行性 + 排期建议）。本文只记录事实与判断，不含任何代码改动。
> 数据来源：全部为仓库内代码的直读结果（文件:行号见 §8 证据索引），端点存在性一律以 `server/src/**/routes.rs` 为准，不采用记忆或推测。
> 对照的两个客户端：`gui-vite/`（React 19 + Tauri + zustand，17 个业务模块）与 `gui-native/`（GPUI，`gpui-kit 0.6` = `gpui-component 0.6.6` + `gpui-pre 0.3.6`）。

---

## 0. 一句话结论

17 个业务模块合计约 **13.4k 行 tsx**（不含 `__tests__`），前端实际调用 **46 条后端端点，全部在 server 侧存在**，没有"前端有、后端没有"的空接口；反方向上存在 3 处"后端有、前端没用/没用全"的缺口（`/tasks/{id}/log`、`/workspace/apply-edit`、`/traces`）。

GPUI 侧的组件覆盖比普遍预期更好：**树 / 表格 / Tab / 表单 / 设置页骨架 / 代码编辑器 / 消息与滚动器 / 对话框 / 通知 / 分栏** 都有现成组件，需要自组装的只有 3 类：**双栏 diff 视图**、**富文本输入（@文件 chip + 图片粘贴）**、**托盘/系统通知/单实例/updater**。

---

## 1. 两侧现状

### 1.1 Web 侧（gui-vite）规模

| 区域 | 内容 | 行数量级 |
| --- | --- | --- |
| `src/components/` 业务面板（17 个模块） | chat / session / sidebar / settings / wizard / workspace / approval / 后台任务 / todo-goal / scheduled / insights / knowledge / memory / skills / roles / tools / clipboard | **≈ 13 400** |
| `src/components/ui` + `layout`（通用件） | Toast / ErrorBoundary / Modal / ConfirmDialog / Spinner / EmptyState / ErrorBanner / FieldRow / ListDetailPanel / ImageLightbox | ≈ 580 |
| `src/lib`（数据层） | `types.ts` 1216、`api-client.ts` 953、`config-transform.ts` 558、`chat-stream.ts` 228、`attachments.ts` 287 等 | ≈ 4 400 |
| `src/lib/store`（zustand 三个 slice） | chat-slice 858 / model-slice 93 / ui-slice 55 | ≈ 1 070 |
| `src/hooks` | `use-unified-events` 278、`use-session-history` 142、`use-resource` 93、`use-polling` 81、`useChatStream` 83 等 | ≈ 660 |

第三方 UI 依赖（决定了迁移时"要拿什么替代"）：`react-arborist`（文件树）、`@codemirror/*`（编辑器与 diff 高亮）、`lexical`（富文本输入）、`react-markdown`+`remark-gfm`、`react-virtuoso`（消息虚拟滚动）、`diff`（jsdiff，生成 unified patch）、`lucide-react`（图标）、`zustand`。

### 1.2 原生侧（gui-native）已有能力

单文件 `gui-native/src/main.rs`，**864 行**，一个 `ClientApp` 视图。已实现：

- 会话列表（`List` + `ListDelegate`）、点选打开；
- 消息渲染（`TextView::markdown`，表格/代码块/高亮）；
- 输入发送（`Textarea` + Enter 发送 / Shift+Enter 换行）；
- SSE 常驻订阅（`GET /api/v1/events`，按字节分帧、1s 断线重连）+ `POST /chat/stream` 发送开关；
- 停止（`POST /chat/streams/{id}/cancel`）、`turn_state` 驱动 `streaming` 态；
- 消息历史：`GET /sessions/{id}/messages?limit=200`（**全量式**，非分页）。

已调用的端点共 **5 条**：`GET /sessions`、`GET /sessions/{id}/messages`、`POST /chat/stream`、`POST /chat/streams/{id}/cancel`、`GET /events`。

**尚未**：`/events/subscribe` 快照恢复、会话 CRUD/分组、模型选择、审批、任务、workspace、设置、一级导航壳、首启配置判定。

---

## 2. 模块总表（17 个业务模块）

难度评级口径：**低** = 纯只读列表/表单，组件齐备，≤1 天级；**中** = 需要 2 个以上自组装交互（轮询 + 展开 + 对话框 + 分页）或中等数据层；**高** = 需要自组装缺失组件（diff / 富文本 / 编辑器集成）或大配置域建模。

| # | 模块 | 目录 · 主文件（行） | 职责（一句话） | 主要后端端点（方法 + 路径，均已核实存在） | 关键 UI 形态 | GPUI 侧可用组件 | 难度 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | **chat 会话主面板** | `components/chat/`（≈4.0k；ChatPanel 955、ChatInput 530、ComposerEditor 536） | 一条会话的完整生命周期：消息流 → 工具/技能卡片 → 追问接管 → 审批横幅 → 模型/思考档位 → 上下文占用 → 回退/压缩 | `POST /chat/stream`、`POST /chat/answer`、`POST /chat/streams/{session_id}/cancel`、`GET /events`、`POST /events/subscribe`、`GET /sessions/{id}/messages`、`POST /sessions/{id}/messages/delete`、`POST /sessions/{id}/messages/redo`、`POST /sessions/{id}/compress`、`GET|PUT /sessions/{id}/model`、`GET /config/models`、`GET /usage/stats`、`GET /usage/recent`、`GET /approval/status`、`POST /approval/respond`、`GET /tasks` | 虚拟滚动消息流 / Markdown+代码块 / 气泡 / 折叠工具卡 / 富文本输入（@文件 chip、图片）/ 下拉选择 / 圆环进度 / Tab 条 / 横幅 | `list::List`+`text::TextView::markdown`+`bubble::Bubble`/`message::Message`+`message_scroller`+`select::Select`+`tab::TabBar`+`input::Textarea`；**富文本需自组装** | 高（正在做） |
| 2 | **session 会话列表** | `components/session/`（599；SessionList 581） | 左栏二级列表：按工作目录分组、24h 活跃段、折叠加载更多、重命名/删除、运行中与后台任务角标 | `GET /sessions`、`DELETE /sessions/{id}`、`POST /sessions/{id}/title`、`GET /tasks`、`GET /workspace/dirs` | 分组折叠列表 / 搜索 / 行内菜单 / 确认框 / 目录选择器 | `list::List`+`collapsible`+`dialog::Dialog`+`gpui::App::prompt_for_paths` | 中 |
| 3 | **sidebar 一级导航** | `components/sidebar/`（115） | 图标 rail，11 个一级入口 + 审批待办角标（5s 轮询） | `GET /approval/status` | 图标栏 / badge 角标 | `sidebar` 模块或自绘 rail + `badge` | 低 |
| 4 | **settings 设置页** | `components/settings/`（≈3.9k；SettingsPanel 475，tabs 16 文件 2 865） | 14 个 tab 的配置读写：模型服务 / 数据存储 / Agent 行为 / 人设 / 安全 / 日志 / 记忆 / 检索 / 外观 / 连接 / MCP / 用量 / Web 搜索 / 关于 | `GET /config`、`PUT /config`、`POST /config/test-connection`、`GET /config/provider-presets`、`POST /config/providers/scan`、`GET|PUT /config/soul`、`GET /config/soul/default`、`GET|POST /config/mcp/servers`、`PUT|DELETE /config/mcp/servers/{name}`、`POST /config/mcp/servers/{name}/test`、`POST /config/migrate-data-dir`、`GET /usage/stats` | 侧栏 Tab + 大表单（60+ 字段）+ 开关 + 下拉 + 数字输入 + 多值输入 + provider 卡片内嵌模型行 + 确认框 | `setting::{SettingPage,SettingGroup,SettingItem,SettingField}`+`form::Form`+`switch`+`select`+`input::NumberInput`+`tab::TabBar` | 高 |
| 5 | **wizard 配置向导** | `components/wizard/`（760；ConfigWizard 207 + 6 steps 492） | 首启"未配置"时的分步引导（欢迎 → 模型 → Agent → 数据 → Web → 确认） | `GET /config/status`、`GET /config/provider-presets`、`POST /config/providers/scan`、`PUT /config` | Stepper 分步 / 表单 / 下拉 / 确认页 | `stepper`+`form::Form`+`select` | 中（首版可降级为"最小配置表单"） |
| 6 | **workspace 文件区** | `components/workspace/`（1 640；Tree 292 / FileViewer 389 / DiffPanel 302 / Panel 255 / Picker 193 / DiffView 146） | 文件树 + 文件查看/编辑 + 快照 diff + 保存前 diff 确认 + 目录选择器 | `GET /workspace/tree`、`GET /workspace/dirs`、`GET /workspace/read`、`GET /workspace/diff`、`POST /workspace/apply-patch`、`PUT /sessions/{id}/workspace` | 懒加载树 / 代码编辑器（只读+编辑双模）/ 双栏 diff / 保存确认弹窗 / 目录选择器 / 二进制与分页提示 | `tree::tree(TreeState)`+`input::Editor`+`resizable`+`dialog`；**diff 视图需自组装** | 高 |
| 7 | **approval 审批面板** | `components/approval/`（410）+ `chat/ApprovalBanner`（60） | 待处理审批（风险分级 + 倒计时 + 允许/拒绝/要求更多信息 + 命令改写）、待确认列表、最近 10 条审计 | `GET /approval/status`（2s 轮询）、`POST /approval/respond` | 分区列表 / 风险 badge / 秒级倒计时 / 内联多行编辑 / 按钮组 / 空态 | `list::List`+`badge`+`input::Textarea`+`button`+自管 1s tick | 低 |
| 8 | **后台任务面板** | `chat/AgentTasksPanel`（286）+ `DelegateActivity`（276） | 会话内 dock：delegate 委托任务与 command 终端任务，活跃在上、完成沉底，展开看子代理消息或输出尾部，可取消 | `GET /tasks`（3s）、`POST /tasks/{id}/cancel`、`GET /events`（`task_status`/`command_output`）；`GET /tasks/{id}/log`（后端有、前端未用） | 可折叠 dock / 行内展开 / 状态图标 / 活动步 / 取消按钮 | `collapsible`+`list::List`+`spinner`；子代理消息复用 TextView | 中 |
| 9 | **会话待办·目标面板** | `chat/SessionTodoPanel`（171） | 输入框上方可折叠面板，展示本会话 todo/goal（**只读**，写入由 agent 的 todo/goal 工具完成） | `GET /todos?session_id=`、`GET /goals?session_id=`（3s） | 折叠面板 / 分组 + 子项缩进 / 完成态划线 / 目标进度条 | `collapsible`+`list::List`+`progress`（勾选只作状态样式） | 低 |
| 10 | **scheduled 定时任务** | `components/tasks/`（244；Section 217） | 间隔制定时智能体任务：列表 + 新建（预设/自定义秒 + prompt + 工作目录）+ 删除 | `GET /scheduled-tasks`、`POST /scheduled-tasks`、`DELETE /scheduled-tasks/{id}` | 列表 / 新建表单（下拉预设、数字、多行 prompt）/ 删除确认 | `select`+`input::NumberInput`+`input::Textarea`+`dialog` | 低-中（**无更新/启停端点**，只能删了重建） |
| 11 | **insights 洞察** | `components/insights/`（272） | 内置调度器任务状态（间隔/次数/上次执行/错误）+ 使用统计摘要（技能调用、文档访问、搜索热度） | `GET /scheduler/status`、`GET /stats` | 卡片网格 / 12 栅格行列表 / badge / 空态 | `table::Table` 或自绘栅格 + `badge`；`chart` 可用但当前 UI 无图表 | 低 |
| 12 | **knowledge 知识库** | `components/knowledge/`（889；Search 259 / Ingest 243 / Browse 292 / Panel 95） | 三 tab：语义搜索（含建议）、文档摄入（多文件上传）、条目浏览（目录树 + L0/L1/L2 分级内容 + 删除） | `GET /knowledge/search`、`GET /knowledge/search/suggestions`、`POST /knowledge/ingest`（multipart）、`GET /knowledge/entries`、`GET /knowledge/entries/read`、`POST /knowledge/entries/delete` | Tab / 搜索建议下拉 / 拖放+多选文件上传 / 目录树 / 分级正文 / 确认删除 | `tab::TabBar`+`tree`+`text::TextView::markdown`+`gpui::App::prompt_for_paths`+`reqwest::multipart` | 中 |
| 13 | **memory 记忆** | `components/memory/`（258） | 左分类树 + 右详情，L0 简介 / L1 目录 / L2 正文 层级切换（8s 静默轮询） | `GET /memory` | 分类树 / 层级切换 / Markdown 正文 | `tree`/`list`+`tab`（分段按钮）+`TextView` | 低-中 |
| 14 | **skills 技能** | `components/skills/`（260） | 方法论技能只读浏览（列表 + 搜索 + 详情 markdown + 调用统计） | `GET /skills`、`GET /skills/{id}`、`GET /skills/stats` | 列表/详情双栏 / 搜索 / Markdown | `resizable` 双栏 + `list::List`+`TextView` | 低 |
| 15 | **roles 子智能体** | `components/roles/`（499） | 角色库：列表 + 详情（类型/来源/统计）+ 重置 + 删除 | `GET /roles`、`GET /roles/{name}`、`GET /roles/stats`、`POST /roles/{name}/reset`、`DELETE /roles/{name}` | 列表/详情双栏 / 搜索 / badge / 确认删除 | `resizable`+`list::List`+`dialog`+`badge` | 低-中 |
| 16 | **tools 工具目录** | `components/tools/`（128） | 系统工具目录只读（与 LLM tools 同源） | `GET /tools` | 列表 + 搜索 + 详情 | `list::List`+`text` | 低 |
| 17 | **clipboard 剪贴板确认** | `components/clipboard/`（110） | 底部确认条：把刚复制的内容「沉淀为记忆 / 存入知识库 / 忽略」（捕获动作由宿主进程发起） | `GET /clipboard/pending`、`POST /clipboard/respond`；宿主侧 `POST /clipboard/capture` | 底部条 / 三按钮 | 自绘 bar + `button`；**宿主需补系统剪贴板轮询**（`tauri/src/lib.rs` 有 60 行参考实现） | 低（UI）/ 宿主侧另计 |

> 说明：`components/ui`、`components/layout` 是两个通用件目录（不是业务模块），已计入 §1.1 的"通用件"行。
> 后端已有 `GET /api/v1/traces`（`server/src/api/traces/routes.rs:13`），但 **Web 前端没有对应页面**（`gui-vite/src` 全库 grep 无 `/traces`），因此不在 17 个模块内 —— 属"后端就绪、前端空白"。

---

## 3. 后端端点核实（逐项回答"是否存在"）

| 你问的能力 | 是否存在 | 实际路径与方法 | 证据（server 代码） |
| --- | --- | --- | --- |
| 配置读取 / 更新 | ✅ | `GET /api/v1/config`、`PUT /api/v1/config`；另有 `GET /config/status`、`GET /config/{section}`（**单节只读**） | `server/src/api/config/routes.rs:20-22` |
| 审批「允许 / 拒绝」提交 | ✅ 且**与 `/chat/answer` 不同** | `GET /api/v1/approval/status`；`POST /api/v1/approval/respond`（`request_id` + `decision` ∈ `approve`/`deny`/`request_more_info` + 可选 `reason`/`edited_command`） | `server/src/api/insights/routes.rs:26-27`；`server/src/api/insights/handlers.rs:251-302` |
| todo 读写 | ✅ | `GET|POST /api/v1/todos`；`PATCH|DELETE /api/v1/todos/{id}` | `server/src/api/todos/routes.rs:15-16` |
| goal 读写 | ✅ | `GET|POST /api/v1/goals`；`PATCH|DELETE /api/v1/goals/{id}` | `server/src/api/goals/routes.rs:15-16` |
| 后台任务列表 | ✅ | `GET /api/v1/tasks` | `server/src/api/tasks/routes.rs:19` |
| 命令日志 | ✅（前端未用） | `GET /api/v1/tasks/{id}/log`（分页，返回 `next_offset`） | `server/src/api/tasks/routes.rs:25`；`handlers.rs:211-262` |
| 定时任务 CRUD | ⚠️ **只有 C/R/D** | `GET|POST /api/v1/scheduled-tasks`；`DELETE /api/v1/scheduled-tasks/{id}`；**无 PATCH、无启停开关** | `server/src/scheduled_tasks/routes.rs:15-20` |
| workspace 文件树 | ✅ | `GET /api/v1/workspace/tree?path&depth&session_id` | `server/src/api/workspace/routes.rs:17` |
| workspace 文件读取 | ✅ | `GET /api/v1/workspace/read?path&offset&limit&session_id`（返回 hashline 文本 + 分页信息 + binary 标记） | `workspace/routes.rs:19`；`workspace/types.rs:88-110` |
| workspace 文件保存 | ✅ 但**不是"整文件写"** | `POST /api/v1/workspace/apply-patch`（unified diff 信封）；另有 `POST /api/v1/workspace/apply-edit`（`path` + `ContentEdit[]` 内容匹配，前端未用） | `workspace/routes.rs:20-21`；`workspace/types.rs:126-165` |

**核实过程中发现的 5 个坑（都会影响迁移排期与设计）**

1. **审批没有独立模块**：`/approval/*` 两个端点物理上挂在 `insights`（内部状态视图）里，`api/mod.rs` 的领域清单中没有 `approval`。找路由时不要按目录名找。
2. **`/chat/answer` ≠ 审批**：前者是 `ask_user` 追问工具的回答通道（工具挂起等待 → 回答恢复执行，`chat/routes.rs:12`），后者是工具执行门控的决策通道（有超时自动拒绝，默认 300s）。
3. **`/tasks/stream` 已删除**：`docs/api-endpoints.md:49` 仍写着"兼容入口"，但 `tasks/routes.rs` 注释明确写着"旧路径 `/tasks/stream` 已删除（ADR-028/030：并入统一流）"。**以 routes.rs 为准**，这类文档漂移要在迁移中一并修掉。
4. **保存文件没有 PUT 整文件端点**：只有补丁。Web 端是客户端用 jsdiff（`createTwoFilesPatch`）在本地生成 unified diff 再提交（`workspace/WorkspacePanel.tsx:106-112`）。原生侧要么用 `similar` 生成 patch，要么改走 `apply-edit`（更省事，但需先验证 `ContentEdit` 语义）。
5. **`GET /config/{section}` 是只读**，更新路径只有整包 `PUT /config` —— 意味着客户端必须持有完整配置模型，否则会把未建模字段写空/覆盖掉。

---

## 4. 四个优先模块深挖

### 4.1 设置页（settings）

**端点与数据结构**

- 读：`GET /config` → `{ config: TianyanConfig }`；写：`PUT /config` ← `{ config: TianyanConfig }`（整包）。
- 前端把后端嵌套结构扁平化成 `ConfigState`（`gui-vite/src/lib/types.ts:768-855`），双向映射在 `config-transform.ts`（558 行）。
- 需要建模的配置域（`ConfigState` 字段清单）：
  - `models.providers[]`：每项含 `name / endpoint / api_key / models[] / timeout / enabled / is_local / headers{} / thinking_field / dialect / first_token_timeout / user_agent / dynamic_headers[]`；`models[]` 每项含 `name / capabilities[] / reasoning_efforts[] / context_length / max_output_tokens / max_input_tokens`；
  - `models.preferences{chat,embedding,vision}` + 后端回填的只读映射 `resolvedSpecs / modelCatalog / resolvedEndpoints / providerPresets`；
  - `agent.*`（`default_top_k / max_turns / learned_rules_top_k / working_directory / background_self_review`）；
  - `storage.*`（`data_dir / storage_backend / sqlite_path / collection_name / vector_dimension / max_storage_size / auto_cleanup / cleanup_days`）；
  - `logging.*`（6 项）、`security.*`（11 项，含 `approval_mode` / `safety_mode` / `trash_directory` / 黑白名单逗号串）、`clipboard.*`（3 项）、`memory.auto_consolidation`、`retrieval.*`（7 项）、`web.*`（9 项）、`mcpServers[]`。

**GPUI 侧可行性**

- 骨架几乎现成：`component::setting::{SettingPage, SettingGroup, SettingItem, SettingField}`（`gpui-component-0.6.6/src/setting/page.rs:22`、`group.rs:16`、`item.rs:22`、`fields/mod.rs:62`）——正是"分组标题 + 一行一个字段"的设置页语义；`form::Form` 负责整体表单容器；`tab::TabBar` 管 14 个 tab。
- 字段控件齐备：`switch::Switch`、`select::Select`、`input::NumberInput`、`input::Textarea`（多行，如 soul 编辑）、`color_picker`（外观色）。

**缺口**

1. **数据层要重写**：TS 的 `ConfigState` + `fromBackendConfig/toBackendConfig`（558 行）需对应成 Rust struct + serde 双向映射（含"后端缺字段时给默认值"的容错），估 800–1 200 行。
2. **provider 卡片**：一张卡片内嵌"模型条目列表（可增删行 + 能力多选 + 数字字段）"+ 行内"测试连接/扫描模型"按钮 —— 需自组装（用 `Table` 或 `List` 嵌套），与 Web 端 ProviderCard 655 行等价。
3. **多值输入**（`allowed_commands` / `blocked_directories` 等逗号串）与"测试连接"的异步状态机（idle/testing/success/error 四态）需自写。
4. 保存成功后要触发 agent 热重载并反馈（`PUT /config` 返回 `{success,message}`），用 `notification` 弹出。
5. **不建议一次到底**：14 个 tab 里 `usage`（用量统计）依赖 `/usage/stats` 的时间范围组合、`storage` 依赖 `migrate-data-dir`（会触发服务器关停重启）——这两个可后置。

**工作量（行数量级）**

| 项 | Rust 行数 |
| --- | --- |
| 配置数据模型 + serde 映射（含默认值/容错） | 800–1 200 |
| 14 个 tab 页面（每 tab 80–250） | 1 500–2 200 |
| provider 卡片 + 模型条目编辑 + 测试/扫描 | 500–700 |
| 共享表单件（字段行、多值输入、脏标记） | 250–350 |
| **合计** | **≈ 3.5k–5k** |
| **可裁剪首版**（模型服务 + Agent + 安全 + 外观 4 个 tab） | **≈ 1.2k–1.6k** |

### 4.2 任务面板（todo / goal / 后台任务 / 定时任务）

四类"任务"在前端是**四个不同位置**，不是一个页面（`sidebar/Sidebar.tsx:19-31` 的注释就是权威分层）：

| 任务类型 | 归属位置 | 端点 | 数据源粒度 |
| --- | --- | --- | --- |
| 内置调度器任务（系统级） | 洞察页（`/insights`） | `GET /scheduler/status` | 只读状态（间隔/次数/上次/错误） |
| 定时智能体任务 | 独立一级栏目（`/scheduled`） | `GET|POST /scheduled-tasks`、`DELETE /scheduled-tasks/{id}` | 完整 CRUD（缺 U） |
| 后台任务（delegate / command） | **会话内 dock** | `GET /tasks`、`POST /tasks/{id}/cancel`、`/events` 的 `task_status`/`command_output` | 按 `parent_session_id` 归属到会话 |
| todo / goal | **会话内面板**（输入框上方） | `GET /todos?session_id=`、`GET /goals?session_id=` | 只读（写入靠 agent 工具） |

**数据结构（前端契约，与后端 DTO 同形）**

- `BackgroundTask`：`id / description / status(pending|running|completed|failed|cancelled) / kind?(delegate|command) / parent_session_id / result / error / output_tail? / created_at / completed_at / seq`（`types.ts:576-600`）。
- `TodoItem`：`id / title / description / status(pending|in_progress|completed) / priority(low|medium|high) / goal_id / parent_id / session_id / created_at / updated_at / completed_at / due_at`；列表响应 `{todos, total}`（`types.ts:1019-1045`）。
- `GoalWithProgress`：`goal{...} + progress(0-100) / todo_total / todo_done`；列表响应 `{goals, total}`（`types.ts:1046-1078`）。
- `ScheduledAgentTask`：`id / name / interval_secs / workspace / prompt / enabled / created_at / last_run_at / next_run_at / last_result`（`server/src/scheduled_tasks/types.rs:9-38`）。
- `SchedulerTaskStatus`：`id / name / priority / interval_secs / run_count / last_run_ago_secs / last_run_ago / next_due_in_secs / last_error?`（`types.ts:612-626`）。

**GPUI 侧可行性**

- 全部是"列表 + 折叠 + 徽标 + 按钮"，`list::List`、`collapsible`、`badge`、`progress`、`spinner`、`button` 齐备，没有缺失组件。
- **缺的是"定时刷新"这一层抽象**：Web 端有 `usePolling(fn, ms)`（81 行，含立即执行/静默刷新/失败保留旧数据）。GPUI 侧要写一个等价物（`cx.spawn` + `background_executor().timer` + `WeakEntity::update`），约 80–120 行，一次做好可复用到全部面板。各面板轮询节奏：后台任务 3s、todo/goal 3s、审批 2s、审批角标 5s、记忆 8s。

**缺口与风险**

1. `GET /tasks/{id}/log` 后端有、Web 没接 —— 原生侧可顺手做"查看完整命令日志"抽屉（增益不大，可后置）。
2. 委托任务展开要看**子智能体消息**：事件流里的子代理消息按 `task_id` 路由（会话是 `session_id`），需要在事件处理里建 `session_id → task_id` 两级字典（现有 `handle_event` 只按 `session_id` 过滤）。
3. `POST /events/subscribe` 只接受 `session_id`，没有"按 task 订阅"——子代理消息靠常驻 `GET /events` 推送，不要试图对 task 单独订阅。
4. **定时任务无更新/启停端点**：UI 只能"删了重建"。若要支持启停，得先补后端 `PATCH /scheduled-tasks/{id}`（后端改动，非前端工作量）。

**工作量（行数量级）**

| 块 | Rust 行数 |
| --- | --- |
| 后台任务 dock（列表 + 展开 + 取消 + 活动步） | 400–600 |
| todo / goal 面板（分组、缩进、进度条、折叠+缓存） | 200–300 |
| 定时任务（列表 + 新建表单 + 删除确认） | 300–400 |
| 轮询工具（一次性，全局复用） | 80–120 |
| **合计** | **≈ 1.0k–1.4k** |

### 4.3 审批面板（approval）

**端点与数据结构**

- `GET /api/v1/approval/status` → `ApprovalStatusSnapshot`：
  - `config`：`{ default_timeout_secs, enable_auto_approval, persist_records, max_pending_approvals, mode(autonomous|confirm|interactive), prompt_commands[] }`；
  - `pending_approvals[]`：`{ request_id, session_id, action_description, action{ action_type: "ExecuteCommand", command? , ... }, risk_level(Safe|Low|Medium|High|Critical), requested_at, timeout_secs }`；
  - `pending_confirmations[]`（字符串）、`recent_records[]`（`{request, response|null, execution_result|null, edited_command?}`）、`confirmed_action_count`。
- `POST /api/v1/approval/respond` ← `{ request_id, decision: "approve"|"deny"|"request_more_info", reason?, edited_command? }`；请求不存在或**已超时（默认 300s 自动拒绝）→ 404**；向导模式（未装配工作流）→ 500（`insights/handlers.rs:284-300`）。

**GPUI 侧可行性** —— 全绿

- 卡片列表 = `list::List` 或直接 `div()` 循环（条目少，无需虚拟化）；
- 风险等级 → `badge`（配色映射照抄 5 档）；
- 命令内联改写的编辑态 = `input::Textarea` + `button`（保存/取消）；
- 倒计时 = 自己 `cx.spawn` 每 1s `cx.notify()`（30 行）；
- 会话内横幅（`ApprovalBanner`，60 行）= 一个带边框的 `div()`，"允许/拒绝"两个按钮；
- 错误可见性：Web 端刻意让审批错误"显错 + 可重试"（不像任务面板静默）——原生侧用 `notification` + 行内 error banner 复刻。

**缺口** —— **无组件缺口**。唯一要注意的是：审批是否也经 `/events` 推送？现状 Web 端**只靠 2s 轮询**，原生照抄即可（不要自创事件类型）。

**工作量**：面板 400–600 行 + 会话内横幅 80–120 行（含倒计时、编辑态、审计列表折叠）。

### 4.4 workspace（文件树 / 编辑器 / 预览）

**端点与数据结构**

- `GET /workspace/tree?path&depth=1&session_id` → `{ root, path, entries[{ name, type: dir|file, path, size?, mtime? }] }`（单层列出，深度固定 1，靠前端递归展开）。
- `GET /workspace/dirs?path` → 目录选择器用的逐级浏览（`current / parent? / entries[{name,path,is_root?}]`）。
- `GET /workspace/read?path&offset&limit=2000&session_id` → `{ path, content?（hashline 文本 "N#ID|content"）, truncated?, total_lines?, showing{offset,limit}, binary?, size?, preview? }`。
- `GET /workspace/diff?base=snapshot&path&session_id&index|message_id` → 单文件 `{ path, status(modified|added|removed|binary|unchanged), hunks[{old_start,old_len,new_start,new_len}], unified, old_lines, new_lines }`；不带 `path` 返回文件列表。
- `POST /workspace/apply-patch` ← `{ patch, session_id? }` → `{ files[{path,hunks_applied,lines_changed}], total_files }`；冲突 → 409（前端用 `isConflict` 区分）。
- `PUT /sessions/{id}/workspace` ← `{ working_directory }`（绑定/解绑会话工作目录，空串清除）。

**GPUI 侧可行性**

| 子能力 | 判断 | 依据 |
| --- | --- | --- |
| 文件树（懒加载 + 展开态 + 图标 + 行内错误重试） | **有现成组件** | `component::tree::tree(&TreeState, render_item)`（`src/tree.rs:18`）+ `gpui_base::{TreeState,TreeItem,TreeEntry,TreeEvent}`；展开时在 `TreeEvent::Expanded` 里 fetch 子层塞回 TreeState |
| 目录选择 | 可换原生对话框，更省事 | `gpui::App::prompt_for_paths` + `PathPromptOptions`（`gpui-pre-0.3.6/src/app.rs:1687`、`platform.rs:2485`）；保留 `/workspace/dirs` 作为兜底（远端/无 GUI 场景） |
| 文件查看/编辑（高亮、行号、超大文件分页） | **有** | `component::input::Editor` + `EditorState::new(...).language("rust")`，需 `gpui-kit` 开 `tree-sitter*` feature（否则静默无高亮）；README 实测 10 万行可开 |
| hashline 前缀剥离 / 行号映射 / 分页拼接 | 自组装 | 对应 `fileViewerUtils.ts` 37 行、`fileLanguage.ts` 26 行，逻辑简单 |
| **双栏 diff 视图** | **无组件，必须自组装** | `gpui-component 0.6.6` 顶层 `pub mod` 清单里没有 diff/merge/compare；README 已记此缺口，并指出 `core/snapshot/mod.rs` 已有 `similar` + `build_hunks` |
| 保存 = 生成 unified patch | 自组装（或用 apply-edit 绕开） | 前端用 jsdiff；Rust 用 `similar` 生成 unified diff 约 60–120 行；或改走 `POST /workspace/apply-edit`（`ContentEdit[]`，无需 diff，但需先验证语义与冲突处理） |

**缺口**

1. diff 视图（唯一硬缺口）：建议做法 = 双栏 `Hunk` 列表 + `TextView`/等宽字体渲染，复用 `highlighter` 做语法着色；先做"纯文本 + 行号 + 增删底色"的最小版（≈120 行），再迭代到行内字符级高亮。
2. 保存前"diff 确认弹窗"（`SaveConfirmDialog`）需要上面那份 diff 能力，二者一起做。
3. 编辑器只读/编辑双模 + "保存后重新拉盘"（Web 端用 `reloadKey` 自增触发）：原生侧用 entity 状态位即可。
4. 二进制文件 / 超大文件：`read` 已返回 `binary/size/preview`，UI 只需给提示 + "分页加载更多"。

**工作量（行数量级）**

| 块 | Rust 行数 |
| --- | --- |
| 文件树（TreeState 装配 + 懒加载 + 图标/错误重试） | 350–500 |
| 文件查看/编辑器（只读+编辑、hashline 剥离、分页、二进制） | 500–700 |
| diff 视图（自组装，含保存确认弹窗） | 300–450 |
| 保存路径（unified patch 生成 或 apply-edit 接入） | 100–180 |
| 目录选择 + 会话工作区绑定 | 120–180 |
| **合计** | **≈ 1.4k–2.0k** |

---

## 5. 组件对照：我们需要的 → 库里有没有

依据：`gpui-component-0.6.6/src/lib.rs` 的 `pub mod` 清单 + 各模块源码直读；`gpui-kit 0.6.6` 只是门面（`gpui_kit::component` = `gpui_component`，同版本）。

| 需要的能力 | 用在哪 | gpui-component 0.6.6 | 结论 |
| --- | --- | --- | --- |
| 虚拟列表（可变/等高） | 消息流、会话列表、任务列表 | `list::List` + `ListDelegate`、`virtual_list::{v_virtual_list,VirtualList}` | ✅ 现成（gui-native 已在用） |
| Markdown 渲染（标题/列表/表格/引用/代码块） | 消息、技能/记忆/知识正文 | `text::TextView::markdown` + `highlighter`（tree-sitter） | ✅ 现成（README 实测表格与代码块） |
| 消息气泡 / 头像 / 消息组 / 自动滚动 | 消息流 | `bubble::{Bubble,BubbleGroup}`、`message::{Message,MessageGroup,MessageAvatar,MessageHeader}`、`message_scroller::{MessageScroller,MessageScrollerState}` | ✅ 现成（可替掉当前自绘行） |
| 多行输入 + IME + 撤销 | 输入框、prompt/命令编辑 | `input::{Textarea,TextareaState,InputEvent}` | ✅ 现成（中文 IME 已实测） |
| 富文本输入（@文件 chip、图片粘贴、行内 code 序列化） | `ChatInput`+`ComposerEditor`（1066 行） | 只有纯文本 `Textarea`；`attachment::Attachment` 可做 chip 外观 | ❌ **需自组装**（首版可降级为纯文本 + 按钮插入 `@file:"path"`） |
| 下拉选择 / 可搜索选择 | 模型、思考档位、各类枚举 | `select::{Select,SelectState}`、`combobox::Combobox`、`searchable_list` | ✅ |
| 开关 / 复选 / 单选 | 设置页大量布尔 | `switch`、`checkbox`、`radio` | ✅ |
| 数字输入（含步进） | 设置页、定时任务间隔 | `input::NumberInput` + `NumberStep` | ✅ |
| 表单容器 / 设置页骨架 | 设置页、向导 | `form::Form`、`setting::{SettingPage,SettingGroup,SettingItem,SettingField}` | ✅ 设置页语义直接命中 |
| 标签页（Tab 条 / 分段） | 设置、知识库、会话 tab 条 | `tab::{TabBar,Tab}` | ✅ |
| 对话框 / 确认框 | 删除确认、保存 diff 确认 | `dialog::Dialog`、`sheet` | ✅ |
| Toast / 通知 | 保存成功、错误提示 | `notification` | ✅ |
| 表格（列头/排序/固定列/虚拟行） | 用量明细、roles 列表、(可选)洞察 | `table::{Table,TableState,TableDelegate,Column,ColumnFixed}` | ✅ 比 Web 手写 `<table>` 更强 |
| 树（懒加载/展开/图标/上下文菜单） | workspace 文件树、知识浏览、记忆树 | `tree::tree` + `gpui_base::{TreeState,TreeItem}`（支持 `context_menu`） | ✅ **文件树有现成组件**（"无文件树、需自组装"的旧判断不成立） |
| 代码编辑器（高亮/行号/大文件） | workspace 查看/编辑 | `input::{Editor,...}` + tree-sitter feature | ✅（搜索替换/多光标/折叠/LSP 未验证，当前 CodeMirror 用法很基础） |
| **Diff 双栏视图** | workspace diff、保存前确认 | 顶层模块无 diff/merge/compare | ❌ **需自组装**（README 已记；core 有 `similar`+`build_hunks`） |
| 分栏 / 可拖拽布局 / dock 面板 | 会话页左右栏、workspace 三栏、任务 dock | `resizable::{h_resizable,v_resizable,ResizablePanelGroup}`、`dock::{DockArea,Panel,PanelHandle}` | ✅ |
| 图标栏（一级导航 rail） | sidebar | `sidebar` 模块 + `icon`（`icon_named!`） | ✅ |
| 进度（条 / 环） | 上下文环、目标进度、任务进度 | `progress`（条状）+ `progress::ProgressCircle`（**已核实存在**，progress/progress_circle.rs） | ✅ 环可直接用 |
| Badge / Tag | 风险等级、状态、角标 | `badge`、`tag` | ✅ |
| 命令面板 / 全局搜索 | 未来全局搜索 | `command::Command` | ✅ |
| 日期 / 时间选择 | 定时任务、目标 `target_date` | `time::{calendar,date_picker}` | ✅ |
| 系统文件 / 目录选择 | 工作区绑定、知识摄入 | `gpui::App::prompt_for_paths`（平台原生） | ✅（比 `/workspace/dirs` 逐级浏览更省事） |
| 剪贴板写入 | 代码块/消息复制 | `clipboard::Clipboard`（复制按钮 + tooltip）、gpui 剪贴板 API | ✅ |
| 快捷键 | 新会话 / 打开设置 | `actions!` + `KeyBinding` + `kbd` | ✅ |
| 窗口标题栏 / 状态栏 | 壳层 | `title_bar`、`status_bar`、`window_border` | ✅ |
| 图表 | 洞察（当前 UI 无图表，只有卡片/栅格） | `chart::{LineChart,BarChart,PieChart,RadarChart,SankeyChart}` | ✅（超出当前需求） |
| 托盘 / 系统通知 / 单实例 / updater | 壳层 | 无 | ❌ 自研（README 已列待接管项，`tauri/` 有参考实现，updater 编排估 200–400 行） |

---

## 6. 建议迁移顺序（"能日常用"的最短路径）

排序原则：**先让"发一条消息并拿到回复"在全新环境下自洽**（这是唯一不可替代的日常路径），再按"每天都会看的面板"补，最后做"偶尔用一次"的参考页与壳层。每一步都给出可执行的验收标准（能跑、能看、能判定通过/不通过）。

| 步 | 做什么 | 关键端点 | 预估 | 验收标准 |
| --- | --- | --- | --- | --- |
| **0（前置）** | **拆分骨架 + 数据层**：把 864 行 `main.rs` 拆成 `app/`（壳+路由）、`api/`（reqwest 封装：GET/POST/PUT/DELETE + 错误语义）、`model/`（DTO）、`ui/`（页面）；补"定时轮询"与"事件分发"两个工具 | — | 2–3 天 | ① `cargo run` 行为与现状等价（能发消息、能收流、能停止）；② 主文件 <300 行；③ 新增页面只需实现一个 `trait Page { fn render(...) }`；④ 全部 HTTP 调用都走 `api::`，无裸 `reqwest::get` |
| **1** | **一级导航壳 + 首启判定**：图标 rail（11 项）+ 页面容器 + `GET /config/status` 决定是否进配置流程 | `GET /api/v1/config/status` | 2–3 天 | ① 11 个入口都能点开（空面板也行），无 panic、无死循环；② 未配置时进入配置流程而不是空白会话页；③ `GET /config/status` 失败时**不**误判为未配置（Web 端有 5 次重试逻辑，照抄） |
| **2** | **最小可配置（首启能填完就能用）**：模型服务最小表单（provider 名 / endpoint / api_key / 模型名 + 能力 chat）+ 保存 + 测试连接；不追求 14 个 tab | `GET /config`、`PUT /config`、`POST /config/test-connection`、`GET /config/models` | 4–6 天 | ① 全新数据目录启动 → 填 endpoint/key/model → 保存成功提示；② 重启后不再要求配置；③ 发一条消息能收到流式回复；④ 测试连接能区分成功/失败并显示原因 |
| **3** | **会话能力补齐**：会话列表分组/重命名/删除、历史**分页**上滚、`/events/subscribe` 快照恢复、会话级模型与思考档位、上下文占用、回退/重做 | `GET /sessions`、`POST /sessions/{id}/title`、`DELETE /sessions/{id}`、`GET /sessions/{id}/messages?before_seq&limit`、`POST /events/subscribe`、`GET|PUT /sessions/{id}/model`、`POST /sessions/{id}/messages/{delete,redo}`、`GET /config/models` | 6–9 天 | ① 会话数 ≥50 时列表滚动流畅；② 长会话上滚能加载更早历史（不再靠 `limit=200` 全量）；③ 切换会话不丢正在流式的增量；④ 手动 kill server 后 1s 内重连并快照恢复；⑤ 模型切换对下一条消息生效 |
| **4** | **审批（含会话内横幅）**：面板 + 横幅 + 倒计时 + 命令改写 | `GET /approval/status`、`POST /approval/respond` | 3–4 天 | ① `approval_mode=interactive` 下触发危险命令，2s 内在面板与会话页同时出现；② approve 后工具继续并出结果，deny 后工具中止且有提示；③ 不响应时倒计时归零 → 自动处理可见；④ 命令改写提交后审计列表出现 `edited_command` |
| **5** | **任务面板三件**：后台任务 dock → todo/goal 面板 → 定时任务页 | `GET /tasks`、`POST /tasks/{id}/cancel`、`GET /todos`、`GET /goals`、`GET|POST /scheduled-tasks`、`DELETE /scheduled-tasks/{id}` | 5–7 天 | ① 发起 delegate 后台任务后，会话内出现行内卡片，展开能看到子代理流式输出；② 取消后状态转 `cancelled`；③ agent 推进 todo/goal 后 3s 内面板更新（切会话不串数据）；④ 定时任务可建/删，到点执行后 `last_result` 可见 |
| **6** | **workspace（树 → 查看 → 保存 → diff）** | `GET /workspace/tree`、`/workspace/read`、`/workspace/diff`、`POST /workspace/apply-patch`、`PUT /sessions/{id}/workspace` | 8–12 天 | ① 树能逐层展开到 3 层以上且失败可重试；② 打开 2 000 行以内文件秒开，超出可"加载更多"，二进制有提示；③ 编辑后保存**先看到 diff** 再落盘；④ 补丁冲突（409）有明确提示且不覆盖文件；⑤ 会话工作目录可切换并影响后续工具执行根 |
| **7** | **只读参考面板批量**：skills / tools / roles / memory / insights / knowledge 浏览 | `GET /skills*`、`GET /tools`、`GET /roles*`、`GET /memory`、`GET /stats`、`GET /scheduler/status`、`GET /knowledge/entries*` | 6–8 天（每个 150–350 行） | ① 每个页面的字段与 Web 端逐项一致（抽 3 条记录比对）；② 空态/错误态/加载态齐全；③ 记忆树按 L0/L1/L2 切换正确；④ 删除类操作（roles / knowledge entries）有确认框且成功后列表刷新 |
| **8** | **剩余设置 tab + 知识摄入 + 剪贴板 + 壳层收尾**：storage/usage/retrieval/web/logging/mcp/soul 等 tab；`knowledge/ingest`（multipart）；剪贴板确认条；托盘/系统通知/单实例/updater | `POST /knowledge/ingest`、`GET /clipboard/pending`、`POST /clipboard/respond`、`POST /config/migrate-data-dir`、MCP 5 条、`/config/soul*` | 8–12 天 | ① 14 个 tab 全部可读写，保存后热重载生效；② 3 个文件一次上传，逐文件结果可见（成功/失败原因）；③ 复制文本后 30s 内出现确认条，三选项都生效；④ **单实例锁生效**：同数据目录不再被两个进程双写；⑤ 更新流程可静默完成并能自重启 |

**排期提示**：前 3 步（0→2）约 2 周即可让原生客户端"独立可用"，是整个迁移的拐点；第 6 步（workspace）与第 8 步（设置余量）是两个大块，可以并行给不同人。若人力紧张，**第 7 步可整段后置**——它们都是只读参考页，不影响日常使用。

---

## 7. 风险清单（按影响排序）

1. **单实例 / 同数据目录双写**（工程风险，优先级高于任何 UI）：桌面版（3000）与 gui-native（3001）并存时共享同一数据目录，定时任务会被两个进程各跑一遍，SQLite 并发写。端口探测只解决"端口冲突"，不解决"数据双写"。必须在第 8 步之前解决（共享 `desktop/` 启动编排 + 单实例锁）。
2. **SSE 必须按字节分帧**：中文被 chunk 切断会产生半截 UTF-8，`String` 缓冲会出乱码（README 坑 #11，现有实现已正确用 `Vec<u8>` + `\n\n` 分帧，后续改事件处理时勿退化）。
3. **配置整包 PUT 的"字段丢失即覆盖"**：Rust 侧必须完整建模 `TianyanConfig`（或严格"读-改-写"整包回写），漏一个字段就把它写成默认值。这是设置页最大的隐性坑。
4. **`/events/subscribe` 未接入**：现在靠 `limit=200` 全量拉消息，长会话会同时承担"传输 + 布局"成本（README 记 5 万条变高列表一次性测高 2.9s CPU）。第 3 步必须切到"快照 + `before_seq` 上滚"。
5. **无 diff 组件**：workspace 的保存确认与 diff 面板都依赖它，建议先做最小版（行级、双栏、增删底色），不要一开始追求字符级高亮。
6. **富文本输入要重新设计而不是照搬**：Web 端是 Lexical（1066 行），包含 `@file:"path"` 行内 code 序列化、图片粘贴转 data URL、文件 chip 删除等语义。GPUI 没有等价组件 —— 首版建议"纯文本 + 按钮插入文件引用 + 图片另走附件通道"，把复杂度压到 200 行内。
7. **表格在流式期间必然重排一次**（Markdown 到 `|---|` 才能定列结构）—— 属解析行为，非 bug；若观感差可在流式期延迟渲染表格。
8. **文档漂移**：`docs/api-endpoints.md` 仍写 `/tasks/stream` 为"兼容入口"，实际已删除；迁移时顺手修正，避免下一个人按文档实现。
9. **后端能力缺口（如需则先补后端）**：`PATCH /scheduled-tasks/{id}`（启停/改间隔）、`GET /traces` 的前端页面；`/workspace` 没有整文件写端点（若不想生成 diff，需评估 `apply-edit` 的冲突语义）。

---

## 8. 证据索引（本次盘点直读的文件与行号）

**前端模块与规模**

- `gui-vite/src/App.tsx:20-32`（12 个懒加载面板 + 路由表 128-176）
- `gui-vite/src/components/chat/ChatPanel.tsx:5-19`（chat 模块端点全清单）
- `gui-vite/src/components/chat/ChatPanel.tsx:348-353`（发送 = `POST /chat/stream`；输出走 `/events`）
- `gui-vite/src/hooks/use-unified-events.ts:118,220`（`POST /events/subscribe`、`GET /events`）
- `gui-vite/src/components/session/SessionList.tsx:6`（`listSessions/deleteSession/updateSessionTitle/fetchTasks`）
- `gui-vite/src/components/sidebar/Sidebar.tsx:19-40`（任务语义分层注释 + `GET /approval/status` 角标）
- `gui-vite/src/components/settings/SettingsPanel.tsx:58-73`（14 个 tab 定义）、`:25`（`getConfig/saveConfig/testProviderConnection`）
- `gui-vite/src/lib/config-transform.ts:1-20,558`（配置扁平化/嵌套映射规模）
- `gui-vite/src/lib/types.ts:768-855`（`ConfigState` 全字段）、`:545-575`（审批快照）、`:576-600`（BackgroundTask）、`:612-636`（调度器状态）、`:1019-1078`（Todo/Goal）
- `gui-vite/src/components/approval/ApprovalPanel.tsx:118-160`（2s 轮询、内联命令编辑、`respondApproval`）
- `gui-vite/src/components/chat/AgentTasksPanel.tsx:24-30,45`（后台任务 3s 轮询、dock 展开）
- `gui-vite/src/components/chat/SessionTodoPanel.tsx:3,61`（todo/goal 只读 + 3s 轮询）
- `gui-vite/src/components/tasks/ScheduledTasksSection.tsx:4-10,25-45`（间隔预设 + 新建表单）
- `gui-vite/src/components/workspace/WorkspaceTree.tsx:2`（`react-arborist` 懒加载树）
- `gui-vite/src/components/workspace/FileViewer.tsx:2-29`（CodeMirror 6 + `PAGE_LIMIT=2000`）
- `gui-vite/src/components/workspace/WorkspacePanel.tsx:105-112`（jsdiff 生成 unified patch → `apply-patch`）
- `gui-vite/src/components/knowledge/KnowledgeIngestTab.tsx:65,103-123`（multipart + 拖放/多选）
- `gui-vite/src/components/clipboard/ClipboardConfirmBar.tsx:5`（`pending`/`respond`）

**后端路由（端点存在性，唯一权威）**

- `server/src/api/mod.rs:34-52`（16 领域模块 merge 清单；**无 approval 模块**）
- `server/src/api/config/routes.rs:20-53`（`/config` 全套 + soul + MCP + provider 发现）
- `server/src/api/insights/routes.rs:19-27`（`/memory`、`/stats`、`/scheduler/status`、`/usage/*`、**`/approval/status`、`/approval/respond`**）
- `server/src/api/insights/handlers.rs:238-302`（审批快照与决策解析：`approve`/`deny`/`request_more_info`、`edited_command`、404/500 语义）
- `server/src/api/todos/routes.rs:15-16`、`server/src/api/goals/routes.rs:15-16`
- `server/src/api/tasks/routes.rs:15-26`（`/tasks`、`/events`、`/events/subscribe`、`/tasks/{id}/cancel`、`/tasks/{id}/log`；注释说明 `/tasks/stream` 已删）
- `server/src/api/tasks/handlers.rs:211-262`（任务日志分页 `next_offset`）
- `server/src/scheduled_tasks/routes.rs:15-20`（只有 GET/POST/DELETE）
- `server/src/scheduled_tasks/types.rs:9-38`（`ScheduledAgentTask` 字段）
- `server/src/api/workspace/routes.rs:17-22`（tree/dirs/read/diff/apply-patch/apply-edit）
- `server/src/api/workspace/types.rs:14-188`（Tree/Read/Diff/ApplyPatch/ApplyEdit/Dirs DTO）
- `server/src/api/chat/routes.rs:12-20`（`/chat/stream`、`/chat/answer`、`/chat/streams/{id}/cancel`）
- `server/src/api/sessions/routes.rs:17-28`、`server/src/api/knowledge/routes.rs:17-25`、`server/src/api/skills/routes.rs:15-19`、`server/src/api/tools/routes.rs:9`、`server/src/api/roles.rs:298-301`、`server/src/api/clipboard/routes.rs:14-18`、`server/src/api/traces/routes.rs:13`

**原生侧现状**

- `gui-native/src/main.rs`（864 行；`:93-152` SSE 常驻与字节分帧、`:283-372` `ClientApp` 状态、`:463-525` 事件分发、`:559-700` 发送与历史拉取）
- `gui-native/Cargo.toml:24`（`gpui-kit 0.6` + tree-sitter features）
- `gui-native/README.md`（已验证能力表、11 条踩坑、内存实测、diff 组件缺口、单实例风险）
- `gpui-component-0.6.6/src/lib.rs:24-95`（`pub mod` 全清单：**含 `tree`、`table`、`setting`、`dock`、`chart`、`attachment`、`bubble`、`message`、`message_scroller`；无 diff**）
- `gpui-component-0.6.6/src/tree.rs:15-55`（`tree()` + `TreeState`/`TreeItem`/`context_menu`）
- `gpui-component-0.6.6/src/input/mod.rs:40,51`（`pub use editor::Editor`、`pub use textarea::Textarea`）
- `gpui-component-0.6.6/src/text/compat.rs:56,422`（`TextView::markdown` / `markdown()` 函数）
- `gpui-component-0.6.6/src/table/delegate.rs:16`、`src/table/state.rs:190`（`TableDelegate` / `TableState`）
- `gpui-component-0.6.6/src/setting/{page.rs:22,group.rs:16,item.rs:22,fields/mod.rs:62}`（设置页骨架）
- `gpui-component-0.6.6/src/dock/panel.rs:71`（`Panel` trait）、`src/resizable`（分栏）
- `gpui-pre-0.3.6/src/app.rs:1687`、`src/platform.rs:2485`（`prompt_for_paths` + `PathPromptOptions`）

**文档一致性**

- `docs/api-endpoints.md:49` 与 `server/src/api/tasks/routes.rs:9-14` 冲突（`/tasks/stream` 已删）——以代码为准。
- `docs/api-endpoints.md:38-39,48-55,61-68,69-83` 其余条目与代码一致（已逐条比对）。

---

## 9. 附：模块 → 端点全量对照（便于直接建 API 封装）

| 模块 | 端点（方法 + 路径） |
| --- | --- |
| chat | `POST /chat/stream`、`POST /chat/answer`、`POST /chat/streams/{session_id}/cancel`、`GET /events`、`POST /events/subscribe`、`GET /sessions/{id}/messages`、`POST /sessions/{id}/messages/delete`、`POST /sessions/{id}/messages/redo`、`POST /sessions/{id}/compress`、`GET /sessions/{id}/model`、`PUT /sessions/{id}/model`、`GET /config/models`、`GET /usage/stats`、`GET /usage/recent`、`GET /approval/status`、`POST /approval/respond`、`GET /tasks` |
| session | `GET /sessions`、`DELETE /sessions/{id}`、`POST /sessions/{id}/title`、`GET /tasks`、`GET /workspace/dirs` |
| sidebar | `GET /approval/status` |
| settings | `GET /config`、`PUT /config`、`POST /config/test-connection`、`GET /config/provider-presets`、`POST /config/providers/scan`、`POST /config/providers/add-model`、`GET /config/soul`、`PUT /config/soul`、`GET /config/soul/default`、`GET /config/mcp/servers`、`POST /config/mcp/servers`、`PUT /config/mcp/servers/{name}`、`DELETE /config/mcp/servers/{name}`、`POST /config/mcp/servers/{name}/test`、`POST /config/migrate-data-dir`、`GET /usage/stats` |
| wizard | `GET /config/status`、`GET /config/provider-presets`、`POST /config/providers/scan`、`PUT /config` |
| workspace | `GET /workspace/tree`、`GET /workspace/dirs`、`GET /workspace/read`、`GET /workspace/diff`、`POST /workspace/apply-patch`、`PUT /sessions/{id}/workspace`（+ 后端已就绪未接：`POST /workspace/apply-edit`） |
| approval | `GET /approval/status`、`POST /approval/respond` |
| 后台任务 | `GET /tasks`、`POST /tasks/{id}/cancel`、`GET /events`（+ 未接：`GET /tasks/{id}/log`） |
| todo / goal | `GET /todos?session_id=`、`GET /goals?session_id=`（+ 后端已就绪未接：`POST /todos`、`PATCH /todos/{id}`、`DELETE /todos/{id}`、`POST /goals`、`PATCH /goals/{id}`、`DELETE /goals/{id}`） |
| scheduled | `GET /scheduled-tasks`、`POST /scheduled-tasks`、`DELETE /scheduled-tasks/{id}` |
| insights | `GET /scheduler/status`、`GET /stats` |
| knowledge | `GET /knowledge/search`、`GET /knowledge/search/suggestions`、`POST /knowledge/ingest`、`GET /knowledge/entries`、`GET /knowledge/entries/read`、`POST /knowledge/entries/delete` |
| memory | `GET /memory` |
| skills | `GET /skills`、`GET /skills/{id}`、`GET /skills/stats`（+ 后端已就绪未接：`POST /skills/{id}/execute`） |
| roles | `GET /roles`、`GET /roles/{name}`、`GET /roles/stats`、`POST /roles/{name}/reset`、`DELETE /roles/{name}` |
| tools | `GET /tools` |
| clipboard | `GET /clipboard/pending`、`POST /clipboard/respond`（宿主侧：`POST /clipboard/capture`） |
| （无前端页面） | `GET /traces`、`GET /sessions/{id}`、`GET /config/{section}` |

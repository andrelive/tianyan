//! 天演原生桌面客户端（GPUI）。
//!
//! 与 `gui-vite`（Tauri + React）**并存**的第二个客户端，连同一个内嵌 axum server，
//! 走同一套 HTTP/SSE 协议。gui-vite 退役前 `tauri/` 必须保留。
//!
//! 当前进度：**11 个一级入口全部实现**（会话 / 文件 / 技能 / 子智能体 / 工具 /
//! 知识 / 记忆 / 审批 / 定时任务 / 洞察 / 设置），另有内建 GUI 自动化端口
//! （ADR-052）：`TIANYAN_GUI_AUTOMATION_PORT` 设置时监听本机回环，供 agent 读
//! 界面状态、触发语义动作，取代「DPI 抓屏 + 坐标点击」的进程外验证。
//!
//! 系统集成：通知（`notify.rs`）/ 托盘（`tray.rs`）/ 全局快捷键（`hotkey.rs`）
//! 已具备；内建截图在 `--features screenshot` 构建里可用（`ClientApp::screenshot_to`）。
//!
//! 未完成：updater 编排 / MSI 代码签名。

#![recursion_limit = "512"]
// 发布构建**不要**控制台窗口。
//
// 不加这行时 exe 的 PE subsystem 是 Console（实测 Subsystem=3），于是：
// - MSI 装的发布版双击 → 先弹一个黑窗口，再出主窗口（用户看到的就是「图标出现又消失」）；
// - debug 版从终端启动更明显：控制台窗口比主窗口早约 0.9 秒出现
//   （实测时间线：1.71s 可见控制台 → 2.56s 主窗口）。
//
// debug 特意保留 console subsystem：`println!` 是启动路径最快的观测手段，
// 而 GUI subsystem 下 stdout 无处可去。所以只在 release 切。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use futures::StreamExt as _;
use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::Root;
use gpui_kit::*;
use gpui_kit::{ListAlignment, ListState as GpuiListState};
use std::rc::Rc;

mod automation;
mod hotkey;
mod notify;
mod tray;
mod format;
mod api;
mod theme;
mod types;
mod widgets;
mod msg;
mod app;
mod chat;
mod dialogs;
mod sessions;
mod tasks;
mod pages;

// 子模块的 item `pub(crate) use` 回根：其他模块 `use crate::*;` 取全名。
// ⚠️ 外部 use（`gpui_kit::*` 等）保持**私有**——gpui 也导出 `View` /
// `AppRef` / `colors`，一旦 `pub(crate) use gpui_kit::*` 就会让 `crate::View`
// 变成双来源，子模块 `use crate::View` 直接 ambiguous（实测踩到）。
pub(crate) use format::*;
pub(crate) use api::*;
pub(crate) use theme::*;
pub(crate) use types::*;
pub(crate) use msg::*;
use msg::AppRef;
use theme::colors;
use types::View;

pub(crate) const SERVER_HOST: &str = "127.0.0.1";
pub(crate) const PREFERRED_PORT: u16 = 3000;
/// 单屏渲染的消息条数上限（无虚拟化时控制 Markdown 布局量，否则滚动卡顿）
/// 消息分页页长（`?limit=`）：与 Web 版 `ChatPanel` 同口径的 50 条一页。
///
/// 早先一次性拉 200 条再截到 60 条渲染 —— 结果是「更早的历史永远看不到，
/// 而拉回来的 200 条里 140 条白拉」。现在是「按页拉、按需前插」。
pub(crate) const MESSAGE_PAGE: usize = 50;

/// 前插累积的硬上限：超过就不再往上加载（提示已到上限）。
///
/// 不是性能闸门（列表是虚拟化的，只布局可见项），而是防「一路滚到 5000 条」
/// 那种极端内存占用。
pub(crate) const MAX_LOADED_MESSAGES: usize = 600;
/// 左栏组内初始可见会话数（对齐 Tauri 版 `SESSIONS_INITIAL_VISIBLE`）
pub(crate) const SESSIONS_INITIAL_VISIBLE: usize = 3;
/// 「查看更多」单次要展开的条数（`SESSIONS_LOAD_STEP`）
pub(crate) const SESSIONS_LOAD_STEP: usize = 10;
/// 「最近活跃」段的时间窗口（24h）与组数上限
pub(crate) const ACTIVE_GROUP_WINDOW_SECS: i64 = 24 * 3_600;
pub(crate) const ACTIVE_GROUP_LIMIT: usize = 3;
/// 顶部会话 Tab 上限（超出丢弃最早打开的；当前会话永不被丢）
pub(crate) const MAX_SESSION_TABS: usize = 8;










struct ClientApp {
    /// 左栏派生行（分组 / 会话 / 查看更多）：由 `rebuild_session_rows` 从
    /// `sessions_data` + 折叠态 + 选中态 + 任务角标重新算出
    pub(crate) session_rows: Vec<SessionListRow>,
    /// 会话原始数据（左栏分组的真相源；`sessions` 那份是按折叠/选中态**派生**的行）
    pub(crate) sessions_data: Vec<SessionRow>,
    /// 折叠的工作区分组（key = `working_directory`）
    pub(crate) collapsed_groups: std::collections::HashSet<String>,
    /// 组内额外展开条数（key = `working_directory`；点「查看更多」累加）
    pub(crate) expanded_groups: std::collections::HashMap<String, usize>,
    /// 左栏当前鼠标悬停的会话 id —— 行内的「重命名 / 删除」只在悬停时出现
    /// （对齐 Web 版 `hoveredSession === session.id` 的口径）
    pub(crate) hovered_session: Option<String>,
    /// 全量后台任务（会话角标聚合用；面板展示的是按当前会话过滤后的 `tasks`）
    pub(crate) all_tasks: Vec<TaskRow>,
    /// 任务面板里展开的任务 id（同时只展开一个，与 Tauri 版 `openId` 同口径）
    pub(crate) expanded_task: Option<String>,
    /// 子代理任务的消息缓存（`task_id` = 子会话 id → 结构化消息，供活动流渲染）
    pub(crate) task_messages: std::collections::HashMap<String, Vec<MsgRow>>,
    /// 消息区折叠状态（思考块 / 工具参数 / 工具结果）
    pub(crate) expand: ExpandRef,
    /// 子智能体（角色）列表 / 统计 / 详情
    pub(crate) roles: Vec<RoleRow>,
    pub(crate) roles_stats: RolesStats,
    /// 搜索关键词（输入框实体在 `roles_search`）
    pub(crate) roles_query: String,
    pub(crate) roles_search: Entity<InputState>,
    /// 选中的角色名与其详情
    pub(crate) roles_selected: Option<String>,
    pub(crate) role_detail: Option<RoleDetail>,
    /// 角色页状态行（加载中 / 操作结果）
    pub(crate) roles_status: String,
    /// 角色操作结果提示（回退/退役后的一行反馈）
    pub(crate) roles_action_msg: Option<String>,
    /// 工具页：工具目录（`GET /api/v1/tools`，与 LLM 收到的 tools 列表同源）
    pub(crate) tools: Vec<ToolRow>,
    /// 工具页：搜索关键词（输入框实体在 `tools_search`）
    pub(crate) tools_query: String,
    pub(crate) tools_search: Entity<InputState>,
    /// 工具页：选中的工具名（None = 未选，右侧显示引导）
    pub(crate) tools_selected: Option<String>,
    /// 技能页：学习方法论技能（`GET /api/v1/skills` 中 `category == "custom"` 的那批）
    pub(crate) skills: Vec<SkillRow>,
    /// 技能页：技能使用统计（`GET /api/v1/skills/stats`；失败不阻塞面板）
    pub(crate) skill_stats: Vec<SkillStatRow>,
    /// 技能页：累计调用次数（0 = 不显示统计条与排行）
    pub(crate) skills_total_calls: u64,
    /// 技能页：搜索关键词（输入框实体在 `skills_search`）
    pub(crate) skills_query: String,
    pub(crate) skills_search: Entity<InputState>,
    /// 技能页：选中的技能 id
    pub(crate) skills_selected: Option<String>,
    /// 技能页：选中技能的完整 markdown 内容（详情接口按需拉）
    pub(crate) skill_content: Option<String>,
    /// 技能页状态行（加载/失败提示）
    pub(crate) skills_status: String,
    /// 定时任务页：任务列表（`GET /scheduled-tasks`，ADR-024 间隔制）
    pub(crate) sched_tasks: Vec<SchedTaskRow>,
    /// 定时任务页：新建表单是否展开
    pub(crate) sched_form_open: bool,
    /// 定时任务页：表单选中的间隔预设（秒；`0` = 自定义，读 `sched_custom_secs`）
    pub(crate) sched_preset: u64,
    /// 定时任务页：表单输入框（名称 / 自定义秒 / 工作目录 / 指令）
    pub(crate) sched_name: Entity<InputState>,
    pub(crate) sched_custom_secs: Entity<InputState>,
    pub(crate) sched_workspace: Entity<InputState>,
    pub(crate) sched_prompt: Entity<TextareaState>,
    /// 定时任务页状态行（创建/删除结果）
    pub(crate) sched_status: String,
    /// 记忆页：全部记忆条目（`GET /memory`，已聚合 memory/ + user/ + agent/patterns）
    pub(crate) memories: Vec<MemoryRow>,
    /// 记忆页状态行
    pub(crate) memories_status: String,
    /// 记忆页：选中的记忆 uri
    pub(crate) memory_selected: Option<String>,
    /// 记忆页：L2 全文是否展开（默认收起——L0/L1 才是浏览主视图）
    pub(crate) memory_detail_open: bool,
    /// 洞察页：总览统计（`GET /stats`：技能/工具/文档/搜索四个追踪面）
    pub(crate) insight_stats: Option<InsightStats>,
    /// 洞察页：模型用量总计与分组（`GET /usage/stats?days=N`）
    pub(crate) usage_total: Option<UsageTotal>,
    pub(crate) usage_grouped: Vec<UsageGroup>,
    pub(crate) usage_days: i64,
    /// 洞察页：调度器状态（`GET /scheduler/status`）
    pub(crate) scheduler_running: bool,
    pub(crate) scheduler_executing: Option<String>,
    pub(crate) sched_status_tasks: Vec<SchedStatusRow>,
    /// 洞察页状态行
    pub(crate) insights_status: String,
    /// 审批页：工作流配置摘要（`GET /approval/status` 的 `config`）
    pub(crate) approval_mode: String,
    pub(crate) approval_auto: bool,
    pub(crate) approval_timeout: u64,
    pub(crate) approval_max_pending: u64,
    pub(crate) approval_prompt_cmds: Vec<String>,
    pub(crate) approval_rules: Vec<ApprovalRuleRow>,
    /// 审批页：待用户确认的操作指纹（「询问用户」降级链路，agent loop 追问中）
    pub(crate) pending_confirmations: Vec<String>,
    pub(crate) confirmed_action_count: usize,
    /// 审批页：审计记录（后端倒序 50 条，界面显示前 20）
    pub(crate) approval_history: Vec<ApprovalHistRow>,
    /// 审批页状态行
    pub(crate) approvals_page_status: String,
    /// 知识页：当前 tab（0 = 浏览 / 1 = 导入 / 2 = 搜索）
    pub(crate) kb_tab: u8,
    /// 知识页·浏览：当前目录（相对知识根，空 = 根）
    pub(crate) kb_path: String,
    pub(crate) kb_entries: Vec<KbEntryRow>,
    /// 知识页·浏览：选中的条目 uri 与读到的内容（`(level, content)`）
    pub(crate) kb_selected: Option<String>,
    pub(crate) kb_content: Option<(String, String)>,
    /// 知识页状态行（浏览/导入/搜索共用一行）
    pub(crate) kb_status: String,
    /// 知识页·导入：路径与分类输入框、每文件结果
    pub(crate) kb_ingest_path: Entity<InputState>,
    pub(crate) kb_ingest_category: Entity<InputState>,
    pub(crate) kb_ingest_results: Vec<(String, String)>,
    /// 知识页·导入：**内嵌文件浏览器**的当前目录、条目与状态
    ///
    /// 不用系统文件对话框：那是 OS 级模态框，自动化端口既点不到也看不到
    /// （`/state` 里没有它的位置）。内嵌浏览器由 GPUI 自绘，可点、可断言。
    pub(crate) kb_browse_dir: String,
    pub(crate) kb_browse_entries: Vec<KbBrowseEntry>,
    pub(crate) kb_browse_status: String,
    /// 是否显示点开头的隐藏条目（默认关：项目根目录十几个 `.git`/`.cargo`/`.vscode`
    /// 会把第一屏占满，真正想选的文件要滚几屏 —— 选文件的场景下这是纯噪音）
    pub(crate) kb_show_hidden: bool,
    /// 文件选择弹窗里的过滤词（名字包含即匹配；空 = 不过滤）
    pub(crate) kb_pick_query: String,
    pub(crate) kb_pick_search: Entity<InputState>,
    /// 已勾选待导入的文件（完整路径，保持勾选顺序）
    pub(crate) kb_picked: Vec<String>,
    /// 知识页·搜索：查询输入框与结果
    pub(crate) kb_search: Entity<InputState>,
    pub(crate) kb_results: Vec<KbSearchRow>,
    /// 目录选择器：当前浏览目录（绝对路径；浏览根时为「浏览根」）
    pub(crate) picker_current: String,
    /// 目录选择器：上级目录（None = 已在浏览根）
    pub(crate) picker_parent: Option<String>,
    /// 目录选择器：子目录 `(名称, 完整路径)`
    pub(crate) picker_entries: Vec<(String, String)>,
    /// 目录选择器状态行
    pub(crate) picker_status: String,
    /// 文件页：工作区根（绝对路径，来自 tree 响应）
    pub(crate) ws_root: String,
    /// 文件页：当前目录（相对根；`""` = 根）
    pub(crate) ws_path: String,
    /// 文件页：当前目录的条目
    pub(crate) ws_entries: Vec<WsEntry>,
    /// 文件页：打开的文件（`(相对路径, 内容)`）
    pub(crate) ws_file: Option<(String, String)>,
    /// 文件页状态行（加载中 / 截断提示 / 错误）
    pub(crate) ws_status: String,
    /// 文件页：diff 面板是否展开
    pub(crate) ws_diff_open: bool,
    /// 文件页：改动文件列表
    pub(crate) ws_diff_files: Vec<WsDiffFile>,
    /// 文件页：当前查看的 diff（相对路径 + unified 文本）
    pub(crate) ws_diff: Option<(String, String)>,
    /// 文件页：diff 面板状态行
    pub(crate) ws_diff_status: String,
    /// 反向弱引用：`gpui_list` 的渲染闭包只能拿 `&mut App`，点击折叠块时要
    /// 回写 view 状态 → 与旧 `SessionsDelegate` 同一手法
    pub(crate) app_ref: AppRef,
    /// 当前打开的模态对话框（删除确认 / 重命名）
    pub(crate) dialog: Option<Dialog>,
    /// 放大查看的用户消息图片（点击图片打开全屏遮罩；None = 未打开）
    pub(crate) zoomed_image: Option<std::sync::Arc<gpui_kit::Image>>,
    /// 待发送的图片（data URL 列表）：控件行「图片」按钮选入，发送后清空
    pub(crate) composer_images: Vec<String>,
    /// 按会话保存的输入草稿：`会话 id → (正文, 待发图片)`；新会话用空串当键。
    ///
    /// 为什么要它：Web 版 `ChatInput` 按 `sessionId ?? PENDING` 存草稿，切走再切回
    /// 写了一半的内容还在；原生早先切会话直接丢（输入框与图片都清空）——写长 prompt
    /// 时最容易踩，且丢了就找不回来。
    pub(crate) drafts: std::collections::HashMap<String, (String, Vec<String>)>,
    /// 哪些会话**当前有轮在跑**（`turn_state` 事件驱动，含非当前会话）。
    ///
    /// 为什么需要它：`streaming` 只是「**当前**会话在跑」的渲染态，切走时会被
    /// 重置。切回时若只看它，按钮就从「停止」退回「发送」——即使服务端这一轮
    /// 还在生成（用户实测：会话进行中切走再切回，按钮不再是停止）。`turn_state`
    /// 事件本身带 `session_id`，因此在**会话过滤之前**记一份全量集合，切回时
    /// 按 id 恢复即可，不必新增查询端点。
    pub(crate) running_sessions: std::collections::HashSet<String>,
    /// 上滚游标（`GET /sessions/{id}/messages` 的 `next_before_seq`）：
    /// 指向当前窗口**最早一条**的 seq；`None` = 已到链首，没有更早的了。
    pub(crate) oldest_seq: Option<i64>,
    /// 服务端 `has_more`：还有更早的历史可加载（顶部入口据此显示）
    pub(crate) has_more_history: bool,
    /// 正在拉上一页（防连点重复请求）
    pub(crate) loading_older: bool,
    /// 首次启动引导：`GET /config/status` 报 `configured=false` 时置真，
    /// render 走全屏引导页（`pages/setup.rs`）而不是主界面。
    pub(crate) needs_setup: bool,
    /// 用户点过「仍然进入应用」—— 不再拦（配置误判时的逃生舱）
    pub(crate) setup_dismissed: bool,
    /// 右上角瞬时提示栈（错误粘住 10s、普通 4s；同文本去重、最多 5 条）
    pub(crate) notices: Vec<Notice>,
    /// 打开的会话 Tab（浏览器式；按打开顺序，最多 [`MAX_SESSION_TABS`] 个）
    pub(crate) open_tabs: Vec<String>,
    /// 新会话待绑定的工作目录（「＋新目录」设置）：随首条消息以
    /// `working_directory` 下发，服务端建会话时固化（目录不存在则不绑定）
    pub(crate) new_session_workspace: Option<String>,
    /// 重命名输入框：打开对话框时**新建**一个实体（`InputState` 没有公开的
    /// `set_value`，复用旧实体无法回填原标题，只能每次新建）
    pub(crate) dialog_input: Option<Entity<InputState>>,
    /// 消息用 Rc 包一层：闭包驱动的虚拟列表需要把数据带进 'static 闭包，
    /// Rc::clone 是 O(1)，比每帧 clone Vec<MsgRow> 便宜得多
    pub(crate) messages: Rc<Vec<MsgRow>>,
    /// gpui 原生变高虚拟列表的状态（只有可见项会被 render）
    pub(crate) msg_list_state: GpuiListState,
    /// 会话消息总数（messages 可能只保留最近 N 条用于渲染）
    pub(crate) message_total: usize,
    pub(crate) status: String,
    pub(crate) port: u16,
    pub(crate) session_count: usize,
    pub(crate) current_title: String,
    /// 当前选中会话 id —— 事件路由键：只渲染本会话的 `chat_stream` 增量
    pub(crate) current_session: Option<String>,
    /// 输入框（gpui-component 的 `Textarea`：IME / 选区 / 撤销都由它管，不自造）
    pub(crate) input: Entity<TextareaState>,
    /// 是否有轮在跑（`turn_state` 驱动，同时闸住重复发送）
    pub(crate) streaming: bool,
    /// 当前视图
    pub(crate) view: View,
    /// 设置页：模型列表（`GET /api/v1/config/models`）
    pub(crate) models: Vec<ModelRow>,
    /// 设置页状态行（切换结果 / 错误）
    pub(crate) settings_status: String,
    /// 设置页状态行是否为错误（决定用 `--color-error` 还是次级文字色）
    pub(crate) settings_error: bool,
    /// 设置页当前二级栏目（与 vite `SettingsPanel` 的 `activeTab` 同口径）。
    pub(crate) settings_tab: SettingsTab,
    /// `GET /api/v1/config` 返回的 `config` 段原文。
    ///
    /// 设置页大多数栏目（存储/Agent/安全/日志/记忆/检索/MCP/Web）目前**只读展示**
    /// 这几段配置——写路径要逐个字段确认语义（`PUT /config` 是整份替换），一次全做
    /// 会把回归面摊太开。留原始 JSON 是为了「配置里有的字段都能看到」，不必为每个
    /// 字段在 GUI 侧建模。
    pub(crate) raw_config: Option<serde_json::Value>,
    /// 会话级模型选择（`(provider, model)`）：None = 回落到 `preferences.chat`
    pub(crate) session_model: Option<(String, String)>,
    /// 模型下拉是否展开
    pub(crate) model_menu_open: bool,
    /// 会话级思考强度：`None` = default（不附加思考参数，是否思考由模型决定）。
    /// 档位值来自当前模型声明的 `reasoning_efforts`，**原样展示不翻译**
    /// （厂商可能给 low / high / max 等任意值）
    pub(crate) thinking: Option<String>,
    /// 思考强度下拉是否展开
    pub(crate) thinking_menu_open: bool,
    /// 最近一轮请求的**实时**用量（流式 `usage` 上报写入，切会话即清）。
    /// 上下文占用优先取它，其次回落到消息链自带的 `usage`（见 `ctx_usage`）
    pub(crate) ctx_live: Option<MsgUsage>,
    /// 流式上报的上下文窗口：模型没声明 `context_length` 时的兜底分母
    pub(crate) ctx_window_live: u64,
    /// 上下文详情面板是否展开（点圆环打开：占用 / 缓存命中 / 输出 + 压缩入口）
    pub(crate) ctx_panel_open: bool,
    /// 压缩进行中（按钮禁点 + 文案「压缩中…」）
    pub(crate) ctx_compressing: bool,
    /// 压缩结果提示：`(是否成功, 文案)`
    pub(crate) ctx_compress: Option<(bool, String)>,
    /// 会话 token 汇总（`GET /usage/stats?days=0&session_id=`）：
    /// `(未命中输入, 缓存命中输入, 输出)`；全零时 `None`（不显示统计行）
    pub(crate) session_usage: Option<(u64, u64, u64)>,
    /// 最近一次请求性能（`GET /usage/recent?limit=1`）：
    /// `(ttft_ms, duration_ms, completion_tokens)`；`duration_ms < 0` 时 `None`
    pub(crate) request_perf: Option<(Option<i64>, i64, u64)>,
    /// 本会话的后台任务（`GET /api/v1/tasks` 按 `parent_session_id` 过滤）
    pub(crate) tasks: Vec<TaskRow>,
    /// 任务面板是否折叠（折叠成 36px 竖条，对齐 Tauri 版 dock 的收起态）
    pub(crate) tasks_collapsed: bool,
    /// 会话待办（`(status, title)`，来自 `GET /todos?session_id=`）
    pub(crate) todos: Vec<(String, String)>,
    /// 会话目标（`GET /goals?session_id=`，元素是 `{goal, progress, todo_total, todo_done}`）
    pub(crate) goals: Vec<GoalRow>,
    /// 待办/目标窄条是否折叠
    pub(crate) todo_bar_collapsed: bool,
    /// 本会话的待审批操作（`GET /api/v1/approval/status` 按 `session_id` 过滤）
    pub(crate) pending_approvals: Vec<ApprovalRow>,
    /// 待回答的追问（`ask_user` 同步工具挂起时）：`(问题, 选项)`
    pub(crate) clarification: Option<Vec<(String, Vec<String>)>>,
    /// 已选答案（与 `clarification` 等长；空串 = 未答）
    pub(crate) clarify_answers: Vec<String>,
    /// 设置页：provider 列表（含 endpoint / api_key，供编辑回填）
    pub(crate) providers: Vec<ProviderRow>,
    /// 设置页：正在编辑的 provider（None = 未打开编辑器）
    pub(crate) editing_provider: Option<String>,
    /// 编辑框：endpoint
    pub(crate) edit_endpoint: Entity<TextareaState>,
    /// 编辑框：api_key
    pub(crate) edit_api_key: Entity<TextareaState>,
    /// 设置页字段引擎：各文本类字段的编辑态（key = 字段表的点路径）。
    ///
    /// GPUI 的编辑态是**实体**，而 render 是 `&self`（不能就地 `cx.new`），
    /// 所以按静态字段表在启动时一次建好（见 `settings::fields::FIELDS`）。
    pub(crate) settings_editors:
        std::collections::HashMap<&'static str, Entity<TextareaState>>,
    /// 设置页字段引擎：Bool/Enum 的改动（文本/数字直接读编辑器，不入表）。
    pub(crate) settings_dirty: std::collections::HashMap<&'static str, serde_json::Value>,
    /// 设置页字段引擎：保存 / 校验结果（顶部状态行优先显示它）。
    pub(crate) settings_msg: String,
    /// 设置页字段引擎：上面那条是否为错误。
    pub(crate) settings_msg_err: bool,
    /// 设置页字段引擎：配置是否已经回填进编辑器（编辑器初始为空，未回填时保存会
    /// 把所有文本字段当成「被清空」——所以回填必须可靠触发，见 `backfill_settings_fields`）
    pub(crate) settings_backfilled: bool,
    // ── 设置页「人设」 ──
    /// 人设正文编辑器（`PUT /config/soul` 的内容源）
    pub(crate) soul_editor: Entity<TextareaState>,
    /// 内置默认人设（`GET /config/soul/default`：给「填入默认」与差异提示用）
    pub(crate) soul_default: String,
    /// 是否已加载过人设（成功一次就不再重复拉——重复拉会覆盖用户正在编辑的正文）
    pub(crate) soul_loaded: bool,
    /// 人设栏目的状态行
    pub(crate) soul_msg: String,
    /// 上面那条是否为错误
    pub(crate) soul_msg_err: bool,
    // ── 设置页「MCP」 ──
    /// MCP 服务器列表
    pub(crate) mcp_servers: Vec<McpServerRow>,
    /// 每台服务器的最近一次测试结果（name → 文案；结果在窗口内保留，不落库）
    pub(crate) mcp_test: std::collections::HashMap<String, String>,
    /// MCP 栏目状态行
    pub(crate) mcp_msg: String,
    /// 上面那条是否为错误
    pub(crate) mcp_msg_err: bool,
    /// 新增表单是否展开
    pub(crate) mcp_form_open: bool,
    /// 新增表单：名称
    pub(crate) mcp_name: Entity<InputState>,
    /// 新增表单：命令
    pub(crate) mcp_command: Entity<InputState>,
    /// 新增表单：参数（空格分隔）
    pub(crate) mcp_args: Entity<InputState>,
    /// 新增表单：说明
    pub(crate) mcp_desc: Entity<InputState>,
    // ── 设置页「数据存储」搬迁 ──
    /// 搬迁目标目录输入
    pub(crate) storage_move_target: Entity<InputState>,
    /// 搬迁状态行
    pub(crate) storage_msg: String,
    /// 上面那条是否为错误
    pub(crate) storage_msg_err: bool,
    // ── 设置页「用量统计」 ──
    /// 分组维度：`model` / `provider`
    pub(crate) usage_group_by: &'static str,
    /// 用量栏目的加载/错误状态
    pub(crate) usage_msg: String,
    // ── 设置页「连接 / 关于」 ──
    /// `/health` 的服务真实信息（版本 / 数据目录 / pid）；`None` = 未探活或探活失败
    pub(crate) health_info: Option<HealthInfo>,
    /// 探活状态行（失败文案）
    pub(crate) health_msg: String,
    // ── 设置页「模型服务」 ──
    /// 提供商/模型增删改的状态行（本栏目独立于整页 `settings_status`）
    pub(crate) models_msg: String,
    /// 上面那条是否为错误
    pub(crate) models_msg_err: bool,
    /// 新增模型表单是否展开
    pub(crate) model_form_open: bool,
    /// 新增模型表单：目标提供商（点选）
    pub(crate) model_form_provider: Option<String>,
    /// 新增模型表单：模型名
    pub(crate) model_form_name: Entity<InputState>,
    /// 新增模型表单：能力标签（kebab-case，可多选）
    pub(crate) model_form_caps: Vec<&'static str>,
    /// 新增提供商表单是否展开
    pub(crate) prov_form_open: bool,
    /// 新增提供商表单：名称
    pub(crate) prov_form_name: Entity<InputState>,
    /// 新增提供商表单：端点（可留空走预置）
    pub(crate) prov_form_endpoint: Entity<InputState>,
    /// 新增提供商表单：密钥
    pub(crate) prov_form_key: Entity<InputState>,
    /// 新增提供商表单：首个模型名（后端校验要求提供商至少有一个模型，
    /// 所以「新增提供商」必须一次带一个模型，否则 PUT 会被 422 拒绝）
    pub(crate) prov_form_model: Entity<InputState>,
    /// 新增提供商表单：首个模型的能力标签
    pub(crate) prov_form_caps: Vec<&'static str>,
    /// 待确认删除的项：`(kind, provider, model)`；`kind` = `provider` / `model`
    pub(crate) delete_confirm: Option<(String, String, String)>,
    /// 本窗口句柄。
    ///
    /// 自动化端口在 `Context` 里执行动作时**没有 `&mut Window`**，而往输入框插文本
    /// （`TextareaState::insert`）需要它——用句柄现取，让两条路径（真实点击 /
    /// 自动化动作）最后都落到同一个插入 API，不出现第二套插入逻辑。
    pub(crate) window_handle: gpui_kit::AnyWindowHandle,
    /// 最近一次内建截图的结果：`(路径, 宽, 高)`。
    ///
    /// 只在 `--features screenshot` 构建里有意义（该构建下动作会写它）；
    /// 生产构建恒为 `None`，字段本身零开销。
    pub(crate) last_screenshot: Option<(String, u32, u32)>,
}



impl ClientApp {
    /// 右上角瞬时提示栈（错误不再被状态行覆盖 —— 早先只有 `status` 单行，
    /// 一次失败提示常在下一次 3s 轮询时被抹掉，用户来不及看）。
    ///
    /// 挂在根 `relative` 容器上、模态对话框**之前**：提示始终可见，但对话框更上层。
    pub(crate) fn render_notices(&self) -> AnyElement {
        if self.notices.is_empty() {
            return div().into_any_element();
        }
        div()
            .absolute()
            .top(px(12.))
            .right(px(12.))
            .flex()
            .flex_col()
            .gap_1()
            .children(self.notices.iter().map(|n| {
                let (border, fg) = if n.is_err {
                    (colors::ERROR(), colors::ERROR())
                } else {
                    (colors::BORDER(), colors::TEXT_PRIMARY())
                };
                div()
                    .max_w(px(420.))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(border))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(rgb(fg))
                    .child(n.text.clone())
                    .into_any_element()
            }))
            .into_any_element()
    }
}

impl Render for ClientApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 说明：设置页/人设的编辑器回填**不在这里做**——窗口隐藏时不会触发渲染，
        // 而自动化验收正是隐藏窗口跑的。回填统一走 `window_handle.update` 现取窗口
        // （见 `backfill_settings_fields` / `set_soul_text`）。
        let _ = window;
        // 整体骨架与 `gui-vite/src/components/layout/AppLayout.tsx` 一致：
        //   一级 rail(64px) + main(flex-1, bg-primary)
        let mut root = div()
            .size_full()
            .flex()
            .flex_row()
            // 相对定位：模态对话框覆盖层用 `absolute` 相对这里铺满整窗
            .relative()
            .bg(rgb(colors::BG_PRIMARY()))
            .text_color(rgb(colors::TEXT_PRIMARY()))
            .child(self.render_rail(cx))
            .child(div().flex_1().min_w_0().h_full().child(match self.view {
                View::Chat => self.render_chat(cx).into_any_element(),
                View::Settings => self.render_settings(cx).into_any_element(),
                View::Roles => self.render_roles(cx).into_any_element(),
                View::Workspace => self.render_workspace(cx).into_any_element(),
                View::Tools => self.render_tools(cx).into_any_element(),
                View::Skills => self.render_skills(cx).into_any_element(),
                View::Scheduled => self.render_scheduled(cx).into_any_element(),
                View::Memory => self.render_memory(cx).into_any_element(),
                View::Insights => self.render_insights(cx).into_any_element(),
                View::Approval => self.render_approval(cx).into_any_element(),
                View::Knowledge => self.render_knowledge(cx).into_any_element(),
            }))
            .child(if self.dialog.is_some() {
                self.render_dialog(cx)
            } else {
                div().into_any_element()
            });
        // 瞬时提示栈：最后挂（最上层）。它在右上角、对话框居中，两者不重叠
        root = root.child(self.render_notices());
        // 图片放大遮罩（对齐 vite `ZoomableImage`）：铺满整窗、点击任意处关闭
        if let Some(img) = self.zoomed_image.clone() {
            root = root.child(
                div()
                    .id("image-zoom")
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .right(px(0.))
                    .bottom(px(0.))
                    // 半透明黑（0xRRGGBBAA）
                    .bg(rgba(0x000000cc))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.zoomed_image = None;
                        cx.notify();
                    }))
                    .child(
                        gpui_kit::img(gpui_kit::ImageSource::Image(img))
                            // 相对父容器 90%（`relative` 走父宽，不是绝对像素）
                            .max_w(relative(0.9))
                            .max_h(relative(0.9))
                            .rounded_lg(),
                    ),
            );
        }
        // 首次启动引导：未配置时用**全屏覆盖层**盖住主界面（而不是早返回另一个
        // 类型——那会让 `Render::render` 的返回类型具体化，撞 `refining_impl_trait`
        // 警告。覆盖层同时保证「配置好后立刻回到主界面」，不需要重建视图）
        if self.needs_setup {
            root = root.child(self.render_setup_screen(cx));
        }
        root
    }
}

/// 子代理活动流的一行（把消息摊平后的投影）。
enum ActivityRow {
    Thinking { key: String, text: String },
    Text { key: String, text: String },
    Tool { key: String, call: ToolCall },
}


/// 隐藏**本进程独占的**控制台窗口。
///
/// 为什么需要：debug 构建是 console subsystem（PE `Subsystem=3`，为了让 `println!`
/// 进终端），从资源管理器/`Start-Process` 启动时会新开一个控制台窗口——它比主窗口
/// 早约 0.9 秒出现（实测时间线：1.71s 可见控制台 → 2.56s 主窗口），在任务栏上看着
/// 就像「图标反复出现又消失」。
///
/// 为什么不能无脑 `ShowWindow(SW_HIDE)`：从已有终端（pwsh/cmd）启动时，进程**继承**
/// 那个控制台，`GetConsoleWindow()` 返回的是**用户的终端窗口**——隐藏它会把用户自己
/// 的终端藏掉。所以先用 `GetConsoleProcessList` 判断：只有本进程附着（count ≤ 1）
/// 才是「自己独占的控制台」，这时隐藏是安全的；继承了别人的终端（count ≥ 2）就什么
/// 都不做，日志照常进那个终端。
///
/// release 构建走 `windows_subsystem = "windows"`，压根没有控制台，本函数是空操作。
#[cfg(windows)]
fn hide_owned_console() {
    use windows::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

    // SAFETY: 两个调用都只读进程/控制台信息；`pids` 是栈上缓冲区，长度由 API 接收。
    // `ShowWindow` 只改窗口可见状态，不涉及内存所有权。句柄无效时跳过。
    unsafe {
        let mut pids = [0u32; 4];
        let attached = GetConsoleProcessList(&mut pids);
        if attached > 1 {
            return;
        }
        let hwnd = GetConsoleWindow();
        if !hwnd.is_invalid() {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[cfg(not(windows))]
fn hide_owned_console() {}

/// 是否以「隐藏窗口」启动（`TIANYAN_GUI_START_HIDDEN=1`）。
///
/// 只给回归/自动化用：反复启动客户端验证时，窗口一出现就会在任务栏闪动并抢焦点，
/// 而自动化要读的东西（`/state`、`/act`、内建截图）全都不需要窗口可见。
fn start_hidden() -> bool {
    matches!(
        std::env::var("TIANYAN_GUI_START_HIDDEN")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

fn main() {
    hide_owned_console();
    let config = tianyan_service_config::load_and_install();
    if let Err(e) = tianyan::common::logging::init_logging(&config.logging) {
        eprintln!("日志初始化失败：{e}");
    }
    tianyan::common::panic_hook::install(tianyan::common::panic_hook::default_log_dir());

    // 数据目录搬迁的**执行侧**：`POST /config/migrate-data-dir` 只写请求文件并触发服务
    // 优雅关停，真正的「复制 → 校验 → 改配置 → 删源」必须在服务停掉之后做（SQLite /
    // LanceDB 的文件锁那时才释放）。桌面版（Tauri）由监督循环在进程内完成；原生端
    // 上一次进程已退出，所以在这里——**起 server 之前**——补上这一步。失败自动回滚
    // （数据留在旧目录、配置未动、请求已清除），此时继续用旧配置启动，应用照常可用。
    let config = match tianyan_service_config::run_pending_migration(&config.storage.data_dir) {
        Ok(outcome) if outcome.migrated => {
            for leftover in &outcome.leftovers {
                eprintln!("[搬迁] 残留（被占用未能删除）：{}", leftover.display());
            }
            let fresh = tianyan_service_config::load_and_install();
            println!(
                "[搬迁] 已完成，数据目录：{}",
                fresh.storage.data_dir.display()
            );
            fresh
        }
        Ok(_) => config,
        Err(e) => {
            eprintln!("[搬迁] 失败（已回滚）：{e}");
            config
        }
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    // 单 server 复用（与桌面版共用同一份启动编排）：本机若已有**服务同一数据目录**的
    // 天演实例就直接连它，本进程不再起第二个 —— 否则两个 server 会各跑一遍定时任务
    // 并并发写同一个 SQLite（实测撞上过 3000 + 3001 同时跑）。
    let endpoint = match runtime().block_on(tianyan_desktop::ensure_server(
        config.clone(),
        PREFERRED_PORT,
        shutdown_rx,
    )) {
        Ok(ep) => ep,
        Err(e) => {
            eprintln!("[启动] 内嵌 server 不可用：{e}");
            std::process::exit(1);
        }
    };
    let port = endpoint.port;
    if endpoint.reused {
        println!("[启动] 复用本机已有 server（端口 {port}），本进程未启动 server");
    } else {
        println!("[启动] 内嵌 server 已就绪（端口 {port}）");
    }
    // 系统通知通道：注册之后 core 的所有通知场景自动生效（后台任务完成/失败、
    // 主动提醒、审批挂起）—— 它们在构建时注入的是动态包装
    // `global_notification_sink()`，读的就是这个全局槽，注册即刻生效。
    //
    // 复用已有 server 时（`endpoint.reused`）core 组件在**另一个进程**里，本进程
    // 注册不到它们（通知归那个进程的宿主负责，tauri 版自带 sink）。这里不特判：
    // 注册是幂等的，代价为零，而加分支只会掩盖真实的装配关系。
    notify::install();


    // 图标资源：`gpui-kit-assets` 内嵌完整 Lucide 目录（`AllAssets`），
    // 必须在 `application()` 上注册 —— 否则 `svg().path("icons/*.svg")`
    // 加载不到，图标静默空白（不报错）。
    let app = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets::new(""));
    app.run(move |cx| {
        gpui_kit::init(cx);
        // 应用上次选的外观。组件库内部控件（Input / Textarea / 按钮）走它自己的
        // `ThemeMode`，不一起切就会出现「天演是羊皮纸、输入框还是深灰」。
        let theme = load_theme_pref();
        set_theme(theme);
        Theme::change(
            match theme {
                ThemeKind::Parchment => ThemeMode::Light,
                _ => ThemeMode::Dark,
            },
            None,
            cx,
        );
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        // 自动化端口（ADR-052）：服务在 tokio 线程、动作用 UI 线程执行，所以先把
        // `AsyncApp` 取出来带进窗口构建闭包——窗口建好才有状态可读。
        let async_cx = cx.to_async();
        // 系统托盘（D2）：图标 + 菜单住在自己的线程（win32 消息循环），动作经
        // channel 回主线程执行。窗口还没建，`on_window_should_close` 在
        // `ClientApp::new` 里注册。
        let (tray_tx, tray_rx) = tokio::sync::mpsc::unbounded_channel::<tray::TrayCommand>();
        tray::spawn(tray_tx);
        // 全局快捷键（D3）：Ctrl+Alt+T 切换主窗口显隐。与托盘共用 `tray::*`
        // 的窗口操作函数——两条入口同一份实现，不会出现"托盘能显示、热键不显示"。
        hotkey::spawn();
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("天演".into()),
                        ..Default::default()
                    }),
                    // 回归/自动化用：`TIANYAN_GUI_START_HIDDEN=1` 时窗口建好但不显示。
                    // 目的 = 测试对用户桌面零打扰（不闪任务栏图标、不抢焦点），而
                    // `/state`、`/act` 与内建截图（`render_to_image` 不依赖可见性）照常。
                    show: !start_hidden(),
                    ..Default::default()
                },
                move |window, cx| {
                    let view = cx.new(|cx| ClientApp::new(window, cx, port, tray_rx));
                    // 未设 `TIANYAN_GUI_AUTOMATION_PORT` 时是空操作
                    automation::maybe_start(&async_cx, view.downgrade());
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });

    let _ = shutdown_tx.send(true);
    println!("[退出] 已发送关停信号");
}

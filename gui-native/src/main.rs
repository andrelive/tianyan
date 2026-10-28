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

use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::text::TextView;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::Root;
use gpui_kit::*;
use gpui_kit::{list as gpui_list, ListAlignment, ListState as GpuiListState};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

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

// 子模块的 item `pub(crate) use` 回根：其他模块 `use crate::*;` 取全名。
// ⚠️ 外部 use（`gpui_kit::*` 等）保持**私有**——gpui 也导出 `View` /
// `AppRef` / `colors`，一旦 `pub(crate) use gpui_kit::*` 就会让 `crate::View`
// 变成双来源，子模块 `use crate::View` 直接 ambiguous（实测踩到）。
pub(crate) use format::*;
pub(crate) use api::*;
pub(crate) use theme::*;
pub(crate) use types::*;
pub(crate) use widgets::*;
pub(crate) use msg::*;
use msg::AppRef;
use theme::colors;
use types::View;

pub(crate) const SERVER_HOST: &str = "127.0.0.1";
pub(crate) const PREFERRED_PORT: u16 = 3000;
/// 单屏渲染的消息条数上限（无虚拟化时控制 Markdown 布局量，否则滚动卡顿）
pub(crate) const RENDER_LIMIT: usize = 60;
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
    session_rows: Vec<SessionListRow>,
    /// 会话原始数据（左栏分组的真相源；`sessions` 那份是按折叠/选中态**派生**的行）
    sessions_data: Vec<SessionRow>,
    /// 折叠的工作区分组（key = `working_directory`）
    collapsed_groups: std::collections::HashSet<String>,
    /// 组内额外展开条数（key = `working_directory`；点「查看更多」累加）
    expanded_groups: std::collections::HashMap<String, usize>,
    /// 左栏当前鼠标悬停的会话 id —— 行内的「重命名 / 删除」只在悬停时出现
    /// （对齐 Web 版 `hoveredSession === session.id` 的口径）
    hovered_session: Option<String>,
    /// 全量后台任务（会话角标聚合用；面板展示的是按当前会话过滤后的 `tasks`）
    all_tasks: Vec<TaskRow>,
    /// 任务面板里展开的任务 id（同时只展开一个，与 Tauri 版 `openId` 同口径）
    expanded_task: Option<String>,
    /// 子代理任务的消息缓存（`task_id` = 子会话 id → 结构化消息，供活动流渲染）
    task_messages: std::collections::HashMap<String, Vec<MsgRow>>,
    /// 消息区折叠状态（思考块 / 工具参数 / 工具结果）
    expand: ExpandRef,
    /// 子智能体（角色）列表 / 统计 / 详情
    roles: Vec<RoleRow>,
    roles_stats: RolesStats,
    /// 搜索关键词（输入框实体在 `roles_search`）
    roles_query: String,
    roles_search: Entity<InputState>,
    /// 选中的角色名与其详情
    roles_selected: Option<String>,
    role_detail: Option<RoleDetail>,
    /// 角色页状态行（加载中 / 操作结果）
    roles_status: String,
    /// 角色操作结果提示（回退/退役后的一行反馈）
    roles_action_msg: Option<String>,
    /// 工具页：工具目录（`GET /api/v1/tools`，与 LLM 收到的 tools 列表同源）
    tools: Vec<ToolRow>,
    /// 工具页：搜索关键词（输入框实体在 `tools_search`）
    tools_query: String,
    tools_search: Entity<InputState>,
    /// 工具页：选中的工具名（None = 未选，右侧显示引导）
    tools_selected: Option<String>,
    /// 技能页：学习方法论技能（`GET /api/v1/skills` 中 `category == "custom"` 的那批）
    skills: Vec<SkillRow>,
    /// 技能页：技能使用统计（`GET /api/v1/skills/stats`；失败不阻塞面板）
    skill_stats: Vec<SkillStatRow>,
    /// 技能页：累计调用次数（0 = 不显示统计条与排行）
    skills_total_calls: u64,
    /// 技能页：搜索关键词（输入框实体在 `skills_search`）
    skills_query: String,
    skills_search: Entity<InputState>,
    /// 技能页：选中的技能 id
    skills_selected: Option<String>,
    /// 技能页：选中技能的完整 markdown 内容（详情接口按需拉）
    skill_content: Option<String>,
    /// 技能页状态行（加载/失败提示）
    skills_status: String,
    /// 定时任务页：任务列表（`GET /scheduled-tasks`，ADR-024 间隔制）
    sched_tasks: Vec<SchedTaskRow>,
    /// 定时任务页：新建表单是否展开
    sched_form_open: bool,
    /// 定时任务页：表单选中的间隔预设（秒；`0` = 自定义，读 `sched_custom_secs`）
    sched_preset: u64,
    /// 定时任务页：表单输入框（名称 / 自定义秒 / 工作目录 / 指令）
    sched_name: Entity<InputState>,
    sched_custom_secs: Entity<InputState>,
    sched_workspace: Entity<InputState>,
    sched_prompt: Entity<TextareaState>,
    /// 定时任务页状态行（创建/删除结果）
    sched_status: String,
    /// 记忆页：全部记忆条目（`GET /memory`，已聚合 memory/ + user/ + agent/patterns）
    memories: Vec<MemoryRow>,
    /// 记忆页状态行
    memories_status: String,
    /// 记忆页：选中的记忆 uri
    memory_selected: Option<String>,
    /// 记忆页：L2 全文是否展开（默认收起——L0/L1 才是浏览主视图）
    memory_detail_open: bool,
    /// 洞察页：总览统计（`GET /stats`：技能/工具/文档/搜索四个追踪面）
    insight_stats: Option<InsightStats>,
    /// 洞察页：模型用量总计与分组（`GET /usage/stats?days=N`）
    usage_total: Option<UsageTotal>,
    usage_grouped: Vec<UsageGroup>,
    usage_days: i64,
    /// 洞察页：调度器状态（`GET /scheduler/status`）
    scheduler_running: bool,
    scheduler_executing: Option<String>,
    sched_status_tasks: Vec<SchedStatusRow>,
    /// 洞察页状态行
    insights_status: String,
    /// 审批页：工作流配置摘要（`GET /approval/status` 的 `config`）
    approval_mode: String,
    approval_auto: bool,
    approval_timeout: u64,
    approval_max_pending: u64,
    approval_prompt_cmds: Vec<String>,
    approval_rules: Vec<ApprovalRuleRow>,
    /// 审批页：待用户确认的操作指纹（「询问用户」降级链路，agent loop 追问中）
    pending_confirmations: Vec<String>,
    confirmed_action_count: usize,
    /// 审批页：审计记录（后端倒序 50 条，界面显示前 20）
    approval_history: Vec<ApprovalHistRow>,
    /// 审批页状态行
    approvals_page_status: String,
    /// 知识页：当前 tab（0 = 浏览 / 1 = 导入 / 2 = 搜索）
    kb_tab: u8,
    /// 知识页·浏览：当前目录（相对知识根，空 = 根）
    kb_path: String,
    kb_entries: Vec<KbEntryRow>,
    /// 知识页·浏览：选中的条目 uri 与读到的内容（`(level, content)`）
    kb_selected: Option<String>,
    kb_content: Option<(String, String)>,
    /// 知识页状态行（浏览/导入/搜索共用一行）
    kb_status: String,
    /// 知识页·导入：路径与分类输入框、每文件结果
    kb_ingest_path: Entity<InputState>,
    kb_ingest_category: Entity<InputState>,
    kb_ingest_results: Vec<(String, String)>,
    /// 知识页·导入：**内嵌文件浏览器**的当前目录、条目与状态
    ///
    /// 不用系统文件对话框：那是 OS 级模态框，自动化端口既点不到也看不到
    /// （`/state` 里没有它的位置）。内嵌浏览器由 GPUI 自绘，可点、可断言。
    kb_browse_dir: String,
    kb_browse_entries: Vec<KbBrowseEntry>,
    kb_browse_status: String,
    /// 是否显示点开头的隐藏条目（默认关：项目根目录十几个 `.git`/`.cargo`/`.vscode`
    /// 会把第一屏占满，真正想选的文件要滚几屏 —— 选文件的场景下这是纯噪音）
    kb_show_hidden: bool,
    /// 文件选择弹窗里的过滤词（名字包含即匹配；空 = 不过滤）
    kb_pick_query: String,
    kb_pick_search: Entity<InputState>,
    /// 已勾选待导入的文件（完整路径，保持勾选顺序）
    kb_picked: Vec<String>,
    /// 知识页·搜索：查询输入框与结果
    kb_search: Entity<InputState>,
    kb_results: Vec<KbSearchRow>,
    /// 目录选择器：当前浏览目录（绝对路径；浏览根时为「浏览根」）
    picker_current: String,
    /// 目录选择器：上级目录（None = 已在浏览根）
    picker_parent: Option<String>,
    /// 目录选择器：子目录 `(名称, 完整路径)`
    picker_entries: Vec<(String, String)>,
    /// 目录选择器状态行
    picker_status: String,
    /// 文件页：工作区根（绝对路径，来自 tree 响应）
    ws_root: String,
    /// 文件页：当前目录（相对根；`""` = 根）
    ws_path: String,
    /// 文件页：当前目录的条目
    ws_entries: Vec<WsEntry>,
    /// 文件页：打开的文件（`(相对路径, 内容)`）
    ws_file: Option<(String, String)>,
    /// 文件页状态行（加载中 / 截断提示 / 错误）
    ws_status: String,
    /// 文件页：diff 面板是否展开
    ws_diff_open: bool,
    /// 文件页：改动文件列表
    ws_diff_files: Vec<WsDiffFile>,
    /// 文件页：当前查看的 diff（相对路径 + unified 文本）
    ws_diff: Option<(String, String)>,
    /// 文件页：diff 面板状态行
    ws_diff_status: String,
    /// 反向弱引用：`gpui_list` 的渲染闭包只能拿 `&mut App`，点击折叠块时要
    /// 回写 view 状态 → 与旧 `SessionsDelegate` 同一手法
    app_ref: AppRef,
    /// 当前打开的模态对话框（删除确认 / 重命名）
    dialog: Option<Dialog>,
    /// 放大查看的用户消息图片（点击图片打开全屏遮罩；None = 未打开）
    zoomed_image: Option<std::sync::Arc<gpui_kit::Image>>,
    /// 待发送的图片（data URL 列表）：控件行「图片」按钮选入，发送后清空
    composer_images: Vec<String>,
    /// 打开的会话 Tab（浏览器式；按打开顺序，最多 [`MAX_SESSION_TABS`] 个）
    open_tabs: Vec<String>,
    /// 新会话待绑定的工作目录（「＋新目录」设置）：随首条消息以
    /// `working_directory` 下发，服务端建会话时固化（目录不存在则不绑定）
    new_session_workspace: Option<String>,
    /// 重命名输入框：打开对话框时**新建**一个实体（`InputState` 没有公开的
    /// `set_value`，复用旧实体无法回填原标题，只能每次新建）
    dialog_input: Option<Entity<InputState>>,
    /// 消息用 Rc 包一层：闭包驱动的虚拟列表需要把数据带进 'static 闭包，
    /// Rc::clone 是 O(1)，比每帧 clone Vec<MsgRow> 便宜得多
    messages: Rc<Vec<MsgRow>>,
    /// gpui 原生变高虚拟列表的状态（只有可见项会被 render）
    msg_list_state: GpuiListState,
    /// 会话消息总数（messages 可能只保留最近 N 条用于渲染）
    message_total: usize,
    status: String,
    port: u16,
    session_count: usize,
    current_title: String,
    /// 当前选中会话 id —— 事件路由键：只渲染本会话的 `chat_stream` 增量
    current_session: Option<String>,
    /// 输入框（gpui-component 的 `Textarea`：IME / 选区 / 撤销都由它管，不自造）
    input: Entity<TextareaState>,
    /// 是否有轮在跑（`turn_state` 驱动，同时闸住重复发送）
    streaming: bool,
    /// 当前视图
    view: View,
    /// 设置页：模型列表（`GET /api/v1/config/models`）
    models: Vec<ModelRow>,
    /// 设置页状态行（切换结果 / 错误）
    settings_status: String,
    /// 设置页状态行是否为错误（决定用 `--color-error` 还是次级文字色）
    settings_error: bool,
    /// 会话级模型选择（`(provider, model)`）：None = 回落到 `preferences.chat`
    session_model: Option<(String, String)>,
    /// 模型下拉是否展开
    model_menu_open: bool,
    /// 会话级思考强度：`None` = default（不附加思考参数，是否思考由模型决定）。
    /// 档位值来自当前模型声明的 `reasoning_efforts`，**原样展示不翻译**
    /// （厂商可能给 low / high / max 等任意值）
    thinking: Option<String>,
    /// 思考强度下拉是否展开
    thinking_menu_open: bool,
    /// 最近一轮请求的**实时**用量（流式 `usage` 上报写入，切会话即清）。
    /// 上下文占用优先取它，其次回落到消息链自带的 `usage`（见 `ctx_usage`）
    ctx_live: Option<MsgUsage>,
    /// 流式上报的上下文窗口：模型没声明 `context_length` 时的兜底分母
    ctx_window_live: u64,
    /// 上下文详情面板是否展开（点圆环打开：占用 / 缓存命中 / 输出 + 压缩入口）
    ctx_panel_open: bool,
    /// 压缩进行中（按钮禁点 + 文案「压缩中…」）
    ctx_compressing: bool,
    /// 压缩结果提示：`(是否成功, 文案)`
    ctx_compress: Option<(bool, String)>,
    /// 会话 token 汇总（`GET /usage/stats?days=0&session_id=`）：
    /// `(未命中输入, 缓存命中输入, 输出)`；全零时 `None`（不显示统计行）
    session_usage: Option<(u64, u64, u64)>,
    /// 最近一次请求性能（`GET /usage/recent?limit=1`）：
    /// `(ttft_ms, duration_ms, completion_tokens)`；`duration_ms < 0` 时 `None`
    request_perf: Option<(Option<i64>, i64, u64)>,
    /// 本会话的后台任务（`GET /api/v1/tasks` 按 `parent_session_id` 过滤）
    tasks: Vec<TaskRow>,
    /// 任务面板是否折叠（折叠成 36px 竖条，对齐 Tauri 版 dock 的收起态）
    tasks_collapsed: bool,
    /// 会话待办（`(status, title)`，来自 `GET /todos?session_id=`）
    todos: Vec<(String, String)>,
    /// 会话目标（`GET /goals?session_id=`，元素是 `{goal, progress, todo_total, todo_done}`）
    goals: Vec<GoalRow>,
    /// 待办/目标窄条是否折叠
    todo_bar_collapsed: bool,
    /// 本会话的待审批操作（`GET /api/v1/approval/status` 按 `session_id` 过滤）
    pending_approvals: Vec<ApprovalRow>,
    /// 待回答的追问（`ask_user` 同步工具挂起时）：`(问题, 选项)`
    clarification: Option<Vec<(String, Vec<String>)>>,
    /// 已选答案（与 `clarification` 等长；空串 = 未答）
    clarify_answers: Vec<String>,
    /// 设置页：provider 列表（含 endpoint / api_key，供编辑回填）
    providers: Vec<ProviderRow>,
    /// 设置页：正在编辑的 provider（None = 未打开编辑器）
    editing_provider: Option<String>,
    /// 编辑框：endpoint
    edit_endpoint: Entity<TextareaState>,
    /// 编辑框：api_key
    edit_api_key: Entity<TextareaState>,
    /// 本窗口句柄。
    ///
    /// 自动化端口在 `Context` 里执行动作时**没有 `&mut Window`**，而往输入框插文本
    /// （`TextareaState::insert`）需要它——用句柄现取，让两条路径（真实点击 /
    /// 自动化动作）最后都落到同一个插入 API，不出现第二套插入逻辑。
    window_handle: gpui_kit::AnyWindowHandle,
    /// 最近一次内建截图的结果：`(路径, 宽, 高)`。
    ///
    /// 只在 `--features screenshot` 构建里有意义（该构建下动作会写它）；
    /// 生产构建恒为 `None`，字段本身零开销。
    last_screenshot: Option<(String, u32, u32)>,
}

impl ClientApp {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        port: u16,
        tray_rx: tokio::sync::mpsc::UnboundedReceiver<tray::TrayCommand>,
    ) -> Self {
        let expand: ExpandRef = Rc::new(std::cell::RefCell::new(ExpandState::default()));
        let app_ref: AppRef = Arc::new(Mutex::new(None));
        let messages: Rc<Vec<MsgRow>> = Rc::new(Vec::new());
        let total: usize = 0;

        let input = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("输入消息 · Enter 发送 / Shift+Enter 换行")
        });
        // 子智能体页的搜索框：改动即过滤（订阅 Change 事件）
        let roles_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索子智能体..."));
        cx.subscribe_in(
            &roles_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.roles_query = this.roles_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 工具页的搜索框：改动即过滤（同 roles_search 的口径）
        let tools_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索工具..."));
        cx.subscribe_in(
            &tools_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.tools_query = this.tools_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 技能页的搜索框（同口径）
        let skills_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索技能..."));
        cx.subscribe_in(
            &skills_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.skills_query = this.skills_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 文件选择弹窗的过滤框：改动即过滤（同 roles/tools 搜索框的口径）
        let kb_pick_search = cx.new(|cx| InputState::new(window, cx).placeholder("按名字过滤…"));
        cx.subscribe_in(
            &kb_pick_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.kb_pick_query = this.kb_pick_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 知识页的三个输入框（导入路径 / 导入分类 / 搜索词）
        let kb_ingest_path =
            cx.new(|cx| InputState::new(window, cx).placeholder("文件或目录的绝对路径"));
        let kb_ingest_category = cx
            .new(|cx| InputState::new(window, cx).placeholder("分类（可选，如 technical / docs）"));
        let kb_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索知识库..."));
        // 定时任务页的四个表单输入框（名称 / 自定义秒 / 工作目录 / 指令）。
        // 不订阅 Change：值只在「创建」时读一次，没必要每敲一个字就刷视图。
        let sched_name = cx.new(|cx| InputState::new(window, cx).placeholder("任务名称"));
        let sched_custom_secs =
            cx.new(|cx| InputState::new(window, cx).placeholder("自定义间隔（秒，最小 60）"));
        let sched_workspace = cx
            .new(|cx| InputState::new(window, cx).placeholder("工作目录（绝对路径，留空 = 默认）"));
        let sched_prompt = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("给智能体的指令（每次触发让它做的事）")
        });
        // 设置页的两个编辑框：endpoint 与 api_key（打开编辑时从 `GET /config` 回填）
        let endpoint_input =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("API 端点（http(s)://…）"));
        let key_input = cx.new(|cx| TextareaState::new(window, cx).placeholder("API 密钥"));
        // Enter 发送：Textarea 已按多行语义插入换行，发送成功后再清空即无残留
        cx.subscribe_in(
            &input,
            window,
            |this, _input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = event {
                    if !shift {
                        this.send_current_input(window, cx);
                    }
                }
            },
        )
        .detach();

        let app = Self {
            session_rows: Vec::new(),
            sessions_data: Vec::new(),
            collapsed_groups: std::collections::HashSet::new(),
            expanded_groups: std::collections::HashMap::new(),
            hovered_session: None,
            all_tasks: Vec::new(),
            expanded_task: None,
            task_messages: std::collections::HashMap::new(),
            expand,
            app_ref: app_ref.clone(),
            roles: Vec::new(),
            roles_stats: RolesStats::default(),
            roles_query: String::new(),
            roles_search,
            roles_selected: None,
            role_detail: None,
            roles_status: "正在加载子智能体…".to_string(),
            roles_action_msg: None,
            tools: Vec::new(),
            tools_query: String::new(),
            tools_search,
            tools_selected: None,
            skills: Vec::new(),
            skill_stats: Vec::new(),
            skills_total_calls: 0,
            skills_query: String::new(),
            skills_search,
            skills_selected: None,
            skill_content: None,
            skills_status: "正在加载技能…".to_string(),
            sched_tasks: Vec::new(),
            sched_form_open: false,
            sched_preset: 21600,
            sched_name,
            sched_custom_secs,
            sched_workspace,
            sched_prompt,
            sched_status: String::new(),
            memories: Vec::new(),
            memories_status: "正在加载记忆…".to_string(),
            memory_selected: None,
            memory_detail_open: false,
            insight_stats: None,
            usage_total: None,
            usage_grouped: Vec::new(),
            usage_days: 7,
            scheduler_running: false,
            scheduler_executing: None,
            sched_status_tasks: Vec::new(),
            insights_status: "正在加载洞察…".to_string(),
            approval_mode: String::new(),
            approval_auto: false,
            approval_timeout: 0,
            approval_max_pending: 0,
            approval_prompt_cmds: Vec::new(),
            approval_rules: Vec::new(),
            pending_confirmations: Vec::new(),
            confirmed_action_count: 0,
            approval_history: Vec::new(),
            approvals_page_status: "正在加载审批状态…".to_string(),
            kb_tab: 0,
            kb_path: String::new(),
            kb_entries: Vec::new(),
            kb_selected: None,
            kb_content: None,
            kb_status: "正在加载知识库…".to_string(),
            kb_ingest_path,
            kb_ingest_category,
            kb_ingest_results: Vec::new(),
            kb_browse_dir: String::new(),
            kb_browse_entries: Vec::new(),
            kb_browse_status: String::new(),
            kb_show_hidden: false,
            kb_pick_query: String::new(),
            kb_pick_search,
            kb_picked: Vec::new(),
            kb_search,
            kb_results: Vec::new(),
            picker_current: String::new(),
            picker_parent: None,
            picker_entries: Vec::new(),
            picker_status: String::new(),
            ws_root: String::new(),
            ws_path: String::new(),
            ws_entries: Vec::new(),
            ws_file: None,
            ws_status: String::new(),
            ws_diff_open: false,
            ws_diff_files: Vec::new(),
            ws_diff: None,
            ws_diff_status: String::new(),
            dialog: None,
            zoomed_image: None,
            composer_images: Vec::new(),
            open_tabs: Vec::new(),
            new_session_workspace: None,
            dialog_input: None,
            messages,
            msg_list_state: GpuiListState::new(0, ListAlignment::Bottom, px(120.)),
            message_total: total,
            status: "正在连接后端…".to_string(),
            port,
            session_count: 0,
            current_title: "选择左侧会话查看消息".to_string(),
            current_session: None,
            input,
            streaming: false,
            view: View::Chat,
            models: Vec::new(),
            settings_status: "正在加载模型列表…".to_string(),
            settings_error: false,
            session_model: None,
            model_menu_open: false,
            thinking: None,
            thinking_menu_open: false,
            ctx_live: None,
            ctx_window_live: 0,
            ctx_panel_open: false,
            ctx_compressing: false,
            ctx_compress: None,
            session_usage: None,
            request_perf: None,
            tasks: Vec::new(),
            tasks_collapsed: false,
            todos: Vec::new(),
            goals: Vec::new(),
            todo_bar_collapsed: false,
            pending_approvals: Vec::new(),
            clarification: None,
            clarify_answers: Vec::new(),
            providers: Vec::new(),
            editing_provider: None,
            edit_endpoint: endpoint_input,
            edit_api_key: key_input,
            window_handle: window.window_handle(),
            last_screenshot: None,
        };
        // 回填自身弱引用：消息区折叠块的点击回调要用它 update 回 view
        if let Ok(mut g) = app_ref.lock() {
            *g = Some(cx.entity().downgrade());
        }
        // 关闭按钮 → 藏到托盘（不是退出）。托盘「退出天演」会先 `begin_quit()`
        // 置位，这里的回调据此放行——否则用户永远关不掉这个程序。
        window.on_window_should_close(cx, |_window, _cx| {
            if tray::is_quitting() {
                true
            } else {
                tray::hide_main_window();
                false
            }
        });
        // 托盘菜单动作（主线程执行）：显隐窗口 / 退出。
        // 窗口操作走 `tray::*` 的 Win32 自由函数而不是 GPUI API —— 后者只有
        // minimize/activate，没有 hide（而托盘语义要求窗口从任务栏消失）。
        cx.spawn(async move |this, cx| {
            let mut tray_rx = tray_rx;
            while let Some(cmd) = tray_rx.recv().await {
                match cmd {
                    tray::TrayCommand::Show => tray::show_main_window(),
                    tray::TrayCommand::Hide => tray::hide_main_window(),
                    tray::TrayCommand::Quit => {
                        tray::begin_quit();
                        let _ = this.update(cx, |this, cx| {
                            let handle = this.window_handle;
                            let _ = handle.update(cx, |_, window, _cx| window.remove_window());
                        });
                        break;
                    }
                }
            }
        })
        .detach();
        // 统一事件常驻订阅（ADR-028）：订阅在窗口创建时建立，与「发送」解耦
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        spawn_event_reader(port, tx);
        cx.spawn(async move |this, cx| loop {
            let Some(ev) = rx.recv().await else {
                break;
            };
            if this
                .update(cx, |this, cx| this.handle_event(&ev, cx))
                .is_err()
            {
                break;
            }
        })
        .detach();
        app.load_sessions(cx);
        app.load_models(cx);
        // 子智能体页数据预加载：等到切页时才拉会让用户先看到空白再「跳」出内容
        app.load_roles(cx);
        app.load_roles_stats(cx);
        // 后台任务面板轮询（3s，对齐 Tauri 版 `AgentTasksPanel.POLL_INTERVAL_MS`）。
        // 面板是观测面：只在有选中会话时拉，失败静默（`load_tasks` 内部处理）。
        cx.spawn(async move |this, cx| loop {
            sleep_secs(3).await;
            let alive = this.update(cx, |this, cx| {
                // 任务列表**无条件**轮询：左栏会话角标要按 `parent_session_id`
                // 聚合所有会话，未选中会话时也得更新；面板那三个只在有会话时拉。
                this.load_tasks(cx);
                if this.current_session.is_some() {
                    this.load_todos_goals(cx);
                    this.load_approvals(cx);
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
        app
    }

    fn load_sessions(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            match fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/sessions")).await {
                Ok(body) => {
                    let arr = body
                        .get("sessions")
                        .and_then(|v| v.as_array())
                        .or_else(|| body.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let rows: Vec<SessionRow> = arr
                        .iter()
                        .map(|s| SessionRow {
                            id: s
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            title: s
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            working_directory: s
                                .get("working_directory")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            updated_at: s
                                .get("updated_at")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            first_request_tokens: s
                                .get("first_request_tokens")
                                .and_then(|v| v.as_u64()),
                        })
                        .collect();
                    let n = rows.len();
                    println!("[会话] 拉到 {n} 条");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.sessions_data = rows;
                            this.session_count = n;
                            this.status = format!("127.0.0.1:{port} · 会话 {n} 条");
                            this.rebuild_session_rows(cx);
                        });
                    });
                }
                Err(e) => {
                    println!("[会话] 拉取失败: {e}");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.status = format!("拉取会话失败：{e}");
                            cx.notify();
                        });
                    });
                }
            }
        })
        .detach();
    }

    /// 打开「重命名会话」对话框：新建输入框实体并回填原标题。
    ///
    /// `InputState` 没有公开的 `set_value`，所以每次开弹窗新建一个实体，
    /// 而不是复用一个再回填（复用的写法编译不过）。
    fn open_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .sessions_data
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        self.dialog_input = Some(cx.new(|cx| InputState::new(window, cx).default_value(title)));
        self.dialog = Some(Dialog::RenameSession { id });
        cx.notify();
    }

    /// 打开目录选择器（先拉浏览根 = 盘符/家目录，再逐级进入）。
    ///
    /// 取代此前的「手输绝对路径」：路径打错不会有任何提示，浏览选择才是正常交互。
    fn open_workspace_picker(&mut self, target: PickerTarget, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::WorkspacePicker { target });
        self.picker_current.clear();
        self.picker_parent = None;
        self.picker_entries.clear();
        self.picker_status = "正在读取目录…".to_string();
        self.load_picker_dirs(String::new(), cx);
        cx.notify();
    }

    /// 拉某一级目录的子目录列表（`GET /workspace/dirs?path=`；空串 = 浏览根）。
    fn load_picker_dirs(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = if path.is_empty() {
                format!("http://{SERVER_HOST}:{port}/api/v1/workspace/dirs")
            } else {
                format!(
                    "http://{SERVER_HOST}:{port}/api/v1/workspace/dirs?path={}",
                    url_encode(&path)
                )
            };
            match fetch_json(url).await {
                Ok(body) => {
                    let current = body
                        .get("current")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let parent = body
                        .get("parent")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let entries: Vec<(String, String)> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some((
                                        e.get("name")?.as_str()?.to_string(),
                                        e.get("path")?.as_str()?.to_string(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = entries.len();
                    let _ = this.update(cx, |this, cx| {
                        this.picker_current = current;
                        this.picker_parent = parent;
                        this.picker_entries = entries;
                        this.picker_status = format!("{n} 个子目录");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.picker_status = format!("读取目录失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 目录选择器「选这个目录」：按目标分派（新会话待绑定 / 绑定当前会话）。
    fn confirm_workspace_picker(&mut self, target: PickerTarget, cx: &mut Context<Self>) {
        // 浏览根（未进入任何盘符）时不可选
        if self.picker_current.is_empty() || self.picker_current == "浏览根" {
            self.picker_status = "请先进入一个目录".to_string();
            cx.notify();
            return;
        }
        let path = self.picker_current.clone();
        self.dialog = None;
        match target {
            PickerTarget::NewSession => {
                self.new_session_workspace = Some(path.clone());
                self.start_new_session(cx);
                self.status = format!("新会话将绑定目录：{path}");
                cx.notify();
            }
            PickerTarget::Session => {
                let Some(sid) = self.current_session.clone() else {
                    self.ws_status = "没有活动会话，无法绑定目录".to_string();
                    cx.notify();
                    return;
                };
                let port = self.port;
                cx.spawn(async move |this, cx| {
                    let url =
                        format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/workspace");
                    match put_json(url, serde_json::json!({ "working_directory": path })).await {
                        Ok(_) => {
                            let _ = this.update(cx, |this, cx| {
                                if let Some(s) = this.sessions_data.iter_mut().find(|s| s.id == sid)
                                {
                                    s.working_directory = path.clone();
                                }
                                // 目录变了：文件页缓存作废并重新拉树
                                this.ws_root.clear();
                                this.ws_entries.clear();
                                this.ws_file = None;
                                this.ws_path.clear();
                                this.load_workspace_tree(String::new(), cx);
                                this.rebuild_session_rows(cx);
                                cx.notify();
                            });
                        }
                        Err(e) => {
                            let _ = this.update(cx, |this, cx| {
                                this.ws_status = format!("绑定目录失败：{e}");
                                cx.notify();
                            });
                        }
                    }
                })
                .detach();
            }
        }
    }

    /// 提交重命名（`POST /sessions/{id}/title`），成功后就地更新左栏标题。
    fn submit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::RenameSession { id }) = self.dialog.clone() else {
            return;
        };
        let title = self
            .dialog_input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default();
        self.dialog = None;
        self.dialog_input = None;
        if title.is_empty() {
            cx.notify();
            return;
        }
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{id}/title");
            match post_json(url, serde_json::json!({ "title": title })).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(s) = this.sessions_data.iter_mut().find(|s| s.id == id) {
                            s.title = title.clone();
                        }
                        this.rebuild_session_rows(cx);
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = format!("重命名失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 选择图片附件（OS 文件对话框）→ 读成 data URL 挂到待发送列表。
    ///
    /// 注：OS 模态框不可自动化（ADR-052），自动化验收走 `attach_image` 动作
    /// 直接给路径（与知识导入「按路径导入」同一口径）。
    fn pick_composer_images(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("选择图片".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let urls: Vec<String> = paths
                .iter()
                .filter_map(|p| image_path_to_data_url(p))
                .collect();
            if urls.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.composer_images.extend(urls);
                cx.notify();
            });
        })
        .detach();
    }

    /// 输入卡片里的附件缩略图条（每张带移除按钮）。
    ///
    /// 抽成独立方法只为**压 `render_chat` 的栈帧**（GPUI 递归 prepaint，单帧过大
    /// 会栈溢出）。
    fn render_composer_attachments(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut strip = div().w_full().flex().flex_row().flex_wrap().gap_2();
        for (i, src) in self.composer_images.iter().enumerate() {
            let Some((fmt, bytes)) = decode_data_url(src) else {
                continue;
            };
            let arc = std::sync::Arc::new(gpui_kit::Image::from_bytes(fmt, bytes));
            strip = strip.child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_1()
                    .child(
                        gpui_kit::img(gpui_kit::ImageSource::Image(arc))
                            .max_w(px(64.))
                            .max_h(px(64.))
                            .rounded_md(),
                    )
                    .child(
                        div()
                            .id(("rm-attach", i))
                            .px_1()
                            .rounded_md()
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::X.path())
                                    .size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY())),
                            )
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                if i < this.composer_images.len() {
                                    this.composer_images.remove(i);
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        strip.into_any_element()
    }

    /// 请求回退到某条用户消息之前：先弹确认框（删除不可恢复）。
    fn request_rollback(
        &mut self,
        session_id: String,
        message_id: String,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.dialog = Some(Dialog::RollbackMessage {
            session_id,
            message_id,
            preview: ellipsis(text.trim(), 16),
        });
        cx.notify();
    }

    /// 确认回退：`POST /sessions/{id}/messages/delete`（删该消息及其后全部；
    /// ADR-040 事务内含工作区文件回退），成功后重拉消息链与会话统计。
    fn confirm_rollback(&mut self, session_id: String, message_id: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url =
                format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{session_id}/messages/delete");
            let res = post_json(url, serde_json::json!({ "message_id": message_id })).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.status = "已回退".to_string();
                        this.load_messages(session_id.clone(), cx);
                        this.load_session_usage(cx);
                        this.load_sessions(cx);
                    }
                    Err(e) => this.status = format!("回退失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 确认删除会话（`DELETE /sessions/{id}`）。
    ///
    /// 删除的是当前会话时清空主区（消息 / 统计 / 待办 / 目标），回到「选一个会话」空态。
    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::DeleteSession { id, .. }) = self.dialog.clone() else {
            return;
        };
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{id}");
            match delete_json(url).await {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.sessions_data.retain(|s| s.id != id);
                        if this.current_session.as_deref() == Some(id.as_str()) {
                            this.current_session = None;
                            this.messages = Rc::new(Vec::new());
                            this.msg_list_state =
                                GpuiListState::new(0, ListAlignment::Bottom, px(120.));
                            this.message_total = 0;
                            this.current_title = "选择左侧会话查看消息".to_string();
                            this.session_usage = None;
                            this.request_perf = None;
                            this.todos.clear();
                            this.goals.clear();
                        }
                        this.rebuild_session_rows(cx);
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = format!("删除失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 模态对话框覆盖层（删除确认 / 重命名）：整窗半透明遮罩 + 居中卡片。
    ///
    /// 自绘而不接组件库 `Dialog`：只有两种形态，自绘更可控（遮罩色、按钮危险色
    /// 都能直接对齐深色主题），也避免为弹窗引入一套焦点/Popover 语义。
    fn render_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.dialog.clone() else {
            return div().into_any_element();
        };
        // 目录选择器是「列表 + 逐级导航」的独立形态，不走下面那条卡片骨架
        if let Dialog::WorkspacePicker { target } = dialog {
            return self.render_workspace_picker_dialog(target, cx);
        }
        // 文件选择弹窗同理：列表 + 导航 + 选中动作，独立形态
        if let Dialog::FilePicker { purpose } = dialog {
            return self.render_file_picker_dialog(purpose, cx);
        }
        let (heading, message, confirm_label, danger) = match &dialog {
            Dialog::DeleteSession { title, .. } => (
                "删除会话".to_string(),
                Some(format!("确定删除会话「{title}」吗？此操作不可恢复。")),
                "删除".to_string(),
                true,
            ),
            Dialog::RenameSession { .. } => {
                ("重命名会话".to_string(), None, "保存".to_string(), false)
            }
            // 目录选择器是独立形态（列表 + 导航），在 `render_dialog` 开头就被分流，
            // 走不到这里；给个兜底以免新增变体时漏配
            Dialog::WorkspacePicker { .. } => {
                ("选择目录".to_string(), None, "选择".to_string(), false)
            }
            // 文件选择弹窗在 `render_dialog` 开头就分流了，走不到这里
            Dialog::FilePicker { .. } => {
                ("选择文件".to_string(), None, "确定".to_string(), false)
            }
            Dialog::ResetRole { name } => (
                "回退内置种子".to_string(),
                Some(format!(
                    "将 {name} 回退到内置种子定义（学习演化结果会被覆盖）？"
                )),
                "回退".to_string(),
                true,
            ),
            Dialog::DeleteRole { name } => (
                "退役删除角色".to_string(),
                Some(format!("退役并删除角色 {name}（定义与会话一并删除）？")),
                "删除".to_string(),
                true,
            ),
            Dialog::RollbackMessage { preview, .. } => (
                "回退到此处".to_string(),
                Some(format!(
                    "将删除「{preview}」这条消息及其之后的全部内容（含工作区文件回退），不可恢复。"
                )),
                "回退".to_string(),
                true,
            ),
        };
        let mut card = div()
            .w(px(420.))
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(15.))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .child(heading),
            );
        if let Some(msg) = message {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(msg),
            );
        }
        // 重命名用输入框（目录选择器是独立形态，走别的分支）
        if let (Dialog::RenameSession { .. }, Some(input)) = (&dialog, self.dialog_input.as_ref()) {
            card = card.child(
                div()
                    .w_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_PRIMARY()))
                    .px_2()
                    .py_1()
                    .child(Input::new(input).appearance(false)),
            );
        }
        card = card.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(
                    div()
                        .id("dlg-cancel")
                        .px_3()
                        .py_1()
                        .rounded_lg()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child("取消")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dialog = None;
                            this.dialog_input = None;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("dlg-confirm")
                        .px_3()
                        .py_1()
                        .rounded_lg()
                        .cursor_pointer()
                        .text_sm()
                        .bg(rgb(if danger {
                            colors::ERROR()
                        } else {
                            colors::ACCENT()
                        }))
                        .text_color(rgb(0xffffff))
                        .child(confirm_label)
                        .on_click(
                            cx.listener(move |this, _, _, cx| match this.dialog.clone() {
                                Some(Dialog::DeleteSession { .. }) => this.confirm_delete(cx),
                                Some(Dialog::RenameSession { .. }) => this.submit_rename(cx),
                                Some(Dialog::WorkspacePicker { .. }) => {}
                                // 文件选择弹窗同样是独立形态（不走这条骨架）
                                Some(Dialog::FilePicker { .. }) => {}
                                Some(Dialog::ResetRole { name }) => {
                                    this.confirm_reset_role(name, cx)
                                }
                                Some(Dialog::DeleteRole { name }) => {
                                    this.confirm_delete_role(name, cx)
                                }
                                Some(Dialog::RollbackMessage {
                                    session_id,
                                    message_id,
                                    ..
                                }) => this.confirm_rollback(session_id, message_id, cx),
                                None => {}
                            }),
                        ),
                ),
        );
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0))
            .child(card)
            .into_any_element()
    }

    /// 左栏的一行（分组 / 会话 / 查看更多）。
    ///
    /// 行高由内容决定，不再用等高虚拟列表：三类行的高度本来就不一样，而
    /// `List` 的测量缓存会把「查看更多」压到上一行标题上（实测重叠）。
    fn render_session_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.session_rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        match row {
            SessionListRow::Group {
                key,
                label,
                count,
                collapsed,
            } => {
                let chevron = if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                };
                div()
                    .id(("ws-group", ix))
                    .w_full()
                    .px_1()
                    .py_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(chevron.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        svg()
                            .path(IconName::FolderOpen.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{count}")),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_group(key.clone(), cx);
                    }))
                    .into_any_element()
            }
            SessionListRow::Session {
                id,
                title,
                meta,
                tasks,
                selected,
            } => {
                // 行内操作按钮要带走 id / 标题（`title` 随后会被 move 进标题节点）
                let rename_id = id.clone();
                let delete_id = id.clone();
                let hover_id = id.clone();
                let delete_title = title.clone();
                // 悬停才显示行内操作（对齐 Web 版）；容器宽度固定，
                // 避免鼠标进出时标题区反复变宽变窄
                let is_hovered = self.hovered_session.as_deref() == Some(id.as_str());
                let mut head = div().flex().flex_row().items_center().gap_2().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .text_sm()
                        .text_color(rgb(if selected {
                            colors::ACCENT()
                        } else {
                            colors::TEXT_PRIMARY()
                        }))
                        .child(if title.is_empty() {
                            "新对话".to_string()
                        } else {
                            title
                        }),
                );
                if tasks > 0 {
                    head = head.child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_full()
                            .bg(rgb(colors::ERROR()))
                            .text_size(px(9.))
                            .text_color(rgb(0xffffff))
                            .child(if tasks > 99 {
                                "99+".to_string()
                            } else {
                                format!("{tasks}")
                            }),
                    );
                }
                // 行内操作：重命名 / 删除。**只在悬停这一行时渲染**（对齐 Web 版
                // `hoveredSession === session.id`），且**不预留宽度**——留着 50px
                // 空位的话，标题在非悬停时就被白占了，hover 与不 hover 看起来没差别
                // （用户指出过）。代价是悬停时标题区会窄 50px，与 Web 版一致。
                // 两个回调都 `stop_propagation()`——整行的 on_click 是「打开会话」，
                // 不拦住会把弹窗和切会话一起触发。
                head = head.child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(if is_hovered {
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .id(("rename", ix))
                                        .size(px(22.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .child(
                                            svg()
                                                .path(IconName::Pencil.path())
                                                .size(px(12.))
                                                .text_color(rgb(colors::TEXT_TERTIARY())),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.open_rename(rename_id.clone(), window, cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id(("delete", ix))
                                        .size(px(22.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .child(
                                            svg()
                                                .path(IconName::Trash.path())
                                                .size(px(12.))
                                                .text_color(rgb(colors::TEXT_TERTIARY())),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.dialog = Some(Dialog::DeleteSession {
                                                id: delete_id.clone(),
                                                title: delete_title.clone(),
                                            });
                                            cx.notify();
                                        })),
                                )
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        }),
                );
                div()
                    .id(("ws-session", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(if selected {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(head)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(meta),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_session(id.clone(), cx);
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                        // 行内操作按钮只在悬停这一行时渲染（对齐 Web 版口径）
                        let next = if *hovered {
                            Some(hover_id.clone())
                        } else {
                            None
                        };
                        if this.hovered_session != next {
                            this.hovered_session = next;
                            cx.notify();
                        }
                    }))
                    .into_any_element()
            }
            SessionListRow::More { key, hidden } => div()
                .id(("ws-more", ix))
                .w_full()
                .pl_4()
                .pr_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child("查看更多")
                .child(
                    div()
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("({hidden})")),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.expand_group(key.clone(), cx);
                }))
                .into_any_element(),
        }
    }

    /// 拉本会话的 token 汇总与最近一次请求性能（输入卡片底部统计行）。
    ///
    /// 对应 Web 版两个接口：
    /// - `/usage/stats?days=0&session_id=` 按会话聚合 `usage_logs`（全时段，与消息分页无关）
    /// - `/usage/recent?limit=1&session_id=` 最近一条样本（ttft / 时长 / 输出 token）
    ///
    /// 失败**静默保留旧值**：统计条是观测面，不能因一次抖动把已有数字清空。
    fn load_session_usage(&self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let stats = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/stats?days=0&session_id={sid}"
            ))
            .await
            .ok();
            let recent = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/recent?limit=1&session_id={sid}"
            ))
            .await
            .ok();
            let usage = stats
                .as_ref()
                .and_then(|b| b.get("total"))
                .map(|t| {
                    (
                        t.get("uncached_input")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        t.get("cached_input").and_then(|v| v.as_u64()).unwrap_or(0),
                        t.get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                    )
                })
                .filter(|(a, b, c)| a + b + c > 0);
            let perf = recent
                .as_ref()
                .and_then(|b| b.get("samples"))
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|s| {
                    let duration = s.get("duration_ms").and_then(|v| v.as_i64())?;
                    if duration < 0 {
                        return None;
                    }
                    let ttft = s
                        .get("ttft_ms")
                        .and_then(|v| v.as_i64())
                        .filter(|v| *v >= 0);
                    let completion = s
                        .get("completion_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    Some((ttft, duration, completion))
                });
            let _ = this.update(cx, |this, cx| {
                this.session_usage = usage;
                this.request_perf = perf;
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载子智能体列表（`GET /roles`）。
    /// 加载审批页（`GET /approval/status`：工作流配置 + 待处理 + 审计）。
    ///
    /// 同时刷一次待审批列表（`load_approvals`）—— 那个走的是同一份
    /// `pending_approvals`，两个入口（对话横幅 / 审批页）必须看到同一批数据。
    /// 打开文件选择弹窗：默认定位到「当前会话的工作目录」→「用户主目录」。
    ///
    /// 直接落在盘符根是很糟的默认值 —— 一屏全是 `hiberfil.sys` / `pagefile.sys`
    /// 这类系统文件，既找不到东西也说明不了从哪开始找。
    fn open_kb_picker(&mut self, cx: &mut Context<Self>) {
        self.open_file_picker(FilePickPurpose::KbIngest, cx);
    }

    /// 打开文件选择弹窗（知识导入 / @ 引用共用一套浏览 UI）。
    fn open_file_picker(&mut self, purpose: FilePickPurpose, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::FilePicker { purpose });
        self.kb_picked.clear();
        let start = self.kb_picker_start_dir();
        self.load_kb_browse(start, cx);
        cx.notify();
    }
    /// 在输入框**光标处**插入一段文本（光标自动前移到插入内容之后）。
    ///
    /// 用 `TextareaState::insert` 而不是「读 value → 拼串 → `set_value`」：
    /// `set_value` 会把选区重置到开头并清空撤销栈，用户接着打字的位置就丢了。
    fn insert_into_composer(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.input.clone();
        input.update(cx, |state, cx| {
            state.insert(text, window, cx);
        });
        cx.notify();
    }

    /// @ 文件引用：把选中的文件序列化成 `` `@file:"<绝对路径>"` `` 插入输入框并关闭弹窗。
    ///
    /// 序列化口径与 Web 版 **逐字一致**（`gui-vite/src/lib/attachments.ts` 的
    /// `serializeInlineChip`：Markdown 行内代码 + 绝对路径）：
    /// - 模型侧看到的是连贯句子里的行内 code，路径直白，`read_file` 直接可用；
    /// - 消息区渲染不需要特殊处理（行内 code 是 markdown 原生语法）；
    /// - 跨端复制粘贴、草稿恢复都只是纯文本。
    ///
    /// 末尾补一个空格：连续引用两个文件时 chip 之间不会粘在一起。
    fn mention_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let snippet = format!("`@file:\"{path}\"` ");
        self.insert_into_composer(snippet, window, cx);
        self.dialog = None;
        cx.notify();
    }

    /// @ 文件引用（无 `&mut Window` 的入口：自动化端口）。
    ///
    /// `Context` 里拿不到 window，就用保存的窗口句柄现取；**最终仍调
    /// `TextareaState::insert`**（与真实点击路径同一个 API），不复制插入逻辑。
    fn mention_file_auto(&mut self, path: String, cx: &mut Context<Self>) -> Result<(), String> {
        let handle = self.window_handle;
        let input = self.input.clone();
        let snippet = format!("`@file:\"{path}\"` ");
        handle
            .update(cx, move |_, window, cx| {
                input.update(cx, |state, cx| {
                    state.insert(snippet, window, cx);
                });
            })
            .map_err(|e| format!("窗口已关闭，无法插入：{e}"))?;
        self.dialog = None;
        cx.notify();
        Ok(())
    }

    /// 内建截图：把当前帧渲染成 PNG 写盘（需 `--features screenshot`）。
    ///
    /// 与 `scripts/ui-shot.ps1`（进程外 DPI 抓屏）的三点区别，正是那个脚本踩过的坑：
    /// 1. **不受 DPI 影响**：拿的是渲染输出，不是屏幕像素（抓屏必须
    ///    `SetProcessDpiAwareness(2)`，否则 150% 缩放下只抓到窗口左上角）；
    /// 2. **不受遮挡影响**：不需要把窗口置顶，别的窗口盖着也能截；
    /// 3. **窗口隐藏时也能截**：`render_to_image` 渲染的是"这一帧本该画成什么"，
    ///    不读屏幕——所以「藏到托盘」状态下仍然可以验证界面。
    ///
    /// 生产构建不带这个能力（`screenshot` feature 默认关闭，见 Cargo.toml）：
    /// 它依赖 gpui 的 `test-support`，那套测试支撑不该进 MSI。
    #[cfg(feature = "screenshot")]
    fn screenshot_to(&mut self, path: String, cx: &mut Context<Self>) -> Result<(), String> {
        let handle = self.window_handle;
        let target = path.clone();
        let size = handle
            .update(cx, move |_, window, _cx| {
                let img = window
                    .render_to_image()
                    .map_err(|e| format!("渲染当前帧失败：{e}"))?;
                let (w, h) = (img.width(), img.height());
                img.save(&target)
                    .map_err(|e| format!("写 PNG 失败（{target}）：{e}"))?;
                Ok::<(u32, u32), String>((w, h))
            })
            .map_err(|e| format!("窗口已关闭，无法截图：{e}"))??;

        self.last_screenshot = Some((path, size.0, size.1));
        cx.notify();
        Ok(())
    }

    /// 清空输入框。
    ///
    /// 只给自动化收尾用（测完不让残留文本污染用户下一次发送）。转的是
    /// `TextareaState::set_value` 这个既有 API，没有第二套写入口。
    fn clear_composer(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let handle = self.window_handle;
        let input = self.input.clone();
        handle
            .update(cx, move |_, window, cx| {
                input.update(cx, |state, cx| {
                    state.set_value("", window, cx);
                });
            })
            .map_err(|e| format!("窗口已关闭：{e}"))?;
        cx.notify();
        Ok(())
    }


    /// 文件选择弹窗的起始目录。
    fn kb_picker_start_dir(&self) -> String {
        if let Some(sid) = &self.current_session {
            if let Some(s) = self.sessions_data.iter().find(|s| &s.id == sid) {
                if !s.working_directory.is_empty() {
                    return s.working_directory.clone();
                }
            }
        }
        std::env::var("USERPROFILE").unwrap_or_default()
    }

    /// 文件选择弹窗的数据源：列某一级（目录 + 文件，`include_files=true`）。
    ///
    /// 走 `/workspace/dirs` 而不是新接口：同一个目录遍历，只是多一个开关
    /// （目录选择器仍只要目录）。空路径 = 浏览根（盘符列表）。
    fn load_kb_browse(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let mut url =
                format!("http://{SERVER_HOST}:{port}/api/v1/workspace/dirs?include_files=true");
            if !path.is_empty() {
                url.push_str(&format!("&path={}", url_encode(&path)));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let current = body
                        .get("current")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let rows: Vec<KbBrowseEntry> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some(KbBrowseEntry {
                                        name: e.get("name")?.as_str()?.to_string(),
                                        path: e.get("path")?.as_str()?.to_string(),
                                        is_dir: e
                                            .get("is_dir")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        size: e.get("size").and_then(|v| v.as_u64()),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.kb_browse_dir = current;
                        this.kb_browse_entries = rows;
                        this.kb_browse_status = format!("{n} 项");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_browse_status = format!("读取目录失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 勾选 / 取消勾选一个文件（目录不参与勾选，点它只是进入）。
    fn toggle_kb_pick(&mut self, path: String) {
        if let Some(pos) = self.kb_picked.iter().position(|p| p == &path) {
            self.kb_picked.remove(pos);
        } else {
            self.kb_picked.push(path);
        }
    }

    /// 导入已勾选的文件（浏览器路径与「按路径导入」共用 [`ClientApp::kb_ingest_paths`]）。
    fn import_kb_picked(&mut self, cx: &mut Context<Self>) {
        if self.kb_picked.is_empty() {
            self.kb_status = "还没勾选文件".to_string();
            cx.notify();
            return;
        }
        let paths: Vec<std::path::PathBuf> = self
            .kb_picked
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        let category = self.kb_ingest_category.read(cx).value().trim().to_string();
        self.kb_ingest_paths(paths, category, cx);
    }

    /// 知识页·浏览：拉某一级条目（`GET /knowledge/entries?path=`）。
    fn load_kb_entries(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let mut url = format!("http://{SERVER_HOST}:{port}/api/v1/knowledge/entries");
            if !path.is_empty() {
                url.push_str(&format!("?path={}", url_encode(&path)));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<KbEntryRow> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some(KbEntryRow {
                                        uri: e.get("uri")?.as_str()?.to_string(),
                                        name: e
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        is_dir: e
                                            .get("is_directory")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        has_abstract: e
                                            .get("has_abstract")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        has_overview: e
                                            .get("has_overview")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        has_detail: e
                                            .get("has_detail")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.kb_path = path;
                        this.kb_entries = rows;
                        this.kb_status = format!("{n} 个条目");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_status = format!("加载知识条目失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 知识页·浏览：读条目某一层内容（`GET /knowledge/entries/read`）。
    fn read_kb_entry(&self, uri: String, level: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/knowledge/entries/read?uri={}&level={}",
                url_encode(&uri),
                url_encode(&level)
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let got_level = body
                        .get("level")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&level)
                        .to_string();
                    let content = body
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let _ = this.update(cx, |this, cx| {
                        this.kb_selected = Some(uri);
                        this.kb_content = Some((got_level, content));
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_status = format!("读取条目失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 知识页·浏览：删除条目（`POST /knowledge/entries/delete`；仅 knowledge 命名空间）。
    fn delete_kb_entry(&mut self, uri: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/knowledge/entries/delete");
            let res = post_json(url, serde_json::json!({ "uri": uri })).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.kb_status = "已删除".to_string();
                        this.kb_selected = None;
                        this.kb_content = None;
                        this.load_kb_entries(this.kb_path.clone(), cx);
                    }
                    Err(e) => this.kb_status = format!("删除失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 知识页·搜索：语义检索（`GET /knowledge/search?q=`）。
    fn search_kb(&self, q: String, cx: &mut Context<Self>) {
        if q.trim().is_empty() {
            return;
        }
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/knowledge/search?q={}&limit=20",
                url_encode(&q)
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<KbSearchRow> = body
                        .get("results")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    Some(KbSearchRow {
                                        id: r.get("id")?.as_str()?.to_string(),
                                        content: r
                                            .get("content")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        source: r
                                            .get("source")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        score: r
                                            .get("score")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0)
                                            as f32,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let total = body.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                    let _ = this.update(cx, |this, cx| {
                        this.kb_results = rows;
                        this.kb_status = format!("命中 {total} 条");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_status = format!("检索失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 知识页·导入：读本地路径 → multipart 上传（`POST /knowledge/ingest`）。
    ///
    /// 用**路径输入**而不是系统文件选择器：与 workspace 目录选择器同一手感，
    /// 且能被自动化 API 直接驱动（文件选择器是模态的，脚本没法点）。
    /// 目录会递归收集（上限 200 个文件——防手滑把整个盘导进去）。
    fn ingest_kb(&mut self, cx: &mut Context<Self>) {
        let raw = self.kb_ingest_path.read(cx).value().trim().to_string();
        let category = self.kb_ingest_category.read(cx).value().trim().to_string();
        self.kb_ingest_direct(raw, category, cx);
    }

    /// 路径输入入口：单一路径 → 交给 [`ClientApp::kb_ingest_paths`]。
    ///
    /// 自动化 `kb_ingest` 与手输路径共用这一条；文件选择器直接走 `kb_ingest_paths`。
    fn kb_ingest_direct(&mut self, raw: String, category: String, cx: &mut Context<Self>) {
        if raw.is_empty() {
            self.kb_status = "请选择文件/目录，或填写绝对路径".to_string();
            cx.notify();
            return;
        }
        self.kb_ingest_paths(vec![std::path::PathBuf::from(&raw)], category, cx);
    }

    /// 按路径列表导入（**路径输入与文件选择器共用这一份**）。
    fn kb_ingest_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        category: String,
        cx: &mut Context<Self>,
    ) {
        let files = collect_ingest_files(&paths);
        if files.is_empty() {
            self.kb_status = "选中的路径里没有可导入的文件".to_string();
            cx.notify();
            return;
        }
        let port = self.port;
        self.kb_status = format!("正在导入 {} 个文件…", files.len());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut form = reqwest::multipart::Form::new();
            if !category.is_empty() {
                form = form.text(
                    "metadata",
                    serde_json::json!({ "source_type": category }).to_string(),
                );
            }
            for f in &files {
                // 一次性导入动作，文件不大；此处阻塞读可接受（在 tokio 线程上顺序读）
                let Ok(bytes) = std::fs::read(f) else {
                    continue;
                };
                let name = f
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                form = form.part(
                    "files",
                    reqwest::multipart::Part::bytes(bytes).file_name(name),
                );
            }
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/knowledge/ingest");
            let res = reqwest::Client::new()
                .post(&url)
                .multipart(form)
                .send()
                .await;
            let parsed = match res {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(body) => Ok(body),
                    Err(e) => Err(format!("响应解析失败：{e}")),
                },
                Err(e) => Err(format!("请求失败：{e}")),
            };
            let _ = this.update(cx, |this, cx| {
                match parsed {
                    Ok(body) => {
                        let msg = body
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("已提交")
                            .to_string();
                        this.kb_ingest_results = body
                            .get("files")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|f| {
                                        Some((
                                            f.get("filename")?.as_str()?.to_string(),
                                            f.get("status")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("?")
                                                .to_string(),
                                        ))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        this.kb_status = msg;
                        this.load_kb_entries(String::new(), cx);
                    }
                    Err(e) => this.kb_status = e,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_approval_page(&self, cx: &mut Context<Self>) {
        self.load_approvals(cx);
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/approval/status");
            match fetch_json(url).await {
                Ok(body) => {
                    let config = body.get("config");
                    let mode = config
                        .and_then(|c| c.get("mode"))
                        .map(|v| {
                            v.as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| v.to_string())
                        })
                        .unwrap_or_default();
                    let auto = config
                        .and_then(|c| c.get("enable_auto_approval"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let timeout = config
                        .and_then(|c| c.get("default_timeout_secs"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let max_pending = config
                        .and_then(|c| c.get("max_pending_approvals"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let prompt_cmds: Vec<String> = config
                        .and_then(|c| c.get("prompt_commands"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let rules: Vec<ApprovalRuleRow> = config
                        .and_then(|c| c.get("auto_approval_rules"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    Some(ApprovalRuleRow {
                                        name: r.get("name")?.as_str()?.to_string(),
                                        decision: r
                                            .get("decision")
                                            .map(|v| {
                                                v.as_str()
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| v.to_string())
                                            })
                                            .unwrap_or_default(),
                                        enabled: r
                                            .get("enabled")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(true),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let pending_conf: Vec<String> = body
                        .get("pending_confirmations")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let confirmed = body
                        .get("confirmed_action_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let history: Vec<ApprovalHistRow> = body
                        .get("recent_records")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    let req = r.get("request")?;
                                    Some(ApprovalHistRow {
                                        description: req
                                            .get("action_description")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        risk: req
                                            .get("risk_level")
                                            .map(|v| {
                                                v.as_str()
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| v.to_string())
                                            })
                                            .unwrap_or_default(),
                                        decision: r
                                            .get("response")
                                            .and_then(|resp| resp.get("decision"))
                                            .map(|v| {
                                                v.as_str()
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| v.to_string())
                                            })
                                            .unwrap_or_else(|| "未决".to_string()),
                                        session_id: req
                                            .get("session_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        result: r.get("execution_result").and_then(|v| v.as_bool()),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = history.len();
                    let _ = this.update(cx, |this, cx| {
                        this.approval_mode = mode;
                        this.approval_auto = auto;
                        this.approval_timeout = timeout;
                        this.approval_max_pending = max_pending;
                        this.approval_prompt_cmds = prompt_cmds;
                        this.approval_rules = rules;
                        this.pending_confirmations = pending_conf;
                        this.confirmed_action_count = confirmed;
                        this.approval_history = history;
                        this.approvals_page_status = format!("审计记录 {n} 条");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.approvals_page_status = format!("加载审批状态失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载洞察数据（`/stats` + `/usage/stats?days=N` + `/scheduler/status`）。
    ///
    /// 三个请求都失败也只是少一块，不阻塞整页（每个面独立降级）。
    fn load_insights(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let days = self.usage_days;
        cx.spawn(async move |this, cx| {
            let stats_res = fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/stats")).await;
            let usage_res = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/stats?days={days}"
            ))
            .await;
            let sched_res = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/scheduler/status"
            ))
            .await;

            let stats = stats_res.ok().map(|b| InsightStats {
                total_skills_tracked: b
                    .get("total_skills_tracked")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_skill_calls: b
                    .get("total_skill_calls")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_tools_tracked: b
                    .get("total_tools_tracked")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_tool_calls: b
                    .get("total_tool_calls")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_docs_tracked: b
                    .get("total_docs_tracked")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_searches: b
                    .get("total_searches")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            });

            let (usage_total, usage_grouped) = match usage_res {
                Ok(b) => {
                    let total = b.get("total").filter(|v| !v.is_null()).map(|t| UsageTotal {
                        calls: t.get("calls").and_then(|v| v.as_u64()).unwrap_or(0),
                        uncached_input: t
                            .get("uncached_input")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cached_input: t.get("cached_input").and_then(|v| v.as_u64()).unwrap_or(0),
                        completion_tokens: t
                            .get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        total_tokens: t.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        cache_hit_rate: t
                            .get("cache_hit_rate")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0),
                    });
                    let grouped: Vec<UsageGroup> = b
                        .get("grouped")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|g| {
                                    Some(UsageGroup {
                                        group: g.get("group")?.as_str()?.to_string(),
                                        calls: g.get("calls").and_then(|v| v.as_u64()).unwrap_or(0),
                                        total_tokens: g
                                            .get("total_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        cache_hit_rate: g
                                            .get("cache_hit_rate")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (total, grouped)
                }
                Err(_) => (None, Vec::new()),
            };

            let (running, executing, sched_tasks) = match sched_res {
                Ok(b) => {
                    let running = b.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
                    let executing = b
                        .get("executing_task_id")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());
                    let tasks: Vec<SchedStatusRow> = b
                        .get("tasks")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| {
                                    Some(SchedStatusRow {
                                        id: t.get("id")?.as_str()?.to_string(),
                                        name: t
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        interval_secs: t
                                            .get("interval_secs")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        run_count: t
                                            .get("run_count")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        last_run_ago_secs: t
                                            .get("last_run_ago_secs")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        next_due_in_secs: t
                                            .get("next_due_in_secs")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (running, executing, tasks)
                }
                Err(_) => (false, None, Vec::new()),
            };

            let n = sched_tasks.len();
            let _ = this.update(cx, |this, cx| {
                this.insight_stats = stats;
                this.usage_total = usage_total;
                this.usage_grouped = usage_grouped;
                this.scheduler_running = running;
                this.scheduler_executing = executing;
                this.sched_status_tasks = sched_tasks;
                this.insights_status = format!(
                    "调度器{} · {n} 个任务",
                    if running { "运行中" } else { "未装配" }
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载记忆条目（`GET /memory`）。
    ///
    /// 后端已聚合三类来源（`memory/` 事实与决策、`user/` 画像、`agent/patterns` 模式）
    /// 并且只返回叶子：目录只是导航中间节点，内容为空、importance 是默认值，
    /// 直接展示只会误导（这是后端注释里的原话，界面不再二次筛选）。
    fn load_memories(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/memory");
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<MemoryRow> = body
                        .get("memories")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|m| {
                            let meta = m.get("metadata");
                            let text_of = |key: &str| {
                                m.get(key)
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .filter(|s| !s.trim().is_empty())
                            };
                            Some(MemoryRow {
                                uri: m.get("uri")?.as_str()?.to_string(),
                                name: m
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                relative_path: m
                                    .get("relative_path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                importance: meta
                                    .and_then(|v| v.get("importance"))
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.5)
                                    as f32,
                                updated_at: meta
                                    .and_then(|v| v.get("updated_at"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                tags: meta
                                    .and_then(|v| v.get("tags"))
                                    .and_then(|v| v.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|t| t.as_str().map(|s| s.to_string()))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                abstract_text: text_of("abstract"),
                                overview_text: text_of("overview"),
                                detail_text: text_of("detail"),
                            })
                        })
                        .collect();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.memories = rows;
                        this.memories_status = format!("{n} 条记忆（L0/L1/L2 三层）");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.memories_status = format!("加载记忆失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载定时任务列表（`GET /scheduled-tasks`）。
    fn load_sched_tasks(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks");
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<SchedTaskRow> = body
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|t| {
                            Some(SchedTaskRow {
                                id: t.get("id")?.as_str()?.to_string(),
                                name: t
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                interval_secs: t
                                    .get("interval_secs")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0),
                                workspace: t
                                    .get("workspace")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                prompt: t
                                    .get("prompt")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                enabled: t.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                                // RFC3339 字符串（磁盘上的旧 epoch 数字由后端兼容读入）
                                last_run_at: t
                                    .get("last_run_at")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                                next_run_at: t
                                    .get("next_run_at")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                                last_result: t
                                    .get("last_result")
                                    .and_then(|v| v.as_str())
                                    .map(|v| v.to_string()),
                            })
                        })
                        .collect();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.sched_tasks = rows;
                        this.sched_status = format!("{n} 个定时任务");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.sched_status = format!("加载定时任务失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 创建定时任务（`POST /scheduled-tasks`）。
    ///
    /// 间隔取预设；预设为 `0`（自定义）时读秒数输入框，最小 60 秒
    /// （与 Web 版 `min={60}` 同口径 —— 更短的间隔没有意义，只会刷屏）。
    fn create_sched_task(&mut self, cx: &mut Context<Self>) {
        let name = self.sched_name.read(cx).value().trim().to_string();
        let workspace = self.sched_workspace.read(cx).value().trim().to_string();
        let prompt = self.sched_prompt.read(cx).value().trim().to_string();
        let secs = if self.sched_preset == 0 {
            self.sched_custom_secs
                .read(cx)
                .value()
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
        } else {
            self.sched_preset
        };
        if name.is_empty() || prompt.is_empty() || secs < 60 {
            self.sched_status = "名称与指令不能为空，间隔至少 60 秒".to_string();
            cx.notify();
            return;
        }
        self.create_sched_task_direct(name, secs, workspace, prompt, cx);
    }

    /// 真正发创建请求（UI 表单与自动化 `sched_create` **共用这一份**）。
    ///
    /// 分开的理由同 ADR-052 D8：自动化若另写一份请求构造，两条路径就分叉了。
    /// 校验也放在调用方之前各做一次，这里只信任入参。
    fn create_sched_task_direct(
        &mut self,
        name: String,
        secs: u64,
        workspace: String,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks");
            let body = serde_json::json!({
                "name": name,
                "interval_secs": secs,
                "workspace": workspace,
                "prompt": prompt,
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.sched_status = "定时任务已创建".to_string();
                        this.sched_form_open = false;
                        this.load_sched_tasks(cx);
                    }
                    Err(e) => this.sched_status = format!("创建失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 删除定时任务（`DELETE /scheduled-tasks/{id}`）。
    fn delete_sched_task(&mut self, id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks/{}",
                url_encode(&id)
            );
            let res = delete_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.sched_status = "定时任务已删除".to_string();
                        this.load_sched_tasks(cx);
                    }
                    Err(e) => this.sched_status = format!("删除失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载学习方法论技能（`GET /skills` + `GET /skills/stats`）。
    ///
    /// 只保留 `category == "custom"`：其余类是内置能力（文件/命令/网络），
    /// 那些由工具面板覆盖（与 Web 版 `SkillsPanel` 同口径）。
    /// 统计失败不阻塞面板（只影响右上统计条与列表里的调用次数）。
    fn load_skills(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let list_url = format!("http://{SERVER_HOST}:{port}/api/v1/skills");
            let stats_url = format!("http://{SERVER_HOST}:{port}/api/v1/skills/stats");
            let list_res = fetch_json(list_url).await;
            let stats_res = fetch_json(stats_url).await;

            let rows: Vec<SkillRow> = list_res
                .ok()
                .filter(|body| body.get("skills").is_some())
                .map(|body| {
                    body.get("skills")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap_or_default()
                .iter()
                .filter(|s| s.get("category").and_then(|v| v.as_str()) == Some("custom"))
                .filter_map(|s| {
                    Some(SkillRow {
                        id: s.get("id")?.as_str()?.to_string(),
                        name: s
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        description: s
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        version: s
                            .get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        updated_at: s
                            .get("updated_at")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string()),
                    })
                })
                .collect();

            let (stats, total_calls) = match stats_res {
                Ok(body) => {
                    let total = body
                        .get("total_calls")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let stats: Vec<SkillStatRow> = body
                        .get("skills")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| {
                                    Some(SkillStatRow {
                                        skill_id: s.get("skill_id")?.as_str()?.to_string(),
                                        total_calls: s
                                            .get("total_calls")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        success_calls: s
                                            .get("success_calls")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        avg_time_ms: s
                                            .get("avg_time_ms")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (stats, total)
                }
                Err(_) => (Vec::new(), 0),
            };

            let n = rows.len();
            let _ = this.update(cx, |this, cx| {
                this.skills = rows;
                this.skill_stats = stats;
                this.skills_total_calls = total_calls;
                this.skills_status = format!("{n} 个方法论技能");
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉选中技能的完整 markdown 内容（`GET /skills/{id}`）。
    fn load_skill_detail(&self, id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/skills/{}",
                url_encode(&id)
            );
            let res = fetch_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(body) => {
                        this.skill_content = body
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string());
                        this.skills_status = "已加载技能内容".to_string();
                    }
                    Err(e) => {
                        this.skill_content = None;
                        this.skills_status = format!("加载技能详情失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载工具目录（`GET /api/v1/tools`：与 LLM 收到的 tools 列表同源）。
    fn load_tools(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/tools");
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<ToolRow> = body
                        .get("tools")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| {
                                    Some(ToolRow {
                                        name: t.get("name")?.as_str()?.to_string(),
                                        description: t
                                            .get("description")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        parameters: t
                                            .get("parameters")
                                            .cloned()
                                            .unwrap_or(serde_json::Value::Null),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = this.update(cx, |this, cx| {
                        this.tools = rows;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = format!("加载工具失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn load_roles(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            match fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/roles")).await {
                Ok(body) => {
                    let rows: Vec<RoleRow> = body
                        .get("roles")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|r| RoleRow {
                            name: r
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            source: r
                                .get("source")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            status: r
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            version: r.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                            purpose: r
                                .get("purpose")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            tool_count: r.get("tool_count").and_then(|v| v.as_u64()),
                            model: r
                                .get("model")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            usage: r.get("usage").map(|u| {
                                (
                                    u.get("calls").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                                    u.get("success_rate")
                                        .and_then(|v| v.as_f64())
                                        .unwrap_or(0.0) as f32,
                                )
                            }),
                        })
                        .collect();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.roles = rows;
                        this.roles_status = format!("{n} 个子智能体");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_status = format!("加载子智能体失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载子智能体统计（`GET /roles/stats`）：页面顶部那条汇总。
    fn load_roles_stats(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            if let Ok(body) =
                fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/roles/stats")).await
            {
                let stats = RolesStats {
                    total_calls: body
                        .get("total_calls")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    success_rate: body
                        .get("success_rate")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0) as f32,
                    by_task_type: body
                        .get("by_task_type")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|pair| {
                                    let p = pair.as_array()?;
                                    Some((
                                        p.first()?.as_str()?.to_string(),
                                        p.get(1)?.as_u64()? as usize,
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    by_role: body
                        .get("by_role")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    Some((
                                        r.get("name")?.as_str()?.to_string(),
                                        r.get("calls")?.as_u64()? as u32,
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let _ = this.update(cx, |this, cx| {
                    this.roles_stats = stats;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 回退内置种子（`POST /roles/{name}/reset`）：成功后刷新列表 / 统计 / 详情。
    ///
    /// 服务端语义是「下次会话边界生效」——已在跑的会话仍用旧定义，所以提示里
    /// 明说这一点，避免用户以为立刻变了。
    fn confirm_reset_role(&mut self, name: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/roles/{name}/reset");
            match post_json(url, serde_json::json!({})).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg =
                            Some("已回退内置种子（下次会话边界生效）".to_string());
                        this.load_roles(cx);
                        this.load_roles_stats(cx);
                        this.load_role_detail(name.clone(), cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg = Some(format!("操作失败：{e}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 退役删除角色（`DELETE /roles/{name}`）：成功后清空选中并刷新列表。
    fn confirm_delete_role(&mut self, name: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/roles/{name}");
            match delete_json(url).await {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg = Some("角色已退役删除".to_string());
                        if this.roles_selected.as_deref() == Some(name.as_str()) {
                            this.roles_selected = None;
                            this.role_detail = None;
                        }
                        this.load_roles(cx);
                        this.load_roles_stats(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg = Some(format!("操作失败：{e}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载工作区目录树（`GET /workspace/tree?depth=1&session_id=`）。
    ///
    /// 单层浏览（点目录进、点「上级」出）：树的可展开形态要维护每个节点的
    /// 独立状态与嵌套请求，先按单层落，够用且不易错。
    fn load_workspace_tree(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url = format!("http://{SERVER_HOST}:{port}/api/v1/workspace/tree?depth=1");
            if !path.is_empty() {
                url.push_str(&format!("&path={}", url_encode(&path)));
            }
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let root = body
                        .get("root")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut entries: Vec<WsEntry> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some(WsEntry {
                                        name: e.get("name")?.as_str()?.to_string(),
                                        is_dir: e.get("type").and_then(|v| v.as_str())
                                            == Some("dir"),
                                        path: e
                                            .get("path")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        size: e.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    // 目录在前、文件在后，各自按名字排序（与常见文件浏览器一致）
                    entries.sort_by(|a, b| {
                        b.is_dir
                            .cmp(&a.is_dir)
                            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    });
                    let n = entries.len();
                    let _ = this.update(cx, |this, cx| {
                        this.ws_root = root;
                        this.ws_path = path;
                        this.ws_entries = entries;
                        this.ws_status = format!("{n} 项");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_status = format!("读取目录失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 读取工作区文件（`GET /workspace/read?limit=2000&session_id=`）。
    fn load_workspace_file(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/workspace/read?limit=2000&path={}",
                url_encode(&path)
            );
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let truncated = body
                        .get("truncated")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let total = body.get("total_lines").and_then(|v| v.as_u64());
                    let raw = body
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let text = strip_hashline(raw);
                    let status = if raw.is_empty() {
                        "（空文件或二进制，无法以文本显示）".to_string()
                    } else if truncated {
                        format!(
                            "已截断：仅显示前 2000 行（共 {} 行）",
                            total
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "?".to_string())
                        )
                    } else {
                        format!(
                            "{} 行",
                            total
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| text.lines().count().to_string())
                        )
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.ws_file = Some((path, text));
                        this.ws_status = status;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_status = format!("读取文件失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 拉工作区改动文件列表（`GET /workspace/diff?base=snapshot`，不带 path）。
    ///
    /// 这是「AI 到底改了什么」的入口。**快照是按消息存的**（后端要求
    /// `message_id` 或 `index`，缺了直接 400）—— `index=0` = 会话起点那一刻的
    /// 工作区快照，也就是「本次会话从头到现在改了什么」。
    fn load_workspace_diff_list(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url =
                format!("http://{SERVER_HOST}:{port}/api/v1/workspace/diff?base=snapshot&index=0");
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let files: Vec<WsDiffFile> = body
                        .get("files")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|f| {
                                    Some(WsDiffFile {
                                        path: f.get("path")?.as_str()?.to_string(),
                                        status: f
                                            .get("status")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("modified")
                                            .to_string(),
                                        old_lines: f
                                            .get("old_lines")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        new_lines: f
                                            .get("new_lines")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = files.len();
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff_files = files;
                        this.ws_diff_status = if n == 0 {
                            "工作区无改动（相对本次会话快照）".to_string()
                        } else {
                            format!("{n} 个文件有改动")
                        };
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff_status = format!("读取改动列表失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 拉单个文件的 unified diff（`GET /workspace/diff?base=snapshot&index=0&path=`）。
    fn load_workspace_diff(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/workspace/diff?base=snapshot&index=0&path={}",
                url_encode(&path)
            );
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let unified = body
                        .get("unified")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let status = body
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("modified")
                        .to_string();
                    let text = if unified.is_empty() {
                        match status.as_str() {
                            "binary" => "（二进制文件，无文本 diff）".to_string(),
                            "unchanged" => "（无改动）".to_string(),
                            _ => "（服务端未返回 unified diff）".to_string(),
                        }
                    } else {
                        unified
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff = Some((path, text));
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff_status = format!("读取 diff 失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载角色详情（`GET /roles/{name}`）。
    fn load_role_detail(&self, name: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/roles/{name}");
            if let Ok(body) = fetch_json(url).await {
                let detail = RoleDetail {
                    system_prompt: body
                        .get("system_prompt")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tools: body
                        .get("tools")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let _ = this.update(cx, |this, cx| {
                    this.role_detail = Some(detail);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 该会话正在跑的后台任务数（`pending` / `running` 计）。
    fn running_tasks_of(&self, session_id: &str) -> usize {
        self.all_tasks
            .iter()
            .filter(|t| {
                t.parent_session_id == session_id
                    && (t.status == "pending" || t.status == "running")
            })
            .count()
    }

    /// 折叠 / 展开一个工作区分组。
    fn toggle_group(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed_groups.remove(&key) {
            self.collapsed_groups.insert(key);
        }
        self.rebuild_session_rows(cx);
    }

    /// 组内「查看更多」：再展开 `SESSIONS_LOAD_STEP` 条。
    fn expand_group(&mut self, key: String, cx: &mut Context<Self>) {
        *self.expanded_groups.entry(key).or_insert(0) += SESSIONS_LOAD_STEP;
        self.rebuild_session_rows(cx);
    }

    /// 新建会话：清空选中与消息。真正的会话记录由服务端在首条消息时按本地
    /// 生成的 id 创建（事件按 `session_id` 路由，id 必须发起前就确定）。
    fn start_new_session(&mut self, cx: &mut Context<Self>) {
        self.current_session = None;
        self.streaming = false;
        self.session_model = None;
        self.model_menu_open = false;
        self.thinking = None;
        self.thinking_menu_open = false;
        self.ctx_live = None;
        self.ctx_window_live = 0;
        self.ctx_panel_open = false;
        self.ctx_compress = None;
        // 待发送附件是按会话的临时草稿：切走即丢弃（避免误发到别的会话）
        self.composer_images.clear();
        self.messages = Rc::new(Vec::new());
        self.msg_list_state = GpuiListState::new(0, ListAlignment::Bottom, px(120.));
        self.message_total = 0;
        self.current_title = "新会话（发送首条消息后创建）".to_string();
        self.rebuild_session_rows(cx);
    }

    /// 重建左栏行（分组 / 会话 / 查看更多）。
    ///
    /// 排序对齐 `gui-vite/src/components/session/SessionList.tsx` 的 `sortGroups`：
    /// 1.「最近活跃」段：24h 内有活动的工作区，按组内最近时间倒序，最多 3 个；
    /// 2.「其余」段：按路径字母序（稳定段，位置不随使用漂移）；
    /// 3. 默认组（`""`）始终垫底（即使没有未绑定会话，也要能在这儿新建）。
    ///
    /// 组内折叠：初始 3 条 + 「查看更多」累加；**当前会话始终可见**（从别处切进
    /// 深处会话时不会被折叠藏住，同 Tauri 版）。
    fn rebuild_session_rows(&mut self, cx: &mut Context<Self>) {
        use std::collections::{BTreeMap, HashSet};
        let mut groups: BTreeMap<String, Vec<&SessionRow>> = BTreeMap::new();
        groups.entry(String::new()).or_default();
        for s in &self.sessions_data {
            groups
                .entry(s.working_directory.clone())
                .or_default()
                .push(s);
        }
        for list in groups.values_mut() {
            list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        }
        let now = now_unix_secs();
        let last_activity = |list: &[&SessionRow]| -> i64 {
            list.iter()
                .filter_map(|s| parse_rfc3339_secs(&s.updated_at))
                .max()
                .unwrap_or(0)
        };
        let mut active: Vec<(String, Vec<&SessionRow>)> = groups
            .iter()
            .filter(|(k, l)| !k.is_empty() && last_activity(l) >= now - ACTIVE_GROUP_WINDOW_SECS)
            .map(|(k, l)| (k.clone(), l.clone()))
            .collect();
        active.sort_by(|a, b| last_activity(&b.1).cmp(&last_activity(&a.1)));
        active.truncate(ACTIVE_GROUP_LIMIT);
        let active_keys: HashSet<&str> = active.iter().map(|(k, _)| k.as_str()).collect();
        let mut rest: Vec<(String, Vec<&SessionRow>)> = groups
            .iter()
            .filter(|(k, _)| !k.is_empty() && !active_keys.contains(k.as_str()))
            .map(|(k, l)| (k.clone(), l.clone()))
            .collect();
        rest.sort_by(|a, b| a.0.cmp(&b.0));
        let mut ordered = active;
        ordered.extend(rest);
        ordered.extend(
            groups
                .iter()
                .filter(|(k, _)| k.is_empty())
                .map(|(k, l)| (k.clone(), l.clone())),
        );

        let mut rows: Vec<SessionListRow> = Vec::new();
        for (key, list) in ordered {
            let collapsed = self.collapsed_groups.contains(&key);
            let count = list.len();
            rows.push(SessionListRow::Group {
                label: group_label(&key),
                key: key.clone(),
                count,
                collapsed,
            });
            if collapsed {
                continue;
            }
            let base =
                SESSIONS_INITIAL_VISIBLE + self.expanded_groups.get(&key).copied().unwrap_or(0);
            let visible = match self
                .current_session
                .as_deref()
                .and_then(|id| list.iter().position(|s| s.id == id))
            {
                Some(pos) if pos >= base => pos + 1,
                _ => base,
            }
            .min(count);
            for s in list.iter().take(visible) {
                rows.push(SessionListRow::Session {
                    id: s.id.clone(),
                    title: s.title.clone(),
                    meta: format_relative(&s.updated_at),
                    tasks: self.running_tasks_of(&s.id),
                    selected: self.current_session.as_deref() == Some(s.id.as_str()),
                });
            }
            let hidden = count.saturating_sub(visible);
            if hidden > 0 {
                rows.push(SessionListRow::More { key, hidden });
            }
        }
        self.session_rows = rows;
        cx.notify();
    }

    /// 加载模型服务列表（`GET /api/v1/config/models`）。
    ///
    /// 只取展示与切换需要的字段：provider / 模型名 / 能力标签 / 上下文窗口 /
    /// 是否为当前 chat 首选。`preferences.chat` 是 `{model, provider}`，provider
    /// 可能缺省（同名模型只注册在一个 provider 上时）。
    fn load_models(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/models");
            match fetch_json(url).await {
                Ok(body) => {
                    let pref = body.get("preferences").and_then(|p| p.get("chat"));
                    let chat_model = pref
                        .and_then(|c| c.get("model"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let chat_provider = pref
                        .and_then(|c| c.get("provider"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut rows: Vec<ModelRow> = Vec::new();
                    if let Some(models) = body.get("models").and_then(|v| v.as_array()) {
                        for m in models {
                            let name = m
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let provider = m
                                .get("provider")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let capabilities = m
                                .get("capabilities")
                                .and_then(|v| v.as_array())
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str())
                                        .collect::<Vec<_>>()
                                        .join("/")
                                })
                                .unwrap_or_default();
                            let context = m.get("context_length").and_then(|v| v.as_u64());
                            // 该模型声明的思考档位（空 = 不支持思考 → 选择器隐藏）
                            let reasoning_efforts = m
                                .get("reasoning_efforts")
                                .and_then(|v| v.as_array())
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str())
                                        .map(|x| x.to_string())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            let is_chat = name == chat_model
                                && (chat_provider.is_empty() || provider == chat_provider);
                            rows.push(ModelRow {
                                provider,
                                name,
                                capabilities,
                                context,
                                reasoning_efforts,
                                is_chat,
                            });
                        }
                    }
                    let n = rows.len();
                    // 再拉一次整份配置：providers 的 endpoint / api_key 只在 `/config`
                    // 里（`/config/models` 只给 name/endpoint/enabled/model_count）。
                    // 编辑回填需要它们，而回填只能在同步点击处理器里做（要 `&mut Window`）。
                    let cfg_res =
                        fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/config")).await;
                    let mut providers: Vec<ProviderRow> = Vec::new();
                    if let Ok(body) = cfg_res {
                        if let Some(arr) = body
                            .get("config")
                            .and_then(|c| c.get("models"))
                            .and_then(|m| m.get("providers"))
                            .and_then(|p| p.as_array())
                        {
                            for p in arr {
                                providers.push(ProviderRow {
                                    name: p
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    endpoint: p
                                        .get("endpoint")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    api_key: p
                                        .get("api_key")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    model_count: p
                                        .get("models")
                                        .and_then(|v| v.as_array())
                                        .map(|a| a.len())
                                        .unwrap_or(0),
                                });
                            }
                        }
                    }
                    let _ = this.update(cx, |this, cx| {
                        this.models = rows;
                        this.providers = providers;
                        this.settings_status = format!("模型 {n} 个");
                        this.settings_error = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("加载模型列表失败：{e}");
                        this.settings_error = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 切换默认聊天模型（`POST /api/v1/config/models/switch`，持久化 preferences.chat）。
    fn switch_chat_model(&mut self, provider: String, model: String, cx: &mut Context<Self>) {
        let port = self.port;
        self.settings_status = format!("正在切换到 {model}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/models/switch");
            // capability 用 kebab-case（core 的 ModelCapability 序列化口径）
            let body = serde_json::json!({
                "model": model,
                "capability": "chat",
                "provider": provider,
            });
            match post_json(url, body).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("已把 {model} 设为聊天模型");
                        this.settings_error = false;
                        this.load_models(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("切换失败：{e}");
                        this.settings_error = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 全局一级导航栏（64px 图标 rail）。
    ///
    /// 与 Tauri 版 `Sidebar.tsx` 同构：**固定宽度、不做展开/收起**——会话列表在会话页
    /// 左栏，不在 rail 里；rail 只放 11 个一级入口。未实现的入口点进去是占位页。
    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rail = div()
            .w(px(colors::RAIL_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py_3()
            .bg(rgb(colors::BG_SECONDARY()))
            .border_r_1()
            .border_color(rgb(colors::BORDER()))
            // 品牌块（Tauri 版是「天」字方块）
            .child(
                div()
                    .w(px(36.))
                    .h(px(36.))
                    .mb_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(rgb(colors::BG_TERTIARY()))
                    .text_color(rgb(colors::ACCENT()))
                    .child("天"),
            );
        for (ix, entry) in NAV_ITEMS.iter().enumerate() {
            let (icon, _label, view) = *entry;
            let tint = if self.view == view {
                colors::ACCENT()
            } else {
                colors::TEXT_SECONDARY()
            };
            let mut item = div()
                .id(("nav", ix))
                .w(px(40.))
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .text_sm();
            if self.view == view {
                item = item.bg(rgb(colors::BG_TERTIARY())).text_color(rgb(tint));
            } else {
                item = item.text_color(rgb(tint));
            }
            // Lucide SVG（stroke 用 currentColor → 由 text_color 上色，可随选中态换色）
            rail = rail.child(
                item.child(svg().path(icon.path()).size(px(18.)).text_color(rgb(tint)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.view = view;
                        if view == View::Settings {
                            this.load_models(cx);
                        }
                        // 数据已在启动时预加载：只在空列表（首次失败/未拉到）时补拉，
                        // 避免每次切页都发请求、页面先空一帧再填
                        if view == View::Roles && this.roles.is_empty() {
                            this.load_roles(cx);
                            this.load_roles_stats(cx);
                        }
                        // 文件页首次进入才拉目录树（工作区根 = 会话绑定目录）
                        if view == View::Workspace && this.ws_root.is_empty() {
                            this.load_workspace_tree(String::new(), cx);
                        }
                        cx.notify();
                    })),
            );
        }
        rail
    }

    /// 切页：**侧栏点击与自动化 `switch_tab` 共用这一份**。
    ///
    /// 「首次进入才补拉」的逻辑必须住在这里。早先它内联在侧栏的 `on_click` 闭包
    /// 里，于是点侧栏会拉数据、而自动化 `switch_tab` 只改 `view` 不拉 —— 两条
    /// 路径分叉，验证结论随即失真（ADR-052 D8 的实例，实测踩过：文件页树一直空）。
    /// 切换外观：改全局色板 + 同步组件库 `ThemeMode` + 持久化 + 重绘。
    ///
    /// 组件库控件（`Input` / `Textarea` / 按钮）走它自己的 `Theme`，不同步切就会
    /// 出现「天演是羊皮纸、输入框还是深灰」的割裂。
    ///
    /// `window` 为 `None` 时（自动化端口调用）只改全局主题，不碰窗口局部状态。
    fn apply_theme_with(
        &mut self,
        kind: ThemeKind,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        set_theme(kind);
        Theme::change(
            match kind {
                ThemeKind::Parchment => ThemeMode::Light,
                _ => ThemeMode::Dark,
            },
            window,
            cx,
        );
        save_theme_pref(kind);
        cx.notify();
    }

    fn switch_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.view = view;
        if view == View::Settings {
            self.load_models(cx);
        }
        // 数据已在启动时预加载：只在空列表（首次失败/未拉到）时补拉，
        // 避免每次切页都发请求、页面先空一帧再填
        if view == View::Roles && self.roles.is_empty() {
            self.load_roles(cx);
            self.load_roles_stats(cx);
        }
        // 文件页首次进入才拉目录树（工作区根 = 会话绑定目录）
        if view == View::Workspace && self.ws_root.is_empty() {
            self.load_workspace_tree(String::new(), cx);
        }
        // 工具页首次进入才拉工具目录（与 roles 同一懒加载口径）
        if view == View::Tools && self.tools.is_empty() {
            self.load_tools(cx);
        }
        // 技能页首次进入才拉（列表 + 使用统计）
        if view == View::Skills && self.skills.is_empty() {
            self.load_skills(cx);
        }
        // 定时任务页每次进入都刷新：agent 可能刚用 schedule_task 建过任务
        if view == View::Scheduled {
            self.load_sched_tasks(cx);
        }
        // 记忆页每次进入都刷新：演化任务会在会话结束后写入
        if view == View::Memory {
            self.load_memories(cx);
        }
        // 洞察页每次进入都刷新（用量/调度状态随时在变）
        if view == View::Insights {
            self.load_insights(cx);
        }
        // 审批页每次进入都刷新（挂起请求随时可能出现）
        if view == View::Approval {
            self.load_approval_page(cx);
        }
        // 知识页首次进入才拉根目录（后续导航自己发请求）
        if view == View::Knowledge && self.kb_entries.is_empty() {
            self.load_kb_entries(String::new(), cx);
        }
        cx.notify();
    }

    /// 输入卡片底部显示的当前 chat 模型（`模型 · provider`）。
    ///
    /// 口径与 Tauri 版 `ModelSelector` 一致：未选中时回退到 `preferences.chat`
    /// （`load_models` 已把它标成 `is_chat`），再没有就显示占位。
    fn current_model_label(&self) -> String {
        match self.current_model_pair() {
            Some((provider, model)) => format!("{model} · {provider}"),
            None => "未选择模型".to_string(),
        }
    }

    /// 当前会话生效的 `(provider, model)`：会话选择优先，回退默认 chat 模型。
    fn current_model_pair(&self) -> Option<(String, String)> {
        if let Some(pair) = &self.session_model {
            return Some(pair.clone());
        }
        let m = self.models.iter().find(|m| m.is_chat)?;
        Some((m.provider.clone(), m.name.clone()))
    }

    /// 当前 chat 模型行（思考档位 / 上下文窗口都从这里取）。
    fn current_chat_model(&self) -> Option<&ModelRow> {
        if let Some((provider, model)) = &self.session_model {
            if let Some(row) = self
                .models
                .iter()
                .find(|m| &m.provider == provider && &m.name == model)
            {
                return Some(row);
            }
        }
        self.models.iter().find(|m| m.is_chat)
    }

    /// 思考强度候选档位；`None` = 当前模型不支持思考 → 选择器整体隐藏。
    ///
    /// 顺序 = 「default」（值 `off`）+ 模型声明的档位（去掉重复的 `off`）。
    /// 档位值**原样展示不翻译**——厂商可能给 low / high / max / think 等任意值。
    fn thinking_options(&self) -> Option<Vec<String>> {
        let declared: Vec<String> = self
            .current_chat_model()?
            .reasoning_efforts
            .iter()
            .filter(|v| v.as_str() != "off")
            .cloned()
            .collect();
        if declared.is_empty() {
            return None;
        }
        let mut out = vec!["off".to_string()];
        out.extend(declared);
        Some(out)
    }

    /// 思考强度展示名：`off` 如实显示为「default」
    /// （不附加思考参数，是否思考由模型决定——思考模型缺省仍会输出推理）。
    fn thinking_label(&self) -> String {
        self.thinking
            .clone()
            .unwrap_or_else(|| "default".to_string())
    }

    /// 选择思考强度：写**当前会话**的模型选择（与模型选择同一个对象，
    /// `PUT /sessions/{id}/model` 带 `thinking`）。未创建会话时只记本地，
    /// 首条消息的 `ChatRequest` 会带上并固化。
    fn select_thinking(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        self.thinking_menu_open = false;
        self.model_menu_open = false;
        self.thinking = effort.clone();
        let Some(sid) = self.current_session.clone() else {
            cx.notify();
            return;
        };
        let Some((provider, model)) = self.current_model_pair() else {
            cx.notify();
            return;
        };
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/model");
            let mut body = serde_json::json!({ "provider": provider, "model": model });
            // 缺省 = off（不附加思考参数）
            if let Some(e) = effort {
                body["thinking"] = serde_json::Value::String(e);
            }
            let res = put_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("思考强度已设置：{}", this.thinking_label()),
                    Err(e) => format!("设置思考强度失败：{e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉会话级模型选择（`GET /sessions/{id}/model`）→ 恢复模型与思考强度。
    ///
    /// 不恢复的话，切到「设过 high」的会话会显示 default——用户一改就把会话里
    /// 的档位覆盖掉（显示与实际不符，比不显示更糟）。
    fn load_session_model(&self, session_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{session_id}/model");
            let Ok(body) = fetch_json(url).await else {
                return;
            };
            let sel = body.get("selection");
            let provider = sel
                .and_then(|s| s.get("provider"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let model = sel
                .and_then(|s| s.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let thinking = sel
                .and_then(|s| s.get("thinking"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let _ = this.update(cx, |this, cx| {
                // 迟到响应不覆盖：请求在飞时会话可能已切走
                if this.current_session.as_deref() != Some(session_id.as_str()) {
                    return;
                }
                if let (Some(p), Some(m)) = (provider, model) {
                    this.session_model = Some((p, m));
                }
                this.thinking = thinking;
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉取本会话的后台任务（`GET /api/v1/tasks`，按 `parent_session_id` 归属过滤）。
    ///
    /// Tauri 版靠「3s 轮询 + `task_status` 事件唤醒」双保险；这里先做轮询
    /// （事件唤醒在 `handle_event` 里补，见 `task_status` 分支）。
    /// 失败**静默保留旧值**——面板是观测面，不能因为一次网络抖动把已有列表清空。
    fn load_tasks(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/tasks");
            if let Ok(body) = fetch_json(url).await {
                let arr = body.as_array().cloned().unwrap_or_default();
                // 全量任务一次拉回来：面板只显示当前会话的，左栏会话角标要按
                // `parent_session_id` 聚合**所有**会话（同一份轮询供两处用，
                // 与 Tauri 版把 `backgroundTasks` 放进 store 同口径）。
                let all: Vec<TaskRow> = arr
                    .iter()
                    .map(|t| TaskRow {
                        id: t
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        kind: t
                            .get("kind")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .to_string(),
                        status: t
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .to_string(),
                        description: t
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        parent_session_id: t
                            .get("parent_session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        output_tail: t
                            .get("output_tail")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        error: t
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    })
                    .collect();
                let rows: Vec<TaskRow> = all
                    .iter()
                    .filter(|t| {
                        session
                            .as_deref()
                            .map(|s| s == t.parent_session_id)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();
                let _ = this.update(cx, |this, cx| {
                    this.tasks = rows;
                    this.all_tasks = all;
                    this.rebuild_session_rows(cx);
                });
            }
        })
        .detach();
    }

    /// 会话后台任务停靠条（右侧 dock）。
    ///
    /// 对齐 Tauri 版 `AgentTasksPanel`：无任务时可折叠成竖条；活跃任务在上
    /// （顺序由后端返回决定）、终态保留展示。**取消按钮暂未接**（`POST /tasks/{id}/cancel`）。
    fn render_tasks_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.tasks_collapsed {
            return div()
                .id("tasks-expand")
                .w(px(36.))
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .py_2()
                .border_l_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.tasks_collapsed = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child("任务"),
                );
        }
        let mut panel = div()
            .w(px(300.))
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(format!("后台任务 {}", self.tasks.len())),
                    )
                    .child(
                        div()
                            .id("tasks-collapse")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("⟩")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.tasks_collapsed = true;
                                cx.notify();
                            })),
                    ),
            );
        if self.tasks.is_empty() {
            panel = panel.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("本会话暂无后台任务"),
            );
        }
        // 任务卡片（对齐 Tauri 版 `AgentTasksPanel`）：头 = 箭头 + 状态 + 类型徽章 +
        // 描述 + 状态文案 + 取消；展开体 = 终端输出尾部 / 子代理消息流。
        let mut cards: Vec<AnyElement> = Vec::new();
        for (ix, t) in self.tasks.iter().enumerate() {
            let open = self.expanded_task.as_deref() == Some(t.id.as_str());
            let terminal = matches!(
                t.status.as_str(),
                "completed" | "success" | "failed" | "cancelled" | "canceled"
            );
            let (glyph, color) = match t.status.as_str() {
                "running" | "pending" => ("●", colors::ACCENT()),
                "completed" | "success" => ("✓", colors::SUCCESS()),
                "failed" | "cancelled" | "canceled" => ("✗", colors::ERROR()),
                _ => ("○", colors::TEXT_TERTIARY()),
            };
            // 失败要带原因（截断），与 Tauri 版 `statusLabel` 同口径
            let status_label: String = match t.status.as_str() {
                "pending" => "等待中".to_string(),
                "running" => "运行中".to_string(),
                "completed" | "success" => "已完成".to_string(),
                "cancelled" | "canceled" => "已取消".to_string(),
                "failed" if !t.error.is_empty() => {
                    format!("失败（{}）", ellipsis(&t.error, 20))
                }
                "failed" => "失败".to_string(),
                other => other.to_string(),
            };
            let is_command = t.kind == "command";
            let kind_label = if is_command { "终端" } else { "委托" };
            let kind_color = if is_command {
                colors::TEXT_TERTIARY()
            } else {
                colors::ACCENT()
            };
            let chevron = if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            let mut head = div()
                .id(("task-head", ix))
                .px_2()
                .py_1()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(
                    svg()
                        .path(chevron.path())
                        .size(px(12.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(div().text_xs().text_color(rgb(color)).child(glyph))
                .child(
                    div()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(kind_color))
                        .text_size(px(10.))
                        .text_color(rgb(kind_color))
                        .child(kind_label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child(ellipsis(&t.description, 26)),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(if t.status == "failed" {
                            colors::ERROR()
                        } else {
                            colors::TEXT_TERTIARY()
                        }))
                        .child(status_label),
                );
            // 非终态才给「取消」；点它不能顺带把卡片展开（stop_propagation）
            if !terminal {
                let cancel_id = t.id.clone();
                head = head.child(
                    div()
                        .id(("task-cancel", ix))
                        .px_1()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .cursor_pointer()
                        .child("取消")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.cancel_task(cancel_id.clone(), cx);
                        })),
                );
            }
            let open_id = t.id.clone();
            let head = head.on_click(cx.listener(move |this, _, _, cx| {
                let id = open_id.clone();
                if this.expanded_task.as_deref() == Some(id.as_str()) {
                    this.expanded_task = None;
                } else {
                    this.expanded_task = Some(id.clone());
                    // 委托任务的子代理消息按需拉一次并缓存（task_id 即子会话 id）
                    if this.task_messages.get(&id).is_none() {
                        this.load_task_messages(id.clone(), cx);
                    }
                }
                cx.notify();
            }));
            let mut card = div()
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(rgb(if t.status == "failed" {
                    colors::ERROR()
                } else {
                    colors::BORDER()
                }))
                .bg(rgb(colors::BG_PRIMARY()))
                .overflow_hidden()
                .child(head);
            if open {
                let mut body = div()
                    .border_t_1()
                    .border_color(rgb(colors::BORDER()))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1();
                // 失败任务：展开体顶部给完整原因（头部只放截断版）
                if t.status == "failed" && !t.error.is_empty() {
                    body = body.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::ERROR()))
                            .child(t.error.clone()),
                    );
                }
                if is_command {
                    let text = if t.output_tail.is_empty() {
                        "（暂无输出）".to_string()
                    } else {
                        t.output_tail.clone()
                    };
                    body = body.child(
                        div()
                            .max_h(px(200.))
                            .overflow_hidden()
                            .rounded_md()
                            .bg(rgb(0x0d1117))
                            .p_2()
                            .text_size(px(11.))
                            .text_color(rgb(0xc9d1d9))
                            .child(text),
                    );
                } else {
                    // 子代理活动流：紧凑单行投影（思考 / 输出 / 工具），
                    // 不是消息气泡 —— 与 Web 版 `DelegateActivity` 同构
                    body = body.child(self.render_delegate_activity(&t.id, !terminal, cx));
                }
                card = card.child(body);
            }
            cards.push(card.into_any_element());
        }
        panel
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .children(cards)
                    .overflow_y_scrollbar(),
            )
            .id("tasks-panel")
    }

    /// 取消一个后台任务（`POST /tasks/{id}/cancel`），随后刷新任务列表。
    ///
    /// 失败静默：任务可能已自行结束，下一次轮询会把真实状态带回来
    /// （与 Web 版「取消失败不弹错、等轮询」同口径）。
    fn cancel_task(&mut self, task_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/tasks/{task_id}/cancel");
            let _ = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| this.load_tasks(cx));
        })
        .detach();
    }

    /// 拉子代理任务的消息（展开委托任务时用）。
    ///
    /// 子代理是**带父会话引用的独立会话**（ADR-030：`task_id` 即 `session_id`），
    /// 所以直接读它的消息接口即可，不需要另开一条流。
    fn load_task_messages(&self, task_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url =
                format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{task_id}/messages?limit=20");
            if let Ok(body) = fetch_json(url).await {
                let arr = body
                    .get("messages")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                // 与主对话流同一套结构化解析：子代理的活动流要靠 segments 区分
                // 思考 / 输出 / 工具，纯文本摊平后这些结构就没了（此前正是如此）
                let rows: Vec<MsgRow> = arr
                    .iter()
                    .map(|m| {
                        let (segs, tool_calls) = parse_segments(m);
                        MsgRow {
                            id: m
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            role: m
                                .get("role")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?")
                                .to_string(),
                            text: segments_text(m).into(),
                            segs,
                            tool_calls,
                            compression: false,
                            usage: parse_msg_usage(m),
                            images: parse_msg_images(m),
                            ts: parse_msg_ts(m),
                        }
                    })
                    .collect();
                let _ = this.update(cx, |this, cx| {
                    this.task_messages.insert(task_id, rows);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 提交追问回答（`POST /api/v1/chat/answer`）。
    ///
    /// `answers` 是 `ask_user` 工具的返回值（直接回给模型）：
    /// `{ answers: [{question, answer}], extra }`——与 Tauri 版 `ClarificationBubble`
    /// 的序列化口径一致，模型侧看到的形态不因客户端不同而变。
    fn submit_clarification(&mut self, cx: &mut Context<Self>) {
        let Some(questions) = self.clarification.clone() else {
            return;
        };
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let answers: Vec<serde_json::Value> = questions
            .iter()
            .enumerate()
            .map(|(i, (q, _))| {
                serde_json::json!({
                    "question": q,
                    "answer": self.clarify_answers.get(i).cloned().unwrap_or_default(),
                })
            })
            .collect();
        let port = self.port;
        self.status = "正在提交回答…".to_string();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/answer");
            let body = serde_json::json!({
                "session_id": sid,
                "answers": { "answers": answers, "extra": "" },
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.status = "回答已提交".to_string();
                        this.clarification = None;
                        this.clarify_answers.clear();
                    }
                    Err(e) => this.status = format!("提交回答失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 追问卡片（`ask_user` 挂起时显示在输入区上方）。
    ///
    /// 对齐 Tauri 版 `ClarificationBubble` 的**语义**（不是像素）：问题 + 选项，
    /// 全部答完才可提交。第一版只做「点选项作答」；无选项的问题给出提示让用户
    /// 用输入框里的原话补（避免为极少数场景先造一个多行富文本表单）。
    fn render_clarification(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(questions) = &self.clarification else {
            return div().into_any_element();
        };
        let all_answered = !questions.is_empty()
            && self
                .clarify_answers
                .iter()
                .take(questions.len())
                .all(|a| !a.trim().is_empty());
        let mut card = div().px_4().pb_2().child(
            div()
                .w_full()
                .max_w(px(896.))
                .mx_auto()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::ACCENT()))
                .bg(rgb(colors::BG_SECONDARY()))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors::ACCENT()))
                        .child("模型在向你确认（回答后它继续执行）"),
                ),
        );
        for (qi, (question, options)) in questions.iter().enumerate() {
            let answered = self
                .clarify_answers
                .get(qi)
                .map(|a| !a.trim().is_empty())
                .unwrap_or(false);
            let q_title = if answered {
                format!("✓ {}. {}", qi + 1, question)
            } else {
                format!("{}. {}", qi + 1, question)
            };
            let mut q_block = div().flex().flex_col().gap_1().child(
                div()
                    .text_xs()
                    .text_color(if answered {
                        rgb(colors::SUCCESS())
                    } else {
                        rgb(colors::TEXT_PRIMARY())
                    })
                    .child(q_title),
            );
            if options.is_empty() {
                q_block = q_block.child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("（该问题没有选项：把你输入框里写的原话作为回答）"),
                );
            }
            for (oi, option) in options.iter().enumerate() {
                let selected = self
                    .clarify_answers
                    .get(qi)
                    .map(|a| a == option)
                    .unwrap_or(false);
                let mut chip = div()
                    .id(("clarify", qi * 100 + oi))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .cursor_pointer()
                    .text_xs();
                if selected {
                    chip = chip
                        .bg(rgb(colors::ACCENT()))
                        .border_color(rgb(colors::ACCENT()))
                        .text_color(rgb(0xffffff));
                } else {
                    chip = chip
                        .bg(rgb(colors::BG_HOVER()))
                        .text_color(rgb(colors::TEXT_PRIMARY()));
                }
                let answer = option.clone();
                q_block = q_block.child(chip.child(option.clone()).on_click(cx.listener(
                    move |this, _, _w, cx| {
                        if let Some(slot) = this.clarify_answers.get_mut(qi) {
                            *slot = answer.clone();
                        }
                        cx.notify();
                    },
                )));
            }
            card = card.child(q_block);
        }
        let mut submit = div()
            .id("clarify-submit")
            .px_3()
            .py_1()
            .rounded_md()
            .text_xs()
            .text_color(rgb(0xffffff));
        submit = if all_answered {
            submit.bg(rgb(colors::ACCENT())).cursor_pointer()
        } else {
            submit.bg(rgb(colors::BG_TERTIARY()))
        };
        card.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(if all_answered {
                            "已全部作答".to_string()
                        } else {
                            format!("还有 {} 个问题未回答", questions.len())
                        }),
                )
                .child(div().flex_1())
                .child(
                    submit
                        .child("提交回答")
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            if this.clarify_answers.iter().all(|a| !a.trim().is_empty()) {
                                this.submit_clarification(cx);
                            }
                        })),
                ),
        )
        .into_any_element()
    }

    /// 拉取本会话的待审批操作（`GET /api/v1/approval/status`）。
    ///
    /// 只保留 `session_id` 匹配当前会话的项——审批是**会话内**的挂起操作，
    /// 别的会话的审批不该在本文出现（那会诱导用户给不相干的命令授权）。
    fn load_approvals(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/approval/status");
            if let Ok(body) = fetch_json(url).await {
                let mut rows: Vec<ApprovalRow> = Vec::new();
                if let Some(arr) = body.get("pending_approvals").and_then(|v| v.as_array()) {
                    for a in arr {
                        let sid = a
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        if session.as_deref() != Some(sid.as_str()) {
                            continue;
                        }
                        rows.push(ApprovalRow {
                            request_id: a
                                .get("request_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            description: a
                                .get("action_description")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            risk: a
                                .get("risk_level")
                                .and_then(|v| v.as_str())
                                .unwrap_or("-")
                                .to_string(),
                        });
                    }
                }
                let _ = this.update(cx, |this, cx| {
                    this.pending_approvals = rows;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 提交审批决策（`POST /api/v1/approval/respond`，decision = approve / deny）。
    fn respond_approval(
        &mut self,
        request_id: String,
        decision: &'static str,
        cx: &mut Context<Self>,
    ) {
        let port = self.port;
        self.status = format!("正在提交审批：{decision}");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/approval/respond");
            let body = serde_json::json!({ "request_id": request_id, "decision": decision });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("审批已提交（{decision}）"),
                    Err(e) => format!("提交审批失败：{e}"),
                };
                this.load_approvals(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 审批横幅（消息区顶部）：对齐 Tauri 版 `ApprovalBanner`（琥珀色告警 + 允许/拒绝）。
    ///
    /// 只显示**第一条**待审批：审批是一个个来的，堆一屏横幅既看不出关系也不知道点哪个；
    /// 处理完一条，下一条自然顶上（轮询刷新）。
    fn render_approval_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(a) = self.pending_approvals.first() else {
            return div().into_any_element();
        };
        let rid_approve = a.request_id.clone();
        let rid_deny = a.request_id.clone();
        div()
            .px_4()
            .pb_2()
            .child(
                div()
                    .w_full()
                    .max_w(px(896.))
                    .mx_auto()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::WARNING()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(colors::WARNING()))
                                    .child("⚠ 操作等待授权"),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!("风险：{}", a.risk)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(ellipsis(&a.description, 300)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                div()
                                    .id("approve")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::SUCCESS()))
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("允许")
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.respond_approval(rid_approve.clone(), "approve", cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("deny")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::ERROR()))
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("拒绝")
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.respond_approval(rid_deny.clone(), "deny", cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 拉取会话待办与目标（`GET /todos` + `/goals`，均按 `session_id` 过滤）。
    ///
    /// 两者都是**会话绑定**的推理辅助（ADR-022：无全局计划页），所以跟着当前会话走、
    /// 切会话即重拉。失败静默（与任务面板同口径：观测面不因抖动清空）。
    fn load_todos_goals(&self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let todos_res = fetch_json(format!("{base}/todos?session_id={sid}")).await;
            let goals_res = fetch_json(format!("{base}/goals?session_id={sid}")).await;
            let mut todos: Vec<(String, String)> = Vec::new();
            if let Ok(body) = todos_res {
                if let Some(arr) = body.get("todos").and_then(|v| v.as_array()) {
                    for t in arr {
                        todos.push((
                            t.get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("pending")
                                .to_string(),
                            t.get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                        ));
                    }
                }
            }
            let mut goals: Vec<GoalRow> = Vec::new();
            if let Ok(body) = goals_res {
                if let Some(arr) = body.get("goals").and_then(|v| v.as_array()) {
                    for g in arr {
                        // 元素是 `{ goal: {...}, progress, todo_total, todo_done }`
                        let goal = g.get("goal").unwrap_or(g);
                        goals.push(GoalRow {
                            title: goal
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            status: goal
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("active")
                                .to_string(),
                            progress: g.get("progress").and_then(|v| v.as_u64()).unwrap_or(0),
                            todo_total: g.get("todo_total").and_then(|v| v.as_u64()).unwrap_or(0),
                            todo_done: g.get("todo_done").and_then(|v| v.as_u64()).unwrap_or(0),
                        });
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.todos = todos;
                this.goals = goals;
                cx.notify();
            });
        })
        .detach();
    }

    /// 会话待办 / 目标窄条（在输入卡片**上方**，限宽 896px 居中）。
    ///
    /// 对齐 gui-vite `SessionTodoPanel`：
    /// - 两者都空时**整条不渲染**（不占位），有内容时可折叠；
    /// - **卡片 = header + 列表同一个容器**——折叠收起的是列表，不是把列表甩到
    ///   卡片外面（那样 header 一块、条目一块，观感上是两块分开的）；
    /// - 状态用 Lucide 图标（`CircleCheck` / `LoaderCircle` / `CircleDotDashed` /
    ///   `Circle`），不用 `[x]` / `[ ]` 文本——文本形态读起来像正文，且与 vite 两套观感。
    ///
    /// 已完成项只做置灰（vite 另有 line-through，GPUI 的 `Styled` 没有对应笔画修饰）。
    fn render_todo_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.todos.is_empty() && self.goals.is_empty() {
            return div().into_any_element();
        }
        let collapsed = self.todo_bar_collapsed;
        let in_progress = self
            .todos
            .iter()
            .filter(|(status, _)| status == "in_progress")
            .count();
        let done_count = self
            .todos
            .iter()
            .filter(|(status, _)| status == "completed")
            .count();
        // header 计数文案对齐 vite：`待办 N 项（进行中 N）· 完成 N`
        let mut count_text = format!("{} 项", self.todos.len());
        if in_progress > 0 {
            count_text.push_str(&format!("（进行中 {in_progress}）"));
        }
        if done_count > 0 {
            count_text.push_str(&format!(" · 完成 {done_count}"));
        }

        // header 整行可点（vite 那一侧就是通栏 `<button>`，不是只有小箭头能点）
        let mut card = div().flex().flex_col().gap_1().child(
            div()
                .id("todo-bar-toggle")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(
                    svg()
                        .path(IconName::ListTodo.path())
                        .size(px(12.))
                        .text_color(rgb(colors::TEXT_SECONDARY())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child("待办"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(count_text),
                )
                .child(div().flex_1())
                .child(
                    svg()
                        .path(if collapsed {
                            IconName::ChevronDown.path()
                        } else {
                            IconName::ChevronUp.path()
                        })
                        .size(px(12.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.todo_bar_collapsed = !this.todo_bar_collapsed;
                    cx.notify();
                })),
        );
        if !collapsed {
            // 列表落在**卡片内**（对应 vite 的 `{expanded && (...)}`）
            let mut list = div().flex().flex_col().gap_1().pt(px(2.));
            for g in &self.goals {
                let (icon, color) = match g.status.as_str() {
                    "completed" => (IconName::CircleCheck, colors::SUCCESS()),
                    "archived" => (IconName::Circle, colors::TEXT_TERTIARY()),
                    _ => (IconName::Target, colors::ACCENT()),
                };
                // 进度文案与 Tauri 版一致：有子待办才显示百分比，否则「进行中」
                //（`todo_total == 0` 时后端给的 progress 是 0，直接显示 0% 会误导）
                let progress_text = if g.todo_total > 0 {
                    format!("{}%（{}/{}）", g.progress, g.todo_done, g.todo_total)
                } else {
                    "进行中".to_string()
                };
                list = list.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .child(svg().path(icon.path()).size(px(12.)).text_color(rgb(color)))
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .child(ellipsis(&g.title, 40)),
                        )
                        .child(
                            div()
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(progress_text),
                        ),
                );
            }
            for (status, title) in &self.todos {
                let (icon, color) = match status.as_str() {
                    "completed" => (IconName::CircleCheck, colors::SUCCESS()),
                    "in_progress" => (IconName::LoaderCircle, colors::ACCENT()),
                    "pending" => (IconName::CircleDotDashed, colors::TEXT_TERTIARY()),
                    _ => (IconName::Circle, colors::TEXT_TERTIARY()),
                };
                let text_color = match status.as_str() {
                    "completed" => colors::TEXT_TERTIARY(),
                    "in_progress" => colors::TEXT_PRIMARY(),
                    _ => colors::TEXT_SECONDARY(),
                };
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(svg().path(icon.path()).size(px(12.)).text_color(rgb(color)))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(rgb(text_color))
                            .child(ellipsis(title, 48)),
                    );
                // vite 对未开始项右侧标「待开始」，这里同口径
                if status == "pending" {
                    row = row.child(
                        div()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("待开始"),
                    );
                }
                list = list.child(row);
            }
            card = card.child(list);
        }
        div()
            .px_4()
            .pb(px(6.))
            .child(
                div()
                    .w_full()
                    .max_w(px(896.))
                    .mx_auto()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .px_3()
                    .py_2()
                    .child(card),
            )
            .into_any_element()
    }

    /// 最近一轮请求的用量 → 上下文占用的分子。
    ///
    /// 两条来源（与 gui-vite `lib/token-usage.lastMessageUsage` 同口径）：
    /// ① 流式 `usage` 上报（`ctx_live`，最新一轮；切会话即清）；
    /// ② 消息链里每条消息自带的 `usage`——从末尾向前找第一条，让**切会话 /
    ///    重开应用**也能立刻显示占用，而不必等下一轮对话才开始有数。
    ///
    /// 压缩摘要消息特殊处理：它的 `prompt_tokens` 是压缩请求的输入（≈ 压缩前
    /// 上下文），不能当占用；改用「系统前缀基线（会话首条请求 input）+ 摘要输出」
    /// 估算压缩后占用。
    fn ctx_usage(&self) -> Option<MsgUsage> {
        // 流式实时值优先：它比库里最后一条更「当轮」（本轮 tool 循环的最后一跳）
        if let Some(u) = self.ctx_live {
            return Some(u);
        }
        for m in self.messages.iter().rev() {
            let Some(u) = m.usage else { continue };
            if !m.compression {
                return Some(u);
            }
            let baseline = self
                .sessions_data
                .iter()
                .find(|s| Some(&s.id) == self.current_session.as_ref())
                .and_then(|s| s.first_request_tokens)
                .or_else(|| {
                    // 后端没给基线（旧接口）时退回「窗口内第一条 assistant 的
                    // prompt_tokens」——仅当窗口包含会话开头才近似成立
                    self.messages
                        .iter()
                        .find(|x| x.role == "assistant" && x.usage.is_some())
                        .and_then(|x| x.usage)
                        .map(|x| x.prompt_tokens)
                })
                .unwrap_or(0);
            return Some(MsgUsage {
                prompt_tokens: baseline + u.completion_tokens,
                completion_tokens: u.completion_tokens,
                cache_read: u.cache_read,
            });
        }
        None
    }

    /// 上下文窗口（分母）：优先当前 chat 模型声明的 `context_length`（vite 同口径
    /// `chatModels.context_length`），否则用流式上报的窗口兜底。
    fn ctx_window(&self) -> u64 {
        self.current_model_context().unwrap_or(self.ctx_window_live)
    }

    /// 当前 chat 模型的声明上下文窗口：会话级模型优先，回退 `preferences.chat`
    /// （`load_models` 已把默认 chat 模型标成 `is_chat`）。
    fn current_model_context(&self) -> Option<u64> {
        if let Some((provider, model)) = &self.session_model {
            if let Some(row) = self
                .models
                .iter()
                .find(|m| &m.provider == provider && &m.name == model)
            {
                return row.context;
            }
        }
        self.models
            .iter()
            .find(|m| m.is_chat)
            .and_then(|m| m.context)
    }

    /// 上下文占用百分比（0-100；无用量时也是 0，用 `ctx_usage()` 区分「没有数据」）。
    ///
    /// 超过窗口时夹到 100、并按四舍五入取整（与 vite `ContextRing` 的
    /// `Math.round(Math.min(100, …))` 同口径）——截断会把 0.9% 显示成 0%，
    /// 看着像「还没有数据」。
    fn ctx_pct(&self) -> u64 {
        let window = self.ctx_window();
        let Some(used) = self.ctx_usage() else {
            return 0;
        };
        if window == 0 {
            return 0;
        }
        let capped = used
            .prompt_tokens
            .saturating_mul(100)
            .min(window.saturating_mul(100));
        (capped + window / 2) / window
    }

    /// 上下文占用配色（对齐 DSH ContextMeter / vite `ContextRing`）：
    /// 临界 80% → error，高 50% → warning，正常 → success。
    fn ctx_color(&self) -> u32 {
        let pct = self.ctx_pct();
        if pct >= 80 {
            colors::ERROR()
        } else if pct >= 50 {
            colors::WARNING()
        } else {
            colors::SUCCESS()
        }
    }

    /// 手动压缩当前会话（`POST /sessions/{id}/compress`）。
    ///
    /// 与 vite `ChatPanel.handleCompress` 同语义：流式中 / 已压缩中不重入；
    /// 完成后刷新消息链（压缩点消息 + 上下文变短）与会话 token 汇总。
    fn compress_session(&mut self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        if self.ctx_compressing || self.streaming {
            return;
        }
        self.ctx_compressing = true;
        self.ctx_compress = None;
        cx.notify();
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/compress");
            let res = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| {
                this.ctx_compressing = false;
                match res {
                    Ok(body) => {
                        let compressed = body
                            .get("compressed")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        this.ctx_compress = Some((
                            true,
                            if compressed {
                                "已压缩".to_string()
                            } else {
                                "无需压缩（消息太少）".to_string()
                            },
                        ));
                        // 压缩改的是消息链：重拉消息 / 汇总 / 会话列表（首条基线）
                        this.load_messages(sid.clone(), cx);
                        this.load_session_usage(cx);
                        this.load_sessions(cx);
                    }
                    Err(e) => this.ctx_compress = Some((false, format!("压缩失败：{e}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 选择模型：写**当前会话**的模型（`PUT /sessions/{id}/model`）。
    ///
    /// 与 Tauri 版 `ModelSelector.selectModel` 同语义——**只改会话，不改全局默认**
    /// （全局默认由设置页的 `POST /config/models/switch` 负责）。
    /// 未创建会话时先记在本地：首条消息的 `ChatRequest` 会带上 `model`/`provider`，
    /// 服务端建会话时即固化，不必等会话 id 生成后再补一次 PUT。
    fn select_chat_model(&mut self, provider: String, model: String, cx: &mut Context<Self>) {
        self.model_menu_open = false;
        self.thinking_menu_open = false;
        self.ctx_panel_open = false;
        self.session_model = Some((provider.clone(), model.clone()));
        // 新模型没声明当前档位 → 复位 default（与 vite `ThinkingSelect` 的
        // useEffect 兜底同逻辑；否则会把一个该模型不认识的档位发给后端）
        if let Some(cur) = &self.thinking {
            let supported = self
                .models
                .iter()
                .find(|m| m.provider == provider && m.name == model)
                .map(|m| m.reasoning_efforts.iter().any(|v| v == cur))
                .unwrap_or(false);
            if !supported {
                self.thinking = None;
            }
        }
        let Some(sid) = self.current_session.clone() else {
            self.status = format!("新会话将使用 {model} · {provider}");
            cx.notify();
            return;
        };
        let thinking = self.thinking.clone();
        let port = self.port;
        self.status = format!("正在为会话切换模型到 {model}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/model");
            let mut body = serde_json::json!({ "provider": provider, "model": model });
            if let Some(e) = thinking {
                body["thinking"] = serde_json::Value::String(e);
            }
            let res = put_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("会话模型已切换：{model}"),
                    Err(e) => format!("设置会话模型失败：{e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 打开 provider 编辑器：把该 provider 的 endpoint / api_key 回填进输入框。
    ///
    /// 必须**同步**做（`set_value` 要 `&mut Window`），所以值取自随列表缓存的那份
    /// （见 [`ProviderRow`] 的注释：缓存只用于回填，不用于写）。
    fn open_provider_editor(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.providers.iter().find(|p| p.name == name).cloned() else {
            return;
        };
        self.edit_endpoint
            .update(cx, |s, cx| s.set_value(row.endpoint, window, cx));
        self.edit_api_key
            .update(cx, |s, cx| s.set_value(row.api_key, window, cx));
        self.editing_provider = Some(name);
        self.settings_status = "编辑 provider：改完点「保存」（会先重读配置再写）".to_string();
        self.settings_error = false;
        cx.notify();
    }

    /// 测试 provider 连通性（`POST /api/v1/config/test-connection`）。
    ///
    /// 用**当前编辑框里的** endpoint / api_key 测——这样用户可以在保存前先验证，
    /// 避免把错误的 key 写进配置再回滚。model 取该 provider 的第一个模型。
    fn test_provider_connection(&mut self, cx: &mut Context<Self>) {
        let endpoint = self.edit_endpoint.read(cx).value().to_string();
        let api_key = self.edit_api_key.read(cx).value().to_string();
        let model = self
            .editing_provider
            .as_ref()
            .and_then(|name| {
                self.models
                    .iter()
                    .find(|m| &m.provider == name && m.capabilities.contains("chat"))
                    .or_else(|| self.models.iter().find(|m| &m.provider == name))
                    .map(|m| m.name.clone())
            })
            .unwrap_or_default();
        if endpoint.trim().is_empty() || api_key.trim().is_empty() || model.is_empty() {
            self.settings_status = "测试连接需要 endpoint / api_key / 模型名都不为空".to_string();
            self.settings_error = true;
            cx.notify();
            return;
        }
        let port = self.port;
        self.settings_status = format!("正在测试 {model}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/test-connection");
            let body = serde_json::json!({
                "endpoint": endpoint,
                "api_key": api_key,
                "model": model,
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        let ok = v.get("success").and_then(|b| b.as_bool()).unwrap_or(false);
                        let msg = v
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("")
                            .to_string();
                        this.settings_error = !ok;
                        this.settings_status = if ok {
                            format!("连接成功：{msg}")
                        } else {
                            format!("连接失败：{msg}")
                        };
                    }
                    Err(e) => {
                        this.settings_error = true;
                        this.settings_status = format!("测试失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 保存 provider 的 endpoint / api_key。
    ///
    /// ⚠️ `PUT /config` 是**整份替换**（`persist_and_reload`），不是按段合并
    /// （段保全只保「程序未建模的键」）。所以这里**先重新 GET 一份最新配置**，
    /// 只改目标 provider 的两个字段，再整体提交——把「读-改-写」的覆盖窗口压到
    /// 最短（一次往返），不要改成「用启动时缓存的配置提交」，那会冲掉期间别人
    /// 改过的建模字段（例如 agent 刚切的模型、MCP 预热注入的东西）。
    fn save_provider(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.editing_provider.clone() else {
            return;
        };
        let endpoint = self.edit_endpoint.read(cx).value().to_string();
        let api_key = self.edit_api_key.read(cx).value().to_string();
        let port = self.port;
        self.settings_status = format!("正在保存 {name}…");
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let mut cfg = match fetch_json(format!("{base}/config")).await {
                Ok(body) => body
                    .get("config")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("保存失败（读取配置）：{e}");
                        this.settings_error = true;
                        cx.notify();
                    });
                    return;
                }
            };
            let mut found = false;
            if let Some(arr) = cfg
                .get_mut("models")
                .and_then(|m| m.get_mut("providers"))
                .and_then(|p| p.as_array_mut())
            {
                for p in arr.iter_mut() {
                    if p.get("name").and_then(|v| v.as_str()) == Some(name.as_str()) {
                        p["endpoint"] = serde_json::Value::String(endpoint.clone());
                        p["api_key"] = serde_json::Value::String(api_key.clone());
                        found = true;
                    }
                }
            }
            if !found {
                let _ = this.update(cx, |this, cx| {
                    this.settings_status = format!("保存失败：配置里没有 provider {name}");
                    this.settings_error = true;
                    cx.notify();
                });
                return;
            }
            let res = put_json(
                format!("{base}/config"),
                serde_json::json!({ "config": cfg }),
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.settings_status = format!("{name} 已保存");
                        this.settings_error = false;
                        this.editing_provider = None;
                        this.load_models(cx);
                    }
                    Err(e) => {
                        this.settings_status = format!("保存失败：{e}");
                        this.settings_error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 设置页（第一版）：模型列表 + 切换聊天模型。
    ///
    /// 只做「能日常用」的最小集合——没有它，换模型只能回 Tauri 版改配置。
    /// 存储 / 安全 / MCP / soul 等分组按后续批次补（端点都已存在，见
    /// `server/src/api/config/routes.rs`）。
    /// 子智能体页（对齐 `gui-vite/src/components/roles/RolesPanel.tsx`）：
    /// 左列表（搜索 + 角色项）+ 右详情（统计条 + 系统提示 / 工具 / 模型）。
    /// 文件页（对齐 Web 版 `WorkspacePanel` 的**只读部分**）：左侧单层目录树 +
    /// 右侧文件内容。
    ///
    /// 编辑保存（apply-patch）、diff 面板、目录绑定留阶段 2 —— 那三块各自要一套
    /// 状态机（草稿 / 冲突 / 快照索引），先把"能看"落地。
    fn render_workspace(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let crumb = if self.ws_path.is_empty() {
            if self.ws_root.is_empty() {
                "未选择会话（无可浏览目录）".to_string()
            } else {
                self.ws_root.clone()
            }
        } else {
            format!(
                "{}/{}",
                self.ws_root.trim_end_matches(['\\', '/']),
                self.ws_path
            )
        };

        let mut list = div().flex_1().min_h_0().w_full().flex().flex_col().gap_1();
        if !self.ws_path.is_empty() {
            let parent = {
                let p = self.ws_path.replace('\\', "/");
                match p.rfind('/') {
                    Some(pos) => p[..pos].to_string(),
                    None => String::new(),
                }
            };
            list = list.child(
                div()
                    .id("ws-up")
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(
                        svg()
                            .path(IconName::CornerLeftUp.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child("上级目录")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_workspace_tree(parent.clone(), cx);
                    })),
            );
        }
        for (ix, entry) in self.ws_entries.iter().enumerate() {
            let is_dir = entry.is_dir;
            let path = entry.path.clone();
            let name = entry.name.clone();
            let size = entry.size;
            list = list.child(
                div()
                    .id(("ws-entry", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        svg()
                            .path(if is_dir {
                                IconName::Folder.path()
                            } else {
                                IconName::FileText.path()
                            })
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(rgb(if is_dir {
                                colors::TEXT_PRIMARY()
                            } else {
                                colors::TEXT_SECONDARY()
                            }))
                            .child(name),
                    )
                    .children(if is_dir {
                        None
                    } else {
                        Some(
                            div()
                                .flex_shrink_0()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(format_size(size)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_dir {
                            this.load_workspace_tree(path.clone(), cx);
                        } else {
                            this.load_workspace_file(path.clone(), cx);
                        }
                    })),
            );
        }

        let content = match &self.ws_file {
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    svg()
                        .path(IconName::FileText.path())
                        .size(px(36.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("从左侧选择一个文件查看内容"),
                )
                .into_any_element(),
            Some((path, text)) => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(colors::BORDER()))
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(path.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .p_4()
                        .text_size(px(12.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(text.clone())
                        .overflow_y_scrollbar(),
                )
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        svg()
                            .path(IconName::FolderTree.path())
                            .size(px(16.))
                            .text_color(rgb(colors::TEXT_SECONDARY())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(15.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("文件"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(crumb),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(self.ws_status.clone()),
                    )
                    // 选择工作目录：逐级浏览后绑定到当前会话
                    .child(
                        div()
                            .id("ws-pick-dir")
                            .flex_shrink_0()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::FolderOpen.path())
                                    .size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .child("选择目录")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_workspace_picker(PickerTarget::Session, cx);
                            })),
                    )
                    // Diff 开关：右侧展开「工作区改动」面板（对照会话快照基线）
                    .child(
                        div()
                            .id("ws-diff-toggle")
                            .flex_shrink_0()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(if self.ws_diff_open {
                                colors::ACCENT()
                            } else {
                                colors::BORDER()
                            }))
                            .text_size(px(11.))
                            .text_color(rgb(if self.ws_diff_open {
                                colors::ACCENT()
                            } else {
                                colors::TEXT_SECONDARY()
                            }))
                            .cursor_pointer()
                            .child("Diff")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ws_diff_open = !this.ws_diff_open;
                                if this.ws_diff_open {
                                    this.load_workspace_diff_list(cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(280.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_SECONDARY()))
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(if self.ws_path.is_empty() {
                                        "工作区根目录".to_string()
                                    } else {
                                        self.ws_path.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .p_2()
                                    .child(list.overflow_y_scrollbar()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(rgb(colors::BG_PRIMARY()))
                            .child(content),
                    )
                    // diff 面板（右侧，可开关）：AI 改了什么，一眼看到
                    .children(if self.ws_diff_open {
                        Some(self.render_diff_panel(cx))
                    } else {
                        None
                    }),
            )
    }
    /// diff 面板（右侧）：改动文件列表 + 选中文件的 unified diff（+/- 染色）。
    fn render_diff_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut files = div().flex().flex_col().gap_1().w_full();
        if self.ws_diff_files.is_empty() {
            files = files.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(self.ws_diff_status.clone()),
            );
        }
        for (ix, f) in self.ws_diff_files.iter().enumerate() {
            let selected = self
                .ws_diff
                .as_ref()
                .map(|(p, _)| p == &f.path)
                .unwrap_or(false);
            let (color, label) = match f.status.as_str() {
                "added" => (colors::SUCCESS(), "A"),
                "removed" => (colors::ERROR(), "D"),
                "binary" => (colors::WARNING(), "B"),
                _ => (colors::ACCENT(), "M"),
            };
            let path = f.path.clone();
            files = files.child(
                div()
                    .id(("ws-diff-file", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .bg(rgb(if selected {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(color))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(f.path.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("+{} −{}", f.new_lines, f.old_lines)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_workspace_diff(path.clone(), cx);
                    })),
            );
        }

        let detail = match &self.ws_diff {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选一个文件查看 diff"),
                )
                .into_any_element(),
            Some((_, unified)) => {
                let mut lines = div().flex().flex_col().w_full();
                for line in unified.lines().take(3000) {
                    let color = if line.starts_with("+++") || line.starts_with("---") {
                        colors::TEXT_TERTIARY()
                    } else if line.starts_with('+') {
                        colors::SUCCESS()
                    } else if line.starts_with('-') {
                        colors::ERROR()
                    } else if line.starts_with("@@") {
                        colors::ACCENT()
                    } else {
                        colors::TEXT_SECONDARY()
                    };
                    lines = lines.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(color))
                            .child(line.to_string()),
                    );
                }
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .p_2()
                    .child(lines.overflow_y_scrollbar())
                    .into_any_element()
            }
        };

        div()
            .w(px(380.))
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child("工作区改动（相对会话起点快照）"),
            )
            // 列表必须自己可滚动：改动文件可能几十个，`max_h` 只限高不裁剪，
            // 内容会溢出盖到下面的 diff 区（实测二十多个文件时整片文字重叠）
            .child(
                div()
                    // 同 picker：滚动容器必须落在确定高度里（`max_h` 会让它塌成 0）
                    .h(px(240.))
                    .min_h_0()
                    .p_2()
                    .child(files.overflow_y_scrollbar()),
            )
            .child(detail)
            .into_any_element()
    }

    /// 记忆页：左列表（按目录分组）+ 右详情（L0/L1/L2 三层，L2 默认收起）。
    ///
    /// 不建完整树（Web 版 `buildMemoryTree`）：记忆条目本就按目录扁平分布，
    /// 一层分组标题 + 平铺足够定位，且省掉树节点的展开态管理。
    /// 知识页：三 tab（浏览 / 导入 / 搜索）。
    fn render_knowledge(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = div().flex().flex_row().items_center().gap_1();
        for (i, label) in ["浏览", "导入", "搜索"].iter().enumerate() {
            let idx = i as u8;
            let is_cur = self.kb_tab == idx;
            tabs = tabs.child(
                div()
                    .id(("kb-tab", i))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if is_cur {
                        colors::ACCENT()
                    } else {
                        colors::BORDER()
                    }))
                    .bg(rgb(if is_cur {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(if is_cur {
                        colors::ACCENT()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.kb_tab = idx;
                        // 首次进导入 tab 才拉一次浏览目录（避免每次切都发请求）
                        if idx == 1 && this.kb_browse_entries.is_empty() {
                            this.load_kb_browse(String::new(), cx);
                        }
                        cx.notify();
                    })),
            );
        }

        let content = match self.kb_tab {
            1 => self.render_kb_ingest(cx).into_any_element(),
            2 => self.render_kb_search_tab(cx).into_any_element(),
            _ => self.render_kb_browse(cx).into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::BookOpen.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("知识库"),
                                    ),
                            )
                            .child(tabs),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(self.kb_status.clone()),
                    ),
            )
            .child(content)
    }

    /// 知识页·浏览：面包屑 + 条目列表 + 右内容（L0/L1/L2 切换）。
    fn render_kb_browse(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut crumbs = div().flex().flex_row().items_center().gap_1().child(
            div()
                .id("kb-root")
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child("知识库根")
                .on_click(cx.listener(|this, _, _, cx| this.load_kb_entries(String::new(), cx))),
        );
        if !self.kb_path.is_empty() {
            let parent = self
                .kb_path
                .rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default();
            crumbs = crumbs
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("/ {}", self.kb_path)),
                )
                .child(
                    div()
                        .id("kb-up")
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(10.))
                        .text_color(rgb(colors::ACCENT()))
                        .child("↑ 上级")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.load_kb_entries(parent.clone(), cx)
                        })),
                );
        }

        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if self.kb_entries.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("这个目录是空的（或尚未导入任何文档）"),
            );
        }
        for (ix, e) in self.kb_entries.iter().enumerate() {
            let is_sel = self.kb_selected.as_deref() == Some(e.uri.as_str());
            let uri = e.uri.clone();
            let is_dir = e.is_dir;
            let layers = format!(
                "{}{}{}",
                if e.has_abstract { "L0 " } else { "" },
                if e.has_overview { "L1 " } else { "" },
                if e.has_detail { "L2" } else { "" }
            );
            list = list.child(
                div()
                    .id(("kb", ix))
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(if is_sel {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(rgb(if is_sel {
                                        colors::ACCENT()
                                    } else {
                                        colors::TEXT_PRIMARY()
                                    }))
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(format!(
                                        "{}{}",
                                        if is_dir { "[D] " } else { "" },
                                        e.name
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(layers),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_dir {
                            // 目录：uri → 相对知识根的路径（`tianyan://knowledge/a/b` → `a/b`）
                            let rel = uri
                                .split("knowledge/")
                                .nth(1)
                                .unwrap_or_default()
                                .to_string();
                            this.load_kb_entries(rel, cx);
                        } else {
                            this.kb_selected = Some(uri.clone());
                            this.read_kb_entry(uri.clone(), "detail".to_string(), cx);
                        }
                    })),
            );
        }

        let detail = match (&self.kb_selected, &self.kb_content) {
            (Some(uri), Some((level, text))) => {
                let uri_owned = uri.clone();
                let uri_del = uri.clone();
                let mut levels = div().flex().flex_row().items_center().gap_1();
                for (li, lv) in ["abstract", "overview", "detail"].iter().enumerate() {
                    let is_cur = level == lv;
                    let u = uri_owned.clone();
                    levels = levels.child(
                        div()
                            .id(("kb-lv", li))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(if is_cur {
                                colors::ACCENT()
                            } else {
                                colors::BORDER()
                            }))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(if is_cur {
                                colors::ACCENT()
                            } else {
                                colors::TEXT_SECONDARY()
                            }))
                            .child(lv.to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.read_kb_entry(u.clone(), lv.to_string(), cx)
                            })),
                    );
                }
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER()))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_SECONDARY()))
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .overflow_hidden()
                                            .child(uri_owned.clone()),
                                    )
                                    .child(
                                        div()
                                            .id("kb-del")
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .bg(rgb(colors::ERROR()))
                                            .cursor_pointer()
                                            .text_size(px(10.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("删除")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.delete_kb_entry(uri_del.clone(), cx)
                                            })),
                                    ),
                            )
                            .child(levels),
                    )
                    .child(
                        div().flex_1().min_h_0().p_4().child(
                            div()
                                .w_full()
                                .child(TextView::markdown(
                                    ("kb-md", 0usize),
                                    SharedString::from(text.clone()),
                                ))
                                .overflow_y_scrollbar(),
                        ),
                    )
                    .into_any_element()
            }
            _ => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::BookOpen.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选一个条目查看内容（目录点进去）"),
                )
                .into_any_element(),
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().px_4().py_2().child(crumbs))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(320.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_SECONDARY()))
                            .child(list.overflow_y_scrollbar()),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(detail)),
            )
    }
    /// 知识页·导入：选文件（弹窗）或按路径批量，读本地文件 → multipart 上传。
    ///
    /// 文件选择走**弹窗**（`Dialog::KbFilePicker`）而不是常驻列表：导入页的主任务只有
    /// 「选文件 + 导入」，把一张文件列表铺在页面上会把主次关系拉平、也挤掉结果区。
    fn render_kb_ingest(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 已选摘要：弹窗关掉后仍要看得到「将要导入什么」
        let picked_line = if self.kb_picked.is_empty() {
            "还没有选中文件".to_string()
        } else {
            format!(
                "已选 {} 个：{}",
                self.kb_picked.len(),
                self.kb_picked
                    .iter()
                    .take(4)
                    .map(|p| std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        let mut results = div().flex().flex_col().gap_1().w_full();
        for (i, (name, status)) in self.kb_ingest_results.iter().enumerate() {
            results = results.child(
                div()
                    .id(("kb-ir", i))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(status.clone()),
                    ),
            );
        }

        div()
            .flex_1()
            .min_h_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            // ① 主路径：选文件（弹窗）
            .child(
                div()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                svg()
                                    .path(IconName::Upload.path())
                                    .size(px(20.))
                                    .text_color(rgb(colors::ACCENT())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("选择文件导入"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY()))
                                            .child(
                                                "可多选；导入是异步任务，L0/L1 摘要生成前浏览里可能还看不到",
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("kb-open-picker")
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(colors::ACCENT()))
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child("选择文件…")
                                    .on_click(cx.listener(|this, _, _, cx| this.open_kb_picker(cx))),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(picked_line),
                    )
                    .child(field_wrap(&self.kb_ingest_category)),
            )
            // ② 次路径：按路径批量（目录递归）
            .child(
                div()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("或按路径批量导入（文件或目录，目录递归，上限 200）"),
                    )
                    .child(field_wrap(&self.kb_ingest_path))
                    .child(
                        div()
                            .id("kb-ingest")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("按路径导入")
                            .on_click(cx.listener(|this, _, _, cx| this.ingest_kb(cx))),
                    ),
            )
            // ③ 结果
            .child(results)
            .overflow_y_scrollbar()
    }

    fn render_kb_search_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut results = div().flex().flex_col().gap_2().w_full();
        if self.kb_results.is_empty() {
            results = results.child(
                div()
                    .py_6()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("没有结果——输入词后点搜索"),
            );
        }
        for (i, r) in self.kb_results.iter().enumerate() {
            results = results.child(
                div()
                    .id(("kb-r", i))
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(r.source.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::ACCENT()))
                                    .child(format!("{:.3}", r.score)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(ellipsis(&r.content.replace('\n', " "), 300)),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(field_wrap(&self.kb_search)))
                    .child(
                        div()
                            .id("kb-do-search")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(colors::ACCENT()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("搜索")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let q = this.kb_search.read(cx).value().to_string();
                                this.search_kb(q, cx);
                            })),
                    ),
            )
            .child(results.overflow_y_scrollbar())
    }

    /// 审批页：工作流配置 + 待审批（可操作）+ 待确认指纹 + 审计记录。
    ///
    /// 审批链路口径（ADR-033）：工具执行门控 → 风险分级 → 自动审批规则
    /// （Deny 优先）→ `ApprovalMode` 三态短路 → 人工审批挂起通道。
    /// 本页是这个链路的**观测面**：配置长什么样、当前挂起什么、过往怎么判的。
    fn render_approval(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();

        // 1. 配置卡
        let mut cfg = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("工作流配置"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_TERTIARY()))
                            .text_size(px(10.))
                            .text_color(rgb(colors::ACCENT()))
                            .child(if self.approval_mode.is_empty() {
                                "未装配".to_string()
                            } else {
                                format!("模式 {}", self.approval_mode)
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if self.approval_auto {
                                colors::SUCCESS()
                            } else {
                                colors::TEXT_TERTIARY()
                            }))
                            .child(if self.approval_auto {
                                "自动审批已启用"
                            } else {
                                "自动审批已关闭"
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!(
                                "超时 {}s · 最大挂起 {} · 已确认指纹 {}",
                                self.approval_timeout,
                                self.approval_max_pending,
                                self.confirmed_action_count
                            )),
                    ),
            );
        if !self.approval_prompt_cmds.is_empty() {
            let mut chips = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for c in &self.approval_prompt_cmds {
                chips = chips.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::WARNING()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::WARNING()))
                        .child(c.clone()),
                );
            }
            cfg = cfg.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("总是询问的命令（不被自动规则与任何模式放行）"),
                    )
                    .child(chips),
            );
        }
        if !self.approval_rules.is_empty() {
            let mut rules = div().flex().flex_col().gap_1().w_full();
            for r in self.approval_rules.iter().take(12) {
                rules = rules.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .overflow_hidden()
                                .child(r.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(if r.decision.eq_ignore_ascii_case("deny") {
                                    colors::ERROR()
                                } else {
                                    colors::SUCCESS()
                                }))
                                .child(r.decision.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(if r.enabled { "启用" } else { "停用" }),
                        ),
                );
            }
            cfg = cfg.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!(
                                "自动审批规则（{} 条，Deny 优先）",
                                self.approval_rules.len()
                            )),
                    )
                    .child(rules),
            );
        }
        body = body.child(cfg);

        // 2. 待审批（可操作）
        let mut pending = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(if self.pending_approvals.is_empty() {
                colors::BORDER()
            } else {
                colors::WARNING()
            }))
            .bg(rgb(colors::BG_SECONDARY()))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .child(format!("待审批（{}）", self.pending_approvals.len())),
            );
        if self.pending_approvals.is_empty() {
            pending = pending.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("没有挂起的审批请求"),
            );
        }
        for (ix, a) in self.pending_approvals.iter().enumerate() {
            let rid_a = a.request_id.clone();
            let rid_d = a.request_id.clone();
            pending = pending.child(
                div()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_PRIMARY()))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child(a.description.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(risk_color(&a.risk)))
                                    .child(format!("风险 {}", a.risk)),
                            ),
                    )
                    .child(
                        div()
                            .id(("ap-ok", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::SUCCESS()))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("允许")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond_approval(rid_a.clone(), "approve", cx)
                            })),
                    )
                    .child(
                        div()
                            .id(("ap-no", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::ERROR()))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("拒绝")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond_approval(rid_d.clone(), "deny", cx)
                            })),
                    ),
            );
        }
        body = body.child(pending);

        // 3. 待确认指纹（「询问用户」降级链路）
        if !self.pending_confirmations.is_empty() {
            let mut list = div().flex().flex_col().gap_1().w_full();
            for (ix, fp) in self.pending_confirmations.iter().enumerate() {
                list = list.child(
                    div()
                        .id(("ap-fp", ix))
                        .w_full()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(fp.clone()),
                );
            }
            body = body.child(
                div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(format!(
                                "待确认操作指纹（{}）",
                                self.pending_confirmations.len()
                            )),
                    )
                    .child(list),
            );
        }

        // 4. 审计记录
        let mut hist = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .child(format!("审计记录（{}）", self.approval_history.len())),
            );
        if self.approval_history.is_empty() {
            hist = hist.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("暂无审批记录"),
            );
        }
        for h in self.approval_history.iter().take(20) {
            hist = hist.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(risk_color(&h.risk)))
                            .child(h.risk.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(h.description.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if h.decision.eq_ignore_ascii_case("deny") {
                                colors::ERROR()
                            } else if h.decision.eq_ignore_ascii_case("approve") {
                                colors::SUCCESS()
                            } else {
                                colors::TEXT_TERTIARY()
                            }))
                            .child(h.decision.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(match h.result {
                                Some(true) => colors::SUCCESS(),
                                Some(false) => colors::ERROR(),
                                None => colors::TEXT_TERTIARY(),
                            }))
                            .child(match h.result {
                                Some(true) => "执行成功".to_string(),
                                Some(false) => "执行失败".to_string(),
                                None => "未执行".to_string(),
                            }),
                    ),
            );
        }
        body = body.child(hist);

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::Shield.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("审批"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(self.approvals_page_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("ap-reload")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("刷新")
                            .on_click(cx.listener(|this, _, _, cx| this.load_approval_page(cx))),
                    ),
            )
            .child(body.overflow_y_scrollbar())
    }

    /// 洞察页：总览四卡 + 模型用量（含分组）+ 调度器任务表。
    fn render_insights(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.insight_stats.clone().unwrap_or_default();
        let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();

        body = body.child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .flex_wrap()
                .child(stat_card(
                    "技能",
                    format!("{}", s.total_skills_tracked),
                    format!("累计调用 {} 次", group_thousands(s.total_skill_calls)),
                ))
                .child(stat_card(
                    "工具",
                    format!("{}", s.total_tools_tracked),
                    format!("累计调用 {} 次", group_thousands(s.total_tool_calls)),
                ))
                .child(stat_card(
                    "文档",
                    format!("{}", s.total_docs_tracked),
                    "VFS 访问过的文档".to_string(),
                ))
                .child(stat_card(
                    "搜索",
                    format!("{}", s.total_searches),
                    "检索次数".to_string(),
                )),
        );

        if let Some(t) = &self.usage_total {
            let mut usage = div()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .flex_wrap()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(colors::TEXT_PRIMARY()))
                                .child(format!("近 {} 天模型用量", self.usage_days)),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(format!(
                                    "{} 次调用 · 总 token {} · 缓存命中 {:.1}%",
                                    group_thousands(t.calls),
                                    group_thousands(t.total_tokens),
                                    t.cache_hit_rate * 100.0
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .flex_wrap()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("未命中输入 {}", group_thousands(t.uncached_input)))
                        .child(format!("缓存命中输入 {}", group_thousands(t.cached_input)))
                        .child(format!("输出 {}", group_thousands(t.completion_tokens))),
                );
            if !self.usage_grouped.is_empty() {
                let mut rows = div().flex().flex_col().gap_1().w_full();
                for g in self.usage_grouped.iter().take(10) {
                    rows = rows.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(g.group.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!(
                                        "{} 次 · {} tok · 命中 {:.0}%",
                                        group_thousands(g.calls),
                                        group_thousands(g.total_tokens),
                                        g.cache_hit_rate * 100.0
                                    )),
                            ),
                    );
                }
                usage = usage.child(rows);
            }
            body = body.child(usage);
        }

        let mut sched = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("调度器"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if self.scheduler_running {
                                colors::SUCCESS()
                            } else {
                                colors::TEXT_TERTIARY()
                            }))
                            .child(if self.scheduler_running {
                                "运行中"
                            } else {
                                "未装配"
                            }),
                    ),
            );
        if let Some(id) = &self.scheduler_executing {
            sched = sched.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(colors::ACCENT()))
                    .child(format!("正在执行 {id}")),
            );
        }
        if self.sched_status_tasks.is_empty() {
            sched = sched.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("没有已注册的定时任务"),
            );
        }
        for t in &self.sched_status_tasks {
            sched = sched.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(t.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!(
                                "{} · 跑 {} 次 · 上次 {}前",
                                format_interval(t.interval_secs),
                                t.run_count,
                                format_secs_short(t.last_run_ago_secs)
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("下次 {}后", format_secs_short(t.next_due_in_secs))),
                    ),
            );
        }
        body = body.child(sched);

        // 天数切换 + 刷新
        let mut days_bar = div().flex().flex_row().items_center().gap_1();
        for d in [1i64, 3, 7, 30] {
            let is_cur = self.usage_days == d;
            days_bar = days_bar.child(
                div()
                    .id(("ins-days", d as usize))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if is_cur {
                        colors::ACCENT()
                    } else {
                        colors::BORDER()
                    }))
                    .bg(rgb(if is_cur {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .cursor_pointer()
                    .text_size(px(10.))
                    .text_color(rgb(if is_cur {
                        colors::ACCENT()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(format!("{d} 天"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage_days = d;
                        this.load_insights(cx);
                    })),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::ChartLine.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("洞察"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(self.insights_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(days_bar)
                            .child(
                                div()
                                    .id("ins-reload")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .cursor_pointer()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("刷新")
                                    .on_click(cx.listener(|this, _, _, cx| this.load_insights(cx))),
                            ),
                    ),
            )
            .child(body.overflow_y_scrollbar())
    }

    fn render_memory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 按目录分组（relative_path 的父段；根级归到「(根)」）
        let mut groups: Vec<(String, Vec<&MemoryRow>)> = Vec::new();
        for m in &self.memories {
            let key = m
                .relative_path
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_else(|| "(根)".to_string());
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(m),
                None => groups.push((key, vec![m])),
            }
        }
        let selected = self
            .memories
            .iter()
            .find(|m| self.memory_selected.as_deref() == Some(m.uri.as_str()))
            .cloned();

        // 左：分组列表
        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_2();
        if self.memories.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Brain.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("暂无记忆——自演化综述任务会在会话结束后写入"),
                    ),
            );
        }
        for (gi, (dir, items)) in groups.iter().enumerate() {
            let mut section = div().flex().flex_col().gap_1().child(
                div()
                    .px_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(dir.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("· {}", items.len())),
                    ),
            );
            for (ix, m) in items.iter().enumerate() {
                let is_sel = self.memory_selected.as_deref() == Some(m.uri.as_str());
                let uri = m.uri.clone();
                // 重要度三档配色：≥0.7 绿（稳固）/ ≥0.4 次级 / 其余红（待巩固）
                let imp_color = if m.importance >= 0.7 {
                    colors::SUCCESS()
                } else if m.importance >= 0.4 {
                    colors::TEXT_SECONDARY()
                } else {
                    colors::ERROR()
                };
                section = section.child(
                    div()
                        .id(("mem", gi * 1000 + ix))
                        .w_full()
                        .px_2()
                        .py_2()
                        .rounded_lg()
                        .bg(rgb(if is_sel {
                            colors::BG_TERTIARY()
                        } else {
                            colors::BG_SECONDARY()
                        }))
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(12.))
                                        .text_color(rgb(if is_sel {
                                            colors::ACCENT()
                                        } else {
                                            colors::TEXT_PRIMARY()
                                        }))
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .child(m.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(rgb(imp_color))
                                        .child(format!("{:.2}", m.importance)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .overflow_hidden()
                                .child(
                                    m.abstract_text
                                        .as_ref()
                                        .map(|t| ellipsis(t, 56))
                                        .unwrap_or_else(|| "（无摘要）".to_string()),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.memory_selected = Some(uri.clone());
                            this.memory_detail_open = false;
                            cx.notify();
                        })),
                );
            }
            list = list.child(section);
        }

        // 右：详情（L0 → L1 → L2，L2 折叠）
        let detail = match &selected {
            None => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::Brain.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选择一条记忆查看三层内容"),
                )
                .into_any_element(),
            Some(m) => {
                let mut head = div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(m.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(m.relative_path.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .flex_wrap()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("重要度 {:.2}", m.importance))
                            .child(if m.updated_at.is_empty() {
                                String::new()
                            } else {
                                format!("更新于 {}", short_stamp(&m.updated_at))
                            })
                            .child(if m.tags.is_empty() {
                                String::new()
                            } else {
                                format!("#{}", m.tags.join(" #"))
                            }),
                    );
                let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();
                if let Some(t) = &m.abstract_text {
                    body = body.child(memory_block("L0 摘要", t.clone()));
                }
                if let Some(t) = &m.overview_text {
                    body = body.child(memory_block("L1 概览", t.clone()));
                }
                if let Some(t) = &m.detail_text {
                    let open = self.memory_detail_open;
                    let label = if open {
                        "▾ L2 全文（收起）".to_string()
                    } else {
                        format!("▸ L2 全文（{} 字，点击展开）", t.chars().count())
                    };
                    body = body.child(
                        div()
                            .id("mem-detail-toggle")
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::ACCENT()))
                            .child(label)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.memory_detail_open = !this.memory_detail_open;
                                cx.notify();
                            })),
                    );
                    if open {
                        body = body.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .child(t.clone()),
                        );
                    }
                }
                head = head.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(m.uri.clone()),
                );
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(head)
                    .child(body.overflow_y_scrollbar())
                    .into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::Brain.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("记忆"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(self.memories_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("mem-reload")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("刷新")
                            .on_click(cx.listener(|this, _, _, cx| this.load_memories(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(320.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_SECONDARY()))
                            .child(list.overflow_y_scrollbar()),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(detail)),
            )
    }

    /// 定时任务页：列表卡片 + 新建表单（间隔预设 / 自定义秒 / 工作区 / 指令）。
    ///
    /// 与 Web 版 `ScheduledTasksSection` 同口径：间隔预设按钮组而非下拉
    /// （GPUI 没有原生 select；按钮行反而更好点），自定义走秒数输入。
    fn render_scheduled(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let head = div()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                svg()
                                    .path(IconName::CalendarClock.path())
                                    .size(px(16.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child("定时任务"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(
                                "按执行间隔调用智能体在指定工作区工作；\
                                 服务停机期间超期的任务重启后补跑一次",
                            ),
                    ),
            )
            .child(
                div()
                    .id("sched-toggle")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(if self.sched_form_open {
                        "取消"
                    } else {
                        "＋ 新建定时任务"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sched_form_open = !this.sched_form_open;
                        cx.notify();
                    })),
            );

        let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();

        // 状态行（创建/删除结果；空串不占位）
        if !self.sched_status.is_empty() {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(self.sched_status.clone()),
            );
        }

        // 新建表单
        if self.sched_form_open {
            let mut presets = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for (secs, label) in SCHED_PRESETS {
                let is_cur = self.sched_preset == secs;
                presets = presets.child(
                    div()
                        .id(("sched-preset", secs))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(if is_cur {
                            colors::ACCENT()
                        } else {
                            colors::BORDER()
                        }))
                        .bg(rgb(if is_cur {
                            colors::BG_TERTIARY()
                        } else {
                            colors::BG_SECONDARY()
                        }))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(if is_cur {
                            colors::ACCENT()
                        } else {
                            colors::TEXT_SECONDARY()
                        }))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sched_preset = secs;
                            cx.notify();
                        })),
                );
            }
            let mut form = div()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .flex()
                .flex_col()
                .gap_2()
                .child(field_wrap(&self.sched_name))
                .child(presets);
            if self.sched_preset == 0 {
                form = form.child(field_wrap(&self.sched_custom_secs));
            }
            form = form
                .child(field_wrap(&self.sched_workspace))
                .child(field_wrap_area(&self.sched_prompt))
                .child(
                    div()
                        .id("sched-create")
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(colors::ACCENT()))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child("创建")
                        .on_click(cx.listener(|this, _, _, cx| this.create_sched_task(cx))),
                );
            body = body.child(form);
        }

        // 列表
        if self.sched_tasks.is_empty() {
            body = body.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::CalendarClock.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(
                                "暂无定时任务——可让智能体用 schedule_task 工具创建，或在此手动创建",
                            ),
                    ),
            );
        } else {
            let mut list = div().flex().flex_col().gap_2().w_full();
            for (ix, t) in self.sched_tasks.iter().enumerate() {
                let id = t.id.clone();
                let mut card = div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child(t.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(rgb(colors::BORDER()))
                                            .bg(rgb(colors::BG_TERTIARY()))
                                            .text_size(px(10.))
                                            .text_color(rgb(colors::ACCENT()))
                                            .child(format_interval(t.interval_secs)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!(
                                        "工作区: {}",
                                        if t.workspace.is_empty() {
                                            "(默认)"
                                        } else {
                                            &t.workspace
                                        }
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(ellipsis(&t.prompt, 140)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!(
                                        "下次(约): {}",
                                        t.next_run_at
                                            .as_deref()
                                            .and_then(parse_rfc3339_secs)
                                            .map(relative_from_now)
                                            .unwrap_or_else(|| "待定".to_string())
                                    ))
                                    .child(
                                        t.last_run_at
                                            .as_deref()
                                            .and_then(parse_rfc3339_secs)
                                            .map(|ts| format!("上次: {}", relative_from_now(ts)))
                                            .unwrap_or_default(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id(("sched-del", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("删除")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_sched_task(id.clone(), cx)
                            })),
                    );
                if let Some(res) = t.last_result.as_ref().filter(|r| !r.is_empty()) {
                    card = card.child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("结果: {}", ellipsis(res, 120))),
                    );
                }
                list = list.child(card);
            }
            body = body.child(list);
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(body.overflow_y_scrollbar())
    }

    /// 技能页：顶部采纳统计 + 左列表（搜索）+ 右详情（markdown 正文）。
    ///
    /// 与 Web 版 `SkillsPanel` 同口径：技能是方法论文档，只读；不做「运行」按钮
    /// （执行语义归工具面板与对话里的工具卡）。
    fn render_skills(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.skills_query.trim().to_lowercase();
        let visible: Vec<SkillRow> = self
            .skills
            .iter()
            .filter(|s| {
                query.is_empty()
                    || s.name.to_lowercase().contains(&query)
                    || s.description.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let selected = self
            .skills
            .iter()
            .find(|s| self.skills_selected.as_deref() == Some(s.id.as_str()))
            .cloned();

        // 顶部：累计调用 + 采纳排行（方法论技能无「执行成败」语义，只看频率）
        let mut stats_bar = div()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .flex_wrap();
        if self.skills_total_calls > 0 {
            stats_bar = stats_bar.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(format!(
                        "技能累计调用 {} 次",
                        group_thousands(self.skills_total_calls)
                    )),
            );
            let mut chips = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for st in self.skill_stats.iter().take(8) {
                chips = chips.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_SECONDARY()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(format!("{} {} 次", st.skill_id, st.total_calls)),
                );
            }
            stats_bar = stats_bar.child(chips);
        } else {
            stats_bar = stats_bar.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("暂无技能调用统计——智能体调用技能后，这里会展示采纳排行与平均耗时"),
            );
        }

        // 左：技能列表
        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if visible.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Lightbulb.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(if self.skills.is_empty() {
                                "暂无学习方法论技能"
                            } else {
                                "未找到匹配技能"
                            }),
                    ),
            );
        }
        for (ix, skill) in visible.iter().enumerate() {
            let is_sel = self.skills_selected.as_deref() == Some(skill.id.as_str());
            let id = skill.id.clone();
            let stat = self.skill_stats.iter().find(|s| s.skill_id == skill.id);
            let mut row = div()
                .id(("skill", ix))
                .w_full()
                .px_2()
                .py_2()
                .rounded_lg()
                .bg(rgb(if is_sel {
                    colors::BG_TERTIARY()
                } else {
                    colors::BG_SECONDARY()
                }))
                .cursor_pointer()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(if is_sel {
                            colors::ACCENT()
                        } else {
                            colors::TEXT_PRIMARY()
                        }))
                        .child(skill.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(ellipsis(&skill.description, 52)),
                );
            if let Some(st) = stat.filter(|st| st.total_calls > 0) {
                let mut line = format!(
                    "调用 {} 次 · 均值 {}ms",
                    st.total_calls,
                    st.avg_time_ms.round() as u64
                );
                if st.total_calls > st.success_calls {
                    line.push_str(&format!(
                        " · 执行异常 {} 次",
                        st.total_calls - st.success_calls
                    ));
                }
                row = row.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(line),
                );
            }
            if let Some(t) = &skill.updated_at {
                row = row.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("更新于 {}", short_stamp(t))),
                );
            }
            list = list.child(row.on_click(cx.listener(move |this, _, _, cx| {
                this.skills_selected = Some(id.clone());
                this.skill_content = None;
                this.skills_status = "正在加载技能内容…".to_string();
                this.load_skill_detail(id.clone(), cx);
                cx.notify();
            })));
        }

        // 右：详情
        let detail = match &selected {
            None => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::Lightbulb.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选择一个技能查看详情"),
                )
                .into_any_element(),
            Some(skill) => {
                let content = self.skill_content.clone().unwrap_or_default();
                let mut chips = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_SECONDARY()))
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("v{}", skill.version)),
                    );
                if let Some(t) = &skill.updated_at {
                    chips = chips.child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("更新于 {}", short_stamp(t))),
                    );
                }
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER()))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child(skill.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(skill.description.clone()),
                            )
                            .child(chips),
                    )
                    .child(div().flex_1().min_h_0().p_4().child(if content.is_empty() {
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(
                                if self.skills_status.starts_with("加载技能详情失败")
                                    || self.skills_status.starts_with("正在加载")
                                {
                                    self.skills_status.clone()
                                } else {
                                    "该技能没有可查看的内容".to_string()
                                },
                            )
                            .into_any_element()
                    } else {
                        div()
                            .w_full()
                            .child(TextView::markdown(
                                ("skill-md", 0usize),
                                SharedString::from(content),
                            ))
                            .overflow_y_scrollbar()
                            .into_any_element()
                    }))
                    .into_any_element()
            }
        };

        div().size_full().flex().flex_col().child(stats_bar).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .child(
                    div()
                        .w(px(colors::LIST_WIDTH))
                        .h_full()
                        .flex()
                        .flex_col()
                        .border_r_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_SECONDARY()))
                        .child(
                            div()
                                .px_4()
                                .py_3()
                                .border_b_1()
                                .border_color(rgb(colors::BORDER()))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            svg()
                                                .path(IconName::Lightbulb.path())
                                                .size(px(16.))
                                                .text_color(rgb(colors::TEXT_SECONDARY())),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(15.))
                                                .text_color(rgb(colors::TEXT_PRIMARY()))
                                                .child("技能"),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors::TEXT_TERTIARY()))
                                        .child("自动学习方法论（GEPA 总结），只读查看"),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(colors::BORDER()))
                                        .bg(rgb(colors::BG_PRIMARY()))
                                        .px_2()
                                        .py_1()
                                        .child(Input::new(&self.skills_search).appearance(false)),
                                ),
                        )
                        .child(list.overflow_y_scrollbar()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(detail),
                ),
        )
    }

    /// 工具页：左列表（搜索 + 名称/描述）+ 右详情（参数 JSON Schema）。
    ///
    /// 与 Web 版 `ToolsPanel` 同口径：工具是**原子操作**，与「技能」（方法论）两套概念。
    /// 参数不做表单（工具是给 LLM 调用的，人只读 schema）。
    fn render_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.tools_query.trim().to_lowercase();
        let visible: Vec<ToolRow> = self
            .tools
            .iter()
            .filter(|t| {
                query.is_empty()
                    || t.name.to_lowercase().contains(&query)
                    || t.description.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let selected = self
            .tools
            .iter()
            .find(|t| self.tools_selected.as_deref() == Some(t.name.as_str()))
            .cloned();

        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if visible.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Wrench.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(if self.tools.is_empty() {
                                "暂无可用工具"
                            } else {
                                "未找到匹配工具"
                            }),
                    ),
            );
        }
        for (ix, tool) in visible.iter().enumerate() {
            let is_sel = self.tools_selected.as_deref() == Some(tool.name.as_str());
            let name = tool.name.clone();
            list = list.child(
                div()
                    .id(("tool", ix))
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(if is_sel {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(if is_sel {
                                colors::ACCENT()
                            } else {
                                colors::TEXT_PRIMARY()
                            }))
                            .child(tool.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(ellipsis(&tool.description, 56)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tools_selected = Some(name.clone());
                        cx.notify();
                    })),
            );
        }

        let detail = match &selected {
            None => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::Wrench.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选择一个工具查看详情"),
                )
                .into_any_element(),
            Some(tool) => {
                let schema = serde_json::to_string_pretty(&tool.parameters)
                    .unwrap_or_else(|_| "（参数无法序列化）".to_string());
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER()))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child(tool.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(tool.description.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("参数 Schema"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .bg(rgb(colors::BG_SECONDARY()))
                                    .p_3()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_SECONDARY()))
                                            .child(schema)
                                            .overflow_y_scrollbar(),
                                    ),
                            ),
                    )
                    .into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .w(px(colors::LIST_WIDTH))
                    .h_full()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER()))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::Wrench.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("工具"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!(
                                        "系统内置原子操作（{} 个），LLM 通过 tool call 调用",
                                        self.tools.len()
                                    )),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .bg(rgb(colors::BG_PRIMARY()))
                                    .px_2()
                                    .py_1()
                                    .child(Input::new(&self.tools_search).appearance(false)),
                            ),
                    )
                    .child(list.overflow_y_scrollbar()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(detail),
            )
    }

    fn render_roles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.roles_query.trim().to_lowercase();
        let visible: Vec<RoleRow> = self
            .roles
            .iter()
            .filter(|r| {
                query.is_empty()
                    || r.name.to_lowercase().contains(&query)
                    || r.purpose.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();

        // 左：角色列表
        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if visible.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Bot.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(if self.roles.is_empty() {
                                "暂无角色"
                            } else {
                                "未找到匹配子智能体"
                            }),
                    ),
            );
        } else {
            for (ix, role) in visible.iter().enumerate() {
                list = list.child(self.render_role_item(ix, role, cx));
            }
        }

        // 右：统计条 + 详情
        let mut stats_bar = div()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .flex_wrap()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("累计委托"),
                    )
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(format!("{}", self.roles_stats.total_calls)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("成功率"),
                    )
                    .child(
                        div()
                            .text_color(rgb(if self.roles_stats.success_rate < 0.5 {
                                colors::ERROR()
                            } else {
                                colors::SUCCESS()
                            }))
                            .child(format!(
                                "{}%",
                                (self.roles_stats.success_rate * 100.0).round() as u32
                            )),
                    ),
            );
        if !self.roles_stats.by_task_type.is_empty() {
            let mut chips = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_wrap()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("任务类型："),
                );
            for (kind, count) in &self.roles_stats.by_task_type {
                chips = chips.child(
                    div()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_SECONDARY()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(format!("{kind} {count}")),
                );
            }
            stats_bar = stats_bar.child(chips);
        }
        if !self.roles_stats.by_role.is_empty() {
            let mut chips = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_wrap()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("按角色："),
                );
            for (name, calls) in &self.roles_stats.by_role {
                chips = chips.child(
                    div()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_SECONDARY()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(format!("{name} {calls}")),
                );
            }
            stats_bar = stats_bar.child(chips);
        }

        let detail = match self
            .roles_selected
            .as_ref()
            .and_then(|name| self.roles.iter().find(|r| &r.name == name))
        {
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    svg()
                        .path(IconName::Bot.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选择一个子智能体查看详情"),
                )
                .into_any_element(),
            Some(role) => {
                let (source_label, source_color) = source_of(&role.source);
                let card = div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_6()
                            .py_4()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER()))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child(role.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_full()
                                            .border_1()
                                            .border_color(rgb(source_color))
                                            .text_size(px(11.))
                                            .text_color(rgb(source_color))
                                            .child(source_label),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY()))
                                            .child(format!("v{}", role.version)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY()))
                                            .child(match role.tool_count {
                                                Some(n) => format!("{n} 工具"),
                                                None => "工具不限".to_string(),
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY()))
                                            .child(format!(
                                                "模型：{}",
                                                role.model
                                                    .clone()
                                                    .unwrap_or_else(|| "回落主 Agent".to_string())
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(role.purpose.clone()),
                            ),
                    )
                    // 操作栏（对齐 Web 版 RolesPanel）：回退内置种子 / 退役删除
                    .child(self.render_role_actions(role, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child("系统提示"),
                            )
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .bg(rgb(colors::BG_SECONDARY()))
                                    .p_3()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(match &self.role_detail {
                                        Some(d) if !d.system_prompt.is_empty() => {
                                            d.system_prompt.clone()
                                        }
                                        Some(_) => "（空）".to_string(),
                                        None => "加载中…".to_string(),
                                    }),
                            )
                            .children(self.role_detail.as_ref().map(|d| {
                                if d.tools.is_empty() {
                                    div().into_any_element()
                                } else {
                                    let mut chips = div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_1()
                                        .flex_wrap()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                                .child("工具白名单"),
                                        );
                                    for tool in &d.tools {
                                        chips = chips.child(
                                            div()
                                                .px_1()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(rgb(colors::BORDER()))
                                                .text_size(px(11.))
                                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                                .child(tool.clone()),
                                        );
                                    }
                                    chips.into_any_element()
                                }
                            }))
                            .overflow_y_scrollbar(),
                    );
                card.into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .w(px(320.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER()))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::Users.path())
                                            .size(px(18.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("子智能体"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child("角色化分工（内置 / 配置 / 学习三源平级），自演化更新"),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .bg(rgb(colors::BG_PRIMARY()))
                                    .px_2()
                                    .py_1()
                                    .child(Input::new(&self.roles_search).appearance(false)),
                            ),
                    )
                    .child(list.overflow_y_scrollbar()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(stats_bar)
                    .child(detail),
            )
    }

    /// 角色操作栏：回退内置种子 / 退役删除（都走确认弹窗）。
    ///
    /// 「回退」在 `builtin && version == 1` 时禁用 —— 内置种子本来就是 v1，
    /// 没有可回退的内容（Web 版同口径）。
    /// 目录选择器弹窗：当前路径 + 逐级浏览 + 「选择此目录」。
    ///
    /// 浏览根（盘符列表）不允许直接选中 —— 后端要求的是具体目录。
    /// 文件选择弹窗（**GPUI 自绘**，不是 OS 对话框）：点目录进入、点文件勾选。
    ///
    /// 形态与 workspace 目录选择器一致（遮罩 + 居中卡片 + 列表），因为它们是同一类
    /// 任务（在文件系统里定位）。弹窗而不是平铺：导入 tab 的主任务只有「选文件 +
    /// 导入」，把一张常驻的文件列表铺在页面上会把主次关系拉平。
    /// 文件选择弹窗（知识导入 / @ 引用共用）。
    ///
    /// 两种用途的差别全部集中在本函数的三处 `purpose` 分支上：标题、点文件的
    /// 动作、底部按钮组。其余（上级目录 / 过滤 / 隐藏项 / 大小）逐字共用。
    fn render_file_picker_dialog(
        &self,
        purpose: FilePickPurpose,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // 列表：上级目录 → 子目录 → 文件
        let mut list = div().flex().flex_col().gap_1().w_full();
        if !self.kb_browse_dir.is_empty() && self.kb_browse_dir != "浏览根" {
            let parent = std::path::Path::new(&self.kb_browse_dir)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            list = list.child(
                div()
                    .id("kbp-up")
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(
                        svg()
                            .path(IconName::CornerLeftUp.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child("上级目录")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_kb_browse(parent.clone(), cx);
                    })),
            );
        }
        if self.kb_browse_entries.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .w_full()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(if self.kb_browse_status.is_empty() {
                        "（空目录）"
                    } else {
                        "（空目录或读取中）"
                    }),
            );
        }
        // 遇到第一个文件时插一个「文件」分隔标题：目录总在前，没有分隔时
        // 用户看不出「下面还有文件」（截图里第一屏全是目录就是这个感受）。
        let q = self.kb_pick_query.trim().to_lowercase();
        let mut files_label_drawn = false;
        for (ix, e) in self.kb_browse_entries.iter().enumerate() {
            // 默认隐藏点开头的条目：项目根目录动辄十几个 `.git`/`.cargo`/`.vscode`，
            // 第一屏全被它们占满，真正要选的文件得滚好几屏（截图实测过）。
            if !self.kb_show_hidden && e.name.starts_with('.') {
                continue;
            }
            // 过滤词：只在「按名字」上匹配（选文件的场景里，路径信息没用）
            if !q.is_empty() && !e.name.to_lowercase().contains(&q) {
                continue;
            }
            if !e.is_dir && !files_label_drawn {
                files_label_drawn = true;
                list = list.child(
                    div()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("文件"),
                );
            }
            let is_picked = self.kb_picked.iter().any(|p| p == &e.path);
            let dir = e.path.clone();
            let is_dir = e.is_dir;
            let picked_path = e.path.clone();
            let icon = if is_dir {
                IconName::Folder
            } else {
                IconName::File
            };
            let size_text = e.size.map(format_size).unwrap_or_default();
            list =
                list.child(
                    div()
                        .id(("kbp-item", ix))
                        .w_full()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(if is_picked {
                            colors::BG_TERTIARY()
                        } else {
                            colors::BG_SECONDARY()
                        }))
                        .cursor_pointer()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(svg().path(icon.path()).size(px(12.)).text_color(rgb(
                            if is_picked {
                                colors::ACCENT()
                            } else {
                                colors::TEXT_TERTIARY()
                            },
                        )))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .overflow_hidden()
                                .text_size(px(12.))
                                .text_color(rgb(if is_picked {
                                    colors::ACCENT()
                                } else {
                                    colors::TEXT_SECONDARY()
                                }))
                                .child(e.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(size_text),
                        )
                        .child(
                            // 目录不给勾选位（点它就是进入），文件给一个明确的勾选标记
                            div()
                                .w(px(16.))
                                .flex()
                                .justify_center()
                                .text_size(px(11.))
                                .text_color(rgb(if is_picked {
                                    colors::SUCCESS()
                                } else {
                                    colors::TEXT_TERTIARY()
                                }))
                                .child(if is_dir {
                                    ""
                                } else if is_picked {
                                    "✓"
                                } else {
                                    "○"
                                }),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if is_dir {
                                this.load_kb_browse(dir.clone(), cx);
                            } else if purpose == FilePickPurpose::Mention {
                                // @ 引用单选：点中即插入并关闭，不要「勾选 + 确认」两步
                                this.mention_file(picked_path.clone(), window, cx);
                            } else {
                                this.toggle_kb_pick(picked_path.clone());
                                cx.notify();
                            }
                        })),
                );
        }

        let card =
            div()
                .w(px(680.))
                .max_h(px(620.))
                .rounded_xl()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .flex()
                .flex_col()
                // 标题行
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(rgb(colors::BORDER()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            svg()
                                .path(IconName::FolderOpen.path())
                                .size(px(16.))
                                .text_color(rgb(colors::TEXT_SECONDARY())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(14.))
                                .text_color(rgb(colors::TEXT_PRIMARY()))
                                .child(match purpose {
                                    FilePickPurpose::KbIngest => "选择要导入知识库的文件",
                                    FilePickPurpose::Mention => "选择要引用的文件（点中即插入）",
                                }),
                        )
                        .child(
                            div()
                                .id("kbp-close")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child("✕")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                })),
                        ),
                )
                // 路径行
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(colors::BORDER()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .overflow_hidden()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(if self.kb_browse_dir.is_empty() {
                                    "浏览根".to_string()
                                } else {
                                    self.kb_browse_dir.clone()
                                }),
                        )
                        .child(
                            div()
                                .id("kbp-root")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::ACCENT()))
                                .child("回到根")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.load_kb_browse(String::new(), cx)
                                })),
                        ),
                )
                // 隐藏项开关（默认隐藏，见列表循环里的过滤）
                .child(
                    div()
                        .px_4()
                        .pb_2()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(colors::BORDER()))
                                .bg(rgb(colors::BG_PRIMARY()))
                                .px_2()
                                .py_1()
                                .child(Input::new(&self.kb_pick_search).appearance(false)),
                        )
                        .child(
                            div()
                                .id("kbp-hidden")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if self.kb_show_hidden {
                                    colors::ACCENT()
                                } else {
                                    colors::BORDER()
                                }))
                                .cursor_pointer()
                                .text_size(px(10.))
                                .text_color(rgb(if self.kb_show_hidden {
                                    colors::ACCENT()
                                } else {
                                    colors::TEXT_TERTIARY()
                                }))
                                .child(if self.kb_show_hidden {
                                    "隐藏项：显示"
                                } else {
                                    "隐藏项：隐藏"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.kb_show_hidden = !this.kb_show_hidden;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(if self.kb_browse_status.is_empty() {
                                    String::new()
                                } else {
                                    self.kb_browse_status.clone()
                                }),
                        ),
                )
                // 列表（确定高度 + 自己滚动）
                .child(
                    div()
                        .h(px(400.))
                        .min_h_0()
                        .px_3()
                        .py_2()
                        .child(list.overflow_y_scrollbar()),
                )
                // 底部：已选 + 清空 / 取消 / 导入
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(rgb(colors::BORDER()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .overflow_hidden()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(match purpose {
                                    FilePickPurpose::Mention => {
                                        "点目录进入，点文件即插入引用".to_string()
                                    }
                                    FilePickPurpose::KbIngest => {
                                        if self.kb_picked.is_empty() {
                                            "点目录进入，点文件勾选".to_string()
                                        } else {
                                            format!("已选 {} 个文件", self.kb_picked.len())
                                        }
                                    }
                                }),
                        )
                        // 「清空 / 导入选中」只属于多选的导入用途；引用是单选即插入，
                        // 留着这两个按钮只会让人以为要先勾选
                        .children(if purpose == FilePickPurpose::KbIngest {
                            Some(
                                div()
                                    .id("kbp-clear")
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("清空")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.kb_picked.clear();
                                        cx.notify();
                                    })),
                            )
                        } else {
                            None
                        })
                        .child(
                            div()
                                .id("kbp-cancel")
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(colors::BORDER()))
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .child("取消")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                })),
                        )
                        .children(if purpose == FilePickPurpose::KbIngest {
                            Some(
                                div()
                                    .id("kbp-confirm")
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(colors::ACCENT()))
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child("导入选中")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.import_kb_picked(cx);
                                        this.dialog = None;
                                        cx.notify();
                                    })),
                            )
                        } else {
                            None
                        }),
                );

        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0))
            .child(card)
            .into_any_element()
    }

    fn render_workspace_picker_dialog(
        &self,
        target: PickerTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div().flex().flex_col().gap_1().w_full();
        if let Some(parent) = self.picker_parent.clone() {
            list = list.child(
                div()
                    .id("picker-up")
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(
                        svg()
                            .path(IconName::CornerLeftUp.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child("上级目录")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_picker_dirs(parent.clone(), cx);
                    })),
            );
        }
        for (ix, (name, path)) in self.picker_entries.iter().enumerate() {
            let path = path.clone();
            list = list.child(
                div()
                    .id(("picker-dir", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(
                        svg()
                            .path(IconName::Folder.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(name.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_picker_dirs(path.clone(), cx);
                    })),
            );
        }
        if self.picker_entries.is_empty() && self.picker_parent.is_none() {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(self.picker_status.clone()),
            );
        }

        let can_pick = !self.picker_current.is_empty() && self.picker_current != "浏览根";
        let pick_label = match target {
            PickerTarget::NewSession => "用它新建会话",
            PickerTarget::Session => "绑定到当前会话",
        };

        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0))
            .child(
                div()
                    .w(px(560.))
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(15.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("选择工作目录"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(self.picker_current.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(self.picker_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            // 必须给**确定高度**：`overflow_y_scrollbar()` 内部按父高布局，
                            // 父容器只有 `max_h` 时高度会塌成 0（实测列表区只留一条空白）。
                            .h(px(300.))
                            .min_h_0()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_PRIMARY()))
                            .p_2()
                            .child(list.overflow_y_scrollbar()),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("picker-cancel")
                                    .px_3()
                                    .py_1()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dialog = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("picker-confirm")
                                    .px_3()
                                    .py_1()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .text_sm()
                                    .bg(rgb(if can_pick {
                                        colors::ACCENT()
                                    } else {
                                        colors::BG_TERTIARY()
                                    }))
                                    .text_color(rgb(if can_pick {
                                        0xffffff
                                    } else {
                                        colors::TEXT_TERTIARY()
                                    }))
                                    .child(pick_label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if can_pick {
                                            this.confirm_workspace_picker(target, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_role_actions(&self, role: &RoleRow, cx: &mut Context<Self>) -> AnyElement {
        let reset_name = role.name.clone();
        let delete_name = role.name.clone();
        let reset_disabled = role.source == "builtin" && role.version <= 1;
        let reset_color = if reset_disabled {
            colors::TEXT_TERTIARY()
        } else {
            colors::TEXT_SECONDARY()
        };
        div()
            .px_6()
            .py_2()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("role-reset")
                    .px_3()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(reset_color))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(IconName::RotateCcw.path())
                            .size(px(12.))
                            .text_color(rgb(reset_color)),
                    )
                    .child("回退内置种子")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if reset_disabled {
                            return;
                        }
                        this.dialog = Some(Dialog::ResetRole {
                            name: reset_name.clone(),
                        });
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("role-delete")
                    .px_3()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::ERROR()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(colors::ERROR()))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(IconName::Trash.path())
                            .size(px(12.))
                            .text_color(rgb(colors::ERROR())),
                    )
                    .child("退役删除")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.dialog = Some(Dialog::DeleteRole {
                            name: delete_name.clone(),
                        });
                        cx.notify();
                    })),
            )
            .children(self.roles_action_msg.clone().map(|m| {
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(m)
            }))
            .into_any_element()
    }

    /// 角色列表项：名称 + 来源徽章 + 职责 + 版本 / 工具数 / 成功率。
    fn render_role_item(&self, ix: usize, role: &RoleRow, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.roles_selected.as_deref() == Some(role.name.as_str());
        let (source_label, source_color) = source_of(&role.source);
        let name = role.name.clone();
        let usage_text = role
            .usage
            .filter(|(calls, _)| *calls > 0)
            .map(|(calls, rate)| {
                (
                    format!("成功率 {}%", (rate * 100.0).round() as u32),
                    rate < 0.5,
                    calls,
                )
            });
        div()
            .id(("role", ix))
            .w_full()
            .px_2()
            .py_2()
            .rounded_lg()
            .bg(rgb(if selected {
                colors::BG_TERTIARY()
            } else {
                colors::BG_SECONDARY()
            }))
            .cursor_pointer()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(13.))
                            .text_color(rgb(if selected {
                                colors::ACCENT()
                            } else {
                                colors::TEXT_PRIMARY()
                            }))
                            .child(role.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(source_color))
                            .text_size(px(10.))
                            .text_color(rgb(source_color))
                            .child(source_label),
                    )
                    .child(if role.status == "experimental" {
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(colors::WARNING()))
                            .text_size(px(10.))
                            .text_color(rgb(colors::WARNING()))
                            .child("试验性")
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    }),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .overflow_hidden()
                    .child(role.purpose.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(10.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(format!("v{}", role.version))
                    .child(match role.tool_count {
                        Some(n) => format!("{n} 工具"),
                        None => "工具不限".to_string(),
                    })
                    .children(usage_text.map(|(text, low, calls)| {
                        div()
                            .text_color(rgb(if low {
                                colors::ERROR()
                            } else {
                                colors::TEXT_TERTIARY()
                            }))
                            .child(format!("{text}（{calls} 次）"))
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.roles_selected = Some(name.clone());
                this.role_detail = None;
                this.load_role_detail(name.clone(), cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 外观：三套色板（石墨灰 / 羊皮纸 / 午夜蓝）。切换只改全局色板 + 组件库
        // 的 ThemeMode，随后 `cx.notify()` 全量重绘 —— 所有颜色都是**渲染时**读的，
        // 没有任何缓存，所以切完立刻生效。
        let mut theme_block = div().flex().flex_col().gap_2().px_4().pt_3().child(
            div()
                .text_xs()
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child("外观"),
        );
        let mut theme_cards = div().flex().flex_row().gap_2().flex_wrap();
        for kind in ThemeKind::ALL {
            let is_cur = current_theme() == kind;
            let p = kind.palette();
            theme_cards = theme_cards.child(
                div()
                    .id(("theme", kind as usize))
                    .w(px(208.))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(if is_cur {
                        colors::ACCENT()
                    } else {
                        colors::BORDER()
                    }))
                    .bg(rgb(p.bg_secondary))
                    .cursor_pointer()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    // 三块色预览：页面底 / 强调块 / 强调色
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .child(div().flex_1().h(px(20.)).rounded_md().bg(rgb(p.bg_primary)))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(20.))
                                    .rounded_md()
                                    .bg(rgb(p.bg_tertiary)),
                            )
                            .child(div().flex_1().h(px(20.)).rounded_md().bg(rgb(p.accent))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.text_primary))
                                    .child(kind.label()),
                            )
                            .child(if is_cur {
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(p.accent))
                                    .child("✓ 当前")
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(p.text_tertiary))
                            .child(kind.hint()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_theme_with(kind, Some(window), cx);
                    })),
            );
        }
        theme_block = theme_block.child(theme_cards);
        // 提供商区：endpoint / 密钥可编辑（保存时先重读配置再写，见 `save_provider`）
        let mut providers_block = div().flex().flex_col().gap_1().px_4().pt_3().child(
            div()
                .text_xs()
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child("提供商（端点 / 密钥）"),
        );
        for (ix, p) in self.providers.iter().enumerate() {
            let name = p.name.clone();
            providers_block = providers_block.child(
                div()
                    .id(("prov", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .child(div().w(px(110.)).text_xs().child(p.name.clone()))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(ellipsis(&p.endpoint, 44)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{} 模型", p.model_count)),
                    )
                    .child(
                        div()
                            .id(("prov-edit", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::BG_TERTIARY()))
                            .cursor_pointer()
                            .text_xs()
                            .child("编辑")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_provider_editor(name.clone(), window, cx);
                            })),
                    ),
            );
        }
        if let Some(name) = self.editing_provider.clone() {
            providers_block = providers_block.child(
                div()
                    .mt_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(format!("编辑 {name}")),
                    )
                    .child(Textarea::new(&self.edit_endpoint).h(px(40.)))
                    .child(Textarea::new(&self.edit_api_key).h(px(40.)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                div()
                                    .id("prov-test")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::BG_TERTIARY()))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("测试连接")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.test_provider_connection(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("prov-save")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::ACCENT()))
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("保存")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.save_provider(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("prov-cancel")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child("取消")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.editing_provider = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        let mut list = div().flex().flex_col().gap_1().p_4();
        if self.models.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("暂无模型：请先在配置里添加 provider 与模型"),
            );
        }
        for (ix, m) in self.models.iter().enumerate() {
            let provider = m.provider.clone();
            let name = m.name.clone();
            let meta = {
                let mut parts: Vec<String> = Vec::new();
                if !m.capabilities.is_empty() {
                    parts.push(m.capabilities.clone());
                }
                if let Some(ctx_len) = m.context {
                    parts.push(format!("ctx {ctx_len}"));
                }
                parts.join(" · ")
            };
            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md();
            if m.is_chat {
                row = row.bg(rgb(colors::BG_HOVER()));
            }
            let action = if m.is_chat {
                div()
                    .text_xs()
                    .text_color(rgb(colors::SUCCESS()))
                    .child("当前聊天模型")
                    .into_any_element()
            } else {
                div()
                    .id(("set-chat", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(colors::BG_TERTIARY()))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .cursor_pointer()
                    .child("设为聊天模型")
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.switch_chat_model(provider.clone(), name.clone(), cx);
                    }))
                    .into_any_element()
            };
            list = list.child(
                row.child(div().w(px(110.)).text_xs().child(m.provider.clone()))
                    .child(div().flex_1().child(m.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(meta),
                    )
                    .child(action),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .child(div().text_sm().child("设置 · 模型"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(if self.settings_error {
                                rgb(colors::ERROR())
                            } else {
                                rgb(colors::TEXT_SECONDARY())
                            })
                            .child(self.settings_status.clone()),
                    ),
            )
            .child(theme_block)
            .child(providers_block)
            .child(div().flex_1().min_h_0().child(list))
    }

    /// 显式停止当前轮：`POST /chat/streams/{id}/cancel`（ADR-036 的结构性取消
    /// 在服务端置位，客户端断线**不**取消——本地助手跑完再取）。
    fn stop_current_turn(&mut self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        self.status = "正在停止…".to_string();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/streams/{sid}/cancel");
            let res = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("已请求停止 · {sid}"),
                    Err(e) => format!("停止失败：{e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 选中会话：切换路由键并拉消息
    fn open_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.current_session = Some(id.clone());
        // 浏览器式会话 Tab：打开过的会话留在顶部（去重；超上限丢最早且非当前的）
        if !self.open_tabs.iter().any(|t| t == &id) {
            self.open_tabs.push(id.clone());
            while self.open_tabs.len() > MAX_SESSION_TABS {
                let victim = self
                    .open_tabs
                    .iter()
                    .find(|t| **t != id)
                    .cloned()
                    .unwrap_or_else(|| self.open_tabs[0].clone());
                self.open_tabs.retain(|t| t != &victim);
            }
        }
        // 切会话即放弃上一会话的流式状态（本客户端只渲染当前会话的增量）
        self.streaming = false;
        // 会话级模型 / 上下文进度都是**按会话**的：切走即回落（模型回到 preferences.chat）
        self.session_model = None;
        self.model_menu_open = false;
        self.thinking = None;
        self.thinking_menu_open = false;
        // 实时用量与面板状态按会话重置；占用本身由新会话的消息 usage 恢复
        self.ctx_live = None;
        self.ctx_window_live = 0;
        self.ctx_panel_open = false;
        self.ctx_compress = None;
        // 统计行是按会话的：切走即清空，由新会话的两次 GET 重新填
        self.session_usage = None;
        self.request_perf = None;
        // 会话语义的面板数据先清空（避免看着新会话却显示上一条的待办/审批），
        // 再**立即**拉一次 —— 否则要等 3s 轮询窗口，体感就是"切过去慢一拍"
        self.todos.clear();
        self.goals.clear();
        self.pending_approvals.clear();
        // 文件页目录树按会话绑定目录解析：切会话必须作废，否则会继续显示
        // 上一条会话（或全局默认）的目录
        self.ws_root.clear();
        self.ws_entries.clear();
        self.ws_file = None;
        self.ws_path.clear();
        // 会话级模型 / 思考强度的权威值在会话头里：拉回来覆盖上面对默认值的回落
        self.load_session_model(id.clone(), cx);
        self.load_messages(id, cx);
        self.load_session_usage(cx);
        self.load_todos_goals(cx);
        self.load_approvals(cx);
        // 选中态与「当前会话始终可见」都体现在左栏行上
        self.rebuild_session_rows(cx);
    }

    /// 关闭一个会话 Tab：若是当前会话则切到剩余 tab 的最后一个（没有则回空态）。
    fn close_tab(&mut self, id: String, cx: &mut Context<Self>) {
        self.open_tabs.retain(|t| t != &id);
        if self.current_session.as_deref() == Some(id.as_str()) {
            match self.open_tabs.last().cloned() {
                Some(next) => self.open_session(next, cx),
                None => self.start_new_session(cx),
            }
        } else {
            cx.notify();
        }
    }

    /// 顶部会话 Tab 条（对齐 vite `SessionTabBar`）：打开的会话横向排列、当前项高亮、
    /// 每项带关闭按钮；没有任何 tab 时退回原来的单行标题（不占额外高度）。
    ///
    /// 单独成方法：`render_chat` 的栈帧已经很紧（GPUI 递归 prepaint，
    /// 单帧过大会 `has overflowed its stack`），这里不再往它体内塞嵌套。
    fn render_session_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.open_tabs.is_empty() {
            return div()
                .px_3()
                .py_2()
                .text_sm()
                .border_b_1()
                .border_color(rgb(colors::BORDER()))
                .child(self.current_title.clone())
                .into_any_element();
        }
        let mut row = div()
            .w_full()
            .min_w_0()
            .px_2()
            .py_1()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .overflow_hidden();
        for id in &self.open_tabs {
            let is_cur = self.current_session.as_deref() == Some(id.as_str());
            let title = self
                .sessions_data
                .iter()
                .find(|s| &s.id == id)
                .map(|s| ellipsis(&s.title, 12))
                .unwrap_or_else(|| "会话".to_string());
            let id_open = id.clone();
            let id_close = id.clone();
            row = row.child(
                div()
                    .id(format!("session-tab-{id}"))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .whitespace_nowrap()
                    .bg(rgb(if is_cur {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .text_size(px(12.))
                    .text_color(rgb(if is_cur {
                        colors::TEXT_PRIMARY()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(title)
                    .child(
                        div()
                            .id(format!("session-tab-x-{id}"))
                            .px_1()
                            .rounded_md()
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::X.path())
                                    .size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY())),
                            )
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.close_tab(id_close.clone(), cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _w, cx| {
                        if this.current_session.as_deref() != Some(id_open.as_str()) {
                            this.open_session(id_open.clone(), cx);
                        }
                    })),
            );
        }
        row.into_any_element()
    }

    /// 处理一条统一事件。
    ///
    /// 只认 `type=chat_stream` 且只认当前会话（子代理事件的 `session_id` 是
    /// `task_id`，属于任务面板，后续接入）。`chunk_type` 语义见
    /// `docs/architecture/event-protocol.md` §4。
    fn handle_event(&mut self, ev: &serde_json::Value, cx: &mut Context<Self>) {
        if ev.get("type").and_then(|v| v.as_str()) != Some("chat_stream") {
            return;
        }
        let sid = ev
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if Some(sid) != self.current_session.as_deref() {
            return;
        }
        match ev
            .get("chunk_type")
            .and_then(|v| v.as_str())
            .unwrap_or("answer")
        {
            "answer" => {
                let delta = ev.get("delta").and_then(|v| v.as_str()).unwrap_or_default();
                self.append_delta(delta);
                // 上下文占用（`ContextRing` 的数据源）：usage 随 answer 块下发。
                // 分子取 `prompt_tokens`（本轮上下文）——`total_tokens` 含输出，
                // 当占用会偏高；分母优先用模型声明的窗口，这里只记流式兜底值。
                if let Some(u) = ev.get("usage") {
                    let prompt = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    if prompt > 0 {
                        self.ctx_live = Some(MsgUsage {
                            prompt_tokens: prompt,
                            completion_tokens: u
                                .get("completion_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                            cache_read: u.get("cache_read").and_then(|v| v.as_u64()).unwrap_or(0),
                        });
                    }
                    if let Some(win) = u.get("context_window").and_then(|v| v.as_u64()) {
                        self.ctx_window_live = win;
                    }
                }
            }
            // 思考增量暂不并入正文（会污染 Markdown 结构），待独立渲染区
            "thought" => {}
            "tool_call" => {
                let name = ev
                    .get("tool_call")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("工具");
                self.append_delta(&format!("\n`🔧 {name}`\n"));
                // ask_user 是**同步工具**：工具执行挂起等回答，输入区要被追问接管
                if name == "ask_user" {
                    if let Some(args) = ev.get("tool_call").and_then(|v| v.get("arguments")) {
                        let questions = parse_ask_user(args);
                        if !questions.is_empty() {
                            self.clarify_answers = vec![String::new(); questions.len()];
                            self.clarification = Some(questions);
                        }
                    }
                }
            }
            "observation" => {}
            // 消息边界：流末尾的 assistant 完整消息（含工具段）为权威全文，替换占位
            "message" => {
                if let Some(msg) = ev.get("message") {
                    if msg.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                        let full = segments_text(msg);
                        self.replace_last(&full);
                    }
                }
            }
            "error" => {
                let msg = ev
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知错误");
                self.streaming = false;
                self.status = format!("出错：{msg}");
                self.append_delta(&format!("\n\n> ⚠️ {msg}\n"));
            }
            "turn_state" => {
                let state = ev
                    .get("turn_state")
                    .and_then(|v| v.get("state"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                self.streaming = state == "running";
                if !self.streaming {
                    self.status = format!("空闲 · {sid}");
                    // 本轮结束：刷新底部统计行（token 汇总 + 最近一次请求性能）
                    self.load_session_usage(cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// 往最后一条 assistant 消息追加增量（虚拟列表 count 不变，只重测末项）
    fn append_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let Some(last) = self.messages.last() else {
            return;
        };
        if last.role != "assistant" {
            return;
        }
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        let merged = format!("{}{}", next[idx].text, delta);
        next[idx].text = merged.into();
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.msg_list_state.scroll_to_end();
    }

    /// 用权威全文替换最后一条 assistant 消息
    fn replace_last(&mut self, full: &str) {
        if self.messages.last().map(|m| m.role.as_str()) != Some("assistant") {
            return;
        }
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        next[idx].text = full.to_string().into();
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.msg_list_state.scroll_to_end();
    }

    /// 发送输入框内容（Enter 或「发送」按钮）
    fn send_current_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.send_text(text, cx);
    }

    /// 发送一段文本：`POST /chat/stream` 只是开关（ADR-028），输出走常驻 SSE。
    fn send_text(&mut self, text: String, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        // 待发送图片随本条消息一起走（发送即清空草稿）
        let pending_images = std::mem::take(&mut self.composer_images);
        // 新会话：id 必须在发起前确定（事件按 session_id 路由）
        let session_id = match self.current_session.clone() {
            Some(sid) => sid,
            None => {
                let sid = new_session_id();
                self.current_session = Some(sid.clone());
                sid
            }
        };

        let mut next: Vec<MsgRow> = (*self.messages).clone();
        next.push(MsgRow {
            role: "user".to_string(),
            text: text.clone().into(),
            segs: Vec::new(),
            tool_calls: Vec::new(),
            compression: false,
            usage: None,
            // 本地占位直接带图：发送后立刻能看到自己贴的图（历史重载后仍可见）
            images: pending_images.clone(),
            // 用户消息发送即打点（vite 同样在本地取时间；后端 `timestamp` 恒空）
            ts: Some(now_clock()),
            // 本地占位：尚未落库，无服务端 id（回退入口据此隐藏）
            id: String::new(),
        });
        next.push(MsgRow {
            role: "assistant".to_string(),
            text: "".into(),
            segs: Vec::new(),
            tool_calls: Vec::new(),
            compression: false,
            usage: None,
            images: Vec::new(),
            // 与 vite `startNewAssistantTurn` 同口径：本轮 assistant 开始时打点
            ts: Some(now_clock()),
            id: String::new(),
        });
        if next.len() > RENDER_LIMIT {
            let drop_n = next.len() - RENDER_LIMIT;
            next.drain(..drop_n);
        }
        self.messages = Rc::new(next);
        self.msg_list_state.reset(self.messages.len());
        self.msg_list_state.scroll_to_end();
        self.message_total = self.messages.len();
        self.streaming = true;
        self.status = format!("生成中… · {session_id}");

        let port = self.port;
        // 图片走 `message.images`（data URL 数组；后端 `CoreMessage::user_with_images`）
        let mut message = serde_json::json!({ "role": "user", "content": text });
        if !pending_images.is_empty() {
            message["images"] = serde_json::json!(pending_images);
        }
        let mut body = serde_json::json!({
            "session_id": session_id,
            "message": message,
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 2048
        });
        // 会话级模型：随首条消息一起下发——服务端建会话时就固化，不必等会话 id 生成后再补 PUT
        if let Some((provider, model)) = &self.session_model {
            body["model"] = serde_json::Value::String(model.clone());
            body["provider"] = serde_json::Value::String(provider.clone());
        }
        // 「＋新目录」设置的工作目录：只作用于这一条消息——服务端建会话时固化，
        // 之后的消息再带就没意义了（所以 `take()` 掉）。
        if let Some(dir) = self.new_session_workspace.take() {
            body["working_directory"] = serde_json::Value::String(dir);
        }
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/stream");
            match post_json(url, body).await {
                Ok(_) => {
                    // 新会话首轮：刷新左侧列表把新会话显示出来
                    let _ = this.update(cx, |this, cx| {
                        this.load_sessions(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.streaming = false;
                        this.status = format!("发送失败：{e}");
                        this.append_delta(&format!("\n\n> ⚠️ 发送失败：{e}\n"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn load_messages(&mut self, session_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        println!("[消息] 加载会话 {session_id}");
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/sessions/{session_id}/messages?limit=200"
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let arr = body
                        .get("messages")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let rows: Vec<MsgRow> = arr
                        .iter()
                        .map(|m| {
                            let (segs, tool_calls) = parse_segments(m);
                            MsgRow {
                                role: m
                                    .get("role")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("?")
                                    .to_string(),
                                text: segments_text(m).into(),
                                segs,
                                tool_calls,
                                compression: m
                                    .get("compression_marker")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false),
                                usage: parse_msg_usage(m),
                                images: parse_msg_images(m),
                                ts: parse_msg_ts(m),
                                id: m
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                            }
                        })
                        .collect();
                    let n = rows.len();
                    println!("[消息] 拉到 {n} 条");
                    // 性能：只渲染最近 RENDER_LIMIT 条（Markdown 布局成本高，
                    // 全量渲染 90+ 条会明显卡顿）。历史加载留待后续做上滚分页。
                    let total = n;
                    let kept: Vec<MsgRow> = if n > RENDER_LIMIT {
                        rows.into_iter().skip(n - RENDER_LIMIT).collect()
                    } else {
                        rows
                    };
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.messages = Rc::new(kept);
                            // 条数变了必须重建 list state（item_count 在构造时固定）
                            this.msg_list_state = GpuiListState::new(
                                this.messages.len(),
                                ListAlignment::Bottom,
                                px(120.),
                            );
                            this.message_total = total;
                            this.current_title = if total > RENDER_LIMIT {
                                format!("会话消息（共 {total} 条，显示最近 {RENDER_LIMIT} 条）")
                            } else {
                                format!("会话消息（{total} 条）")
                            };
                            cx.notify();
                        });
                    });
                }
                Err(e) => {
                    println!("[消息] 拉取失败: {e}");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.current_title = format!("加载失败：{e}");
                            cx.notify();
                        });
                    });
                }
            }
        })
        .detach();
    }
}

impl ClientApp {
    /// 会话视图骨架：左栏会话列表 + 右栏聊天区 + 右侧任务面板。
    ///
    /// 三段**平级**挂在 `flex_row` 根节点上——任务面板是第三列，不是右栏的子节点
    /// （`ed1b8d7` 修过的挂载点；动这里先跑括号栈快照，编译通过不代表挂对了父节点）。
    ///
    /// 拆成多个 render 子函数不是为了整洁：这段曾是 810 行单函数，单帧栈占用把
    /// GPUI 的递归 prepaint 推过栈上限（`has overflowed its stack`）——加一个控件
    /// 就可能崩。新增控件请挂进对应的子函数，不要往这里堆。
    /// 布局对齐 `gui-vite` 的 `SessionPage`（左栏 280px + 右栏 ChatPanel 撑满）。
    fn render_chat(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(self.render_chat_sidebar(cx))
            .child(self.render_chat_main(cx))
            .child(self.render_tasks_panel(cx))
            .into_any_element()
    }

    /// 左栏：头部（状态行 + 新建会话 / 新建目录）+ 分组列表 + 底部文件视图入口。
    fn render_chat_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w(px(colors::LIST_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(colors::BG_SECONDARY()))
            .border_r_1()
            .border_color(rgb(colors::BORDER()))
            // 头部：状态行 + 新建会话（对齐 Tauri 版 `SessionList` 头部；
            // 「＋ 新目录」要目录选择器弹窗，留到 workspace 页一起做）
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(self.status.clone()),
                    )
                    .child(
                        div()
                            .id("new-session")
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::Plus.path())
                                    .size(px(14.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_new_session(cx);
                            })),
                    )
                    // 「＋新目录」：新建一个绑定指定工作目录的会话（目录在
                    // 首条消息时随 `working_directory` 下发并固化）
                    .child(
                        div()
                            .id("new-workspace")
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::FolderPlus.path())
                                    .size(px(14.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                let _ = window;
                                this.open_workspace_picker(PickerTarget::NewSession, cx);
                            })),
                    ),
            )
            // 分组列表（gpui-component `List`：自带虚拟化与滚动）
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(
                        (0..self.session_rows.len())
                            .map(|ix| self.render_session_row(ix, cx))
                            .collect::<Vec<_>>(),
                    )
                    .overflow_y_scrollbar(),
            )
            // 底部：文件视图入口（占位页；与 Tauri 版同位置）
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(colors::BORDER()))
                    .px_2()
                    .py_2()
                    .child(
                        div()
                            .id("open-workspace")
                            .w_full()
                            .px_2()
                            .py_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(
                                svg()
                                    .path(IconName::FileText.path())
                                    .size(px(14.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .child("文件视图")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.view = View::Workspace;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    /// 右栏：会话 Tab 条 + 消息列 + 三条横幅 + 输入卡片 + 统计行。
    fn render_chat_main(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    // 顶部：打开的会话 Tab 条（浏览器式；无 tab 时退回单行标题）
                    .child(self.render_session_tabs(cx))
                    .child(self.render_message_column())
                    .child(self.render_banners(cx))
                    .child(self.render_composer(cx))
                    .child(self.render_usage_line())
            )
            .into_any_element()
    }

    /// 消息列：限宽 896 居中（与输入卡片同宽、同左边界）。
    ///
    /// 父级用 `flex_col + items_center` 让限宽块水平居中；限宽块自身
    /// `w_full + max_w` 才会在宽窗口下取 896。
    fn render_message_column(&self) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child({
                        let items = self.messages.clone();
                        let expand = self.expand.clone();
                        let app = self.app_ref.clone();
                        // 回退需要会话 id（消息操作按钮用）
                        let sid = self
                            .current_session
                            .clone()
                            .unwrap_or_default();
                        gpui_list(
                            self.msg_list_state.clone(),
                            move |ix, _window, _cx| {
                                let Some(m) = items.get(ix) else {
                                    return div().into_any_element();
                                };
                                // 段渲染口径见 `render_msg_body`（思考块 / 工具卡可折叠，
                                // 结果按 `tool_calls` 的 id 关联）
                                render_msg_body(m, ix, &expand, &app, &sid)
                            },
                        )
                        .size_full()
                    }),
            )
            .into_any_element()
    }

    /// 审批横幅 / 追问卡片 / 待办窄条：与消息列、输入卡片同宽居中。
    /// 三者空时各自返回 0 高元素，不占位。
    fn render_banners(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child(self.render_approval_banner(cx)),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child(self.render_clarification(cx)),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child(self.render_todo_bar(cx)),
            )
            .into_any_element()
    }

    /// 输入卡片 = **一体式卡片**（对齐 Tauri 版 `ChatInput`）：
    /// 外框限宽 896 居中、bg-secondary、rounded-2xl；卡片内 = 编辑器 + 底部控件行。
    fn render_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let ctx_pct = self.ctx_pct();
        let ctx_color = self.ctx_color();
        let ctx_has_usage = self.ctx_usage().is_some() && self.ctx_window() > 0;
        let ctx_center_text = if ctx_has_usage {
            format!("{ctx_pct}%")
        } else {
            "–".to_string()
        };
        div()
            .border_t_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_PRIMARY()))
            .px_4()
            .pt_3()
            .pb_3()
            .child(
                div()
                    .w_full()
                    .max_w(px(896.))
                    .mx_auto()
                    .rounded_2xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    // 编辑器自身不画边框与底色（`appearance(false)`），
                    // 由外层卡片充当「输入框」——否则会看成上下分离的
                    // 两块（上面一个输入框 + 下面一排按钮），与 Tauri
                    // 版「按钮在输入框内」的形态不符。
                    .child(
                        Textarea::new(&self.input)
                            .h(px(64.))
                            .appearance(false),
                    )
                    // 待发送图片缩略图（单独成函数：压 render_chat 栈帧）
                    .children(if self.composer_images.is_empty() {
                        None
                    } else {
                        Some(self.render_composer_attachments(cx))
                    })
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1())
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    // 控件顺序对齐 vite `ChatInput`：模型 → 思考 → 上下文环 → 发送
                                    // @ 文件引用：弹窗选文件 → 光标处插入 `@file:"<路径>"`
                                    // （序列化口径与 Web 版 `serializeInlineChip` 逐字一致）
                                    .child(
                                        div()
                                            .id("mention-file")
                                            .px_1()
                                            .py_1()
                                            .rounded_lg()
                                            .cursor_pointer()
                                            .child(
                                                svg()
                                                    .path(IconName::AtSign.path())
                                                    .size(px(14.))
                                                    .text_color(rgb(
                                                        colors::TEXT_SECONDARY(),
                                                    )),
                                            )
                                            .on_click(cx.listener(
                                                |this, _, _window, cx| {
                                                    this.open_file_picker(
                                                        FilePickPurpose::Mention,
                                                        cx,
                                                    )
                                                },
                                            )),
                                    )
                                    // 附件（图片）：OS 选择器 → data URL 挂到待发送列表
                                    .child(
                                        div()
                                            .id("attach-image")
                                            .px_1()
                                            .py_1()
                                            .rounded_lg()
                                            .cursor_pointer()
                                            .child(
                                                svg()
                                                    .path(IconName::ImagePlus.path())
                                                    .size(px(14.))
                                                    .text_color(rgb(
                                                        colors::TEXT_SECONDARY(),
                                                    )),
                                            )
                                            .on_click(cx.listener(
                                                |this, _, _window, cx| {
                                                    this.pick_composer_images(cx)
                                                },
                                            )),
                                    )
                                    .child(
                                        // 模型选择：浮层挂在自己身上（`relative` 定位 +
                                        // `deferred` 延迟绘制）——这样它出现在**按钮正上方**，
                                        // 且画在卡片边框之后（不会透出下面的线）
                                        div()
                                            .id("model-select")
                                            .relative()
                                            .px_1()
                                            .py_1()
                                            .rounded_lg()
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child(self.current_model_label())
                                            .on_click(cx.listener(
                                                |this, _, _window, cx| {
                                                    this.model_menu_open =
                                                        !this.model_menu_open;
                                                    this.thinking_menu_open = false;
                                                    this.ctx_panel_open = false;
                                                    cx.notify();
                                                },
                                            ))
                                            .children(if self.model_menu_open {
                                                Some(deferred(self.render_model_menu(cx)))
                                            } else {
                                                None
                                            }),
                                    )
                                    // 思考强度（当前模型没声明档位时这里是空）
                                    .children(self.render_thinking_select(cx))
                                    .child(
                                        // 上下文环：百分比在**圈内**（vite 同形态）。
                                        // ⚠️ 环值口径是 **0-100**（库约定），不是 0-1
                                        // ——写 0-1 时环永远是空的。
                                        div()
                                            .id("ctx-ring-btn")
                                            .relative()
                                            .flex()
                                            .items_center()
                                            .px_1()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .child(
                                                ProgressCircle::new("ctx-ring")
                                                    .value(ctx_pct as f32)
                                                    .size(px(30.))
                                                    .color(Hsla::from(rgb(
                                                        ctx_color,
                                                    )))
                                                    .child(
                                                        div()
                                                            .text_size(px(9.))
                                                            .text_color(rgb(
                                                                if ctx_has_usage {
                                                                    colors::TEXT_SECONDARY()
                                                                } else {
                                                                    colors::TEXT_TERTIARY()
                                                                },
                                                            ))
                                                            .child(
                                                                ctx_center_text,
                                                            ),
                                                    ),
                                            )
                                            .on_click(cx.listener(
                                                |this, _, _window, cx| {
                                                    this.ctx_panel_open =
                                                        !this.ctx_panel_open;
                                                    // 三个浮层互斥：开一个收另外两个
                                                    this.model_menu_open = false;
                                                    this.thinking_menu_open = false;
                                                    if !this.ctx_panel_open {
                                                        this.ctx_compress = None;
                                                    }
                                                    cx.notify();
                                                },
                                            ))
                                            .children(if self.ctx_panel_open {
                                                Some(deferred(self.render_ctx_panel(cx)))
                                            } else {
                                                None
                                            }),
                                    )
                            .child({
                                let stopping = self.streaming;
                                div()
                                    .id("send")
                                    .px_3()
                                    .py_1()
                                    .rounded_xl()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .bg(if stopping {
                                        rgb(colors::ERROR())
                                    } else {
                                        rgb(colors::ACCENT())
                                    })
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    // 与 Tauri 版 `ChatInput` 同形：图标 + 文案
                                    // （发送 = lucide `Send`，停止 = `Square`）
                                    .child(
                                        svg()
                                            .path(if stopping {
                                                IconName::Square.path()
                                            } else {
                                                IconName::Send.path()
                                            })
                                            .size(px(14.))
                                            .text_color(rgb(0xffffff)),
                                    )
                                    .child(if stopping {
                                        "停止"
                                    } else {
                                        "发送"
                                    })
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            if this.streaming {
                                                this.stop_current_turn(cx);
                                            } else {
                                                this.send_current_input(
                                                    window, cx,
                                                );
                                            }
                                        },
                                    ))
                            })
                    ),
            )
            )
            .into_any_element()
    }

    /// 输入卡片下方统计行：会话累计（未命中/命中/输出/命中率）+ 最近一轮性能
    /// （TTFT / tok/s）。两路数据都为空时整行不渲染（不留空白条）。
    fn render_usage_line(&self) -> AnyElement {
        if self.session_usage.is_none() && self.request_perf.is_none() {
            return div().into_any_element();
        }
        let mut line = div()
            .w_full()
            .max_w(px(colors::CHAT_MAX_W))
            .mx_auto()
            .pt_2()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .text_size(px(11.))
            .text_color(rgb(colors::TEXT_TERTIARY()));
        let dot = || {
            div().text_color(rgb(colors::BORDER())).child("·")
        };
        if let Some((uncached, cached, completion)) =
            self.session_usage
        {
            let total = uncached + cached;
            let hit_rate = if total > 0 {
                (cached as f64 / total as f64 * 100.0).round()
                    as u64
            } else {
                0
            };
            line = line
                .child(div().child(format!(
                    "未命中输入 {}",
                    group_thousands(uncached)
                )))
                .child(dot())
                .child(div().child(format!(
                    "缓存命中输入 {}",
                    group_thousands(cached)
                )))
                .child(dot())
                .child(div().child(format!(
                    "输出 {}",
                    group_thousands(completion)
                )))
                .child(dot())
                .child(
                    div().child(format!(
                        "缓存命中率 {hit_rate}%"
                    )),
                );
        }
        if let Some((ttft, duration, completion)) =
            self.request_perf
        {
            if self.session_usage.is_some() {
                line = line.child(dot());
            }
            line = line.child(div().child(format!(
            "TTFT {}",
            ttft.map(format_ms)
                .unwrap_or_else(|| "—".to_string())
        )));
            if let Some(tps) =
                tokens_per_sec(ttft, duration, completion)
            {
                line = line
                    .child(dot())
                    .child(div().child(format!("{tps} tok/s")));
            }
        }
        line.into_any_element()
    }

    /// 上下文详情浮层（点圆环弹出）：占用 / 缓存命中 / 输出 +「压缩会话」。
    /// 环本身只是缩略，口径（最近一轮 vs 会话累计）写在浮层里。
    fn render_ctx_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let ctx_pct = self.ctx_pct();
        let ctx_usage = self.ctx_usage();
        let ctx_window = self.ctx_window();
        // 「左标签 / 右值」一行（详情面板三行共用）
        let kv_row = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(div().text_color(rgb(colors::TEXT_TERTIARY())).child(label))
                .child(div().flex_1())
                .child(div().text_color(rgb(colors::TEXT_PRIMARY())).child(value))
        };
        // 上下文详情面板（对齐 vite `ContextRing` 的浮层）：占用 / 缓存命中 / 输出
        // +「压缩会话」。环本身只是缩略，口径（最近一轮 vs 会话累计）写在面板里。
        let mut ctx_panel = div()
            .absolute()
            // 相对**环本身**定位（环容器是 `.relative()`）：浮层贴环的正上方
            .bottom(px(34.))
            .right(px(0.))
            .w(px(300.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_PRIMARY()))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_xs()
            .child(kv_row(
                "上下文占用",
                match ctx_usage {
                    Some(u) if ctx_window > 0 => format!(
                        "{} / {}（{ctx_pct}%）",
                        group_thousands(u.prompt_tokens),
                        group_thousands(ctx_window)
                    ),
                    Some(u) => format!("{} token（窗口未知）", group_thousands(u.prompt_tokens)),
                    None => "还没有用量数据".to_string(),
                },
            ))
            .child(kv_row(
                "缓存命中",
                match ctx_usage {
                    Some(u) if u.cache_read > 0 => format!(
                        "{} token（{}%）",
                        group_thousands(u.cache_read),
                        (u.cache_read as f64 / u.prompt_tokens.max(1) as f64 * 100.0).round()
                            as u64
                    ),
                    Some(_) => "0（提供商未返回明细）".to_string(),
                    None => "—".to_string(),
                },
            ))
            .child(kv_row(
                "输出",
                match ctx_usage {
                    Some(u) => format!("{} token", group_thousands(u.completion_tokens)),
                    None => "—".to_string(),
                },
            ))
            .child(
                div()
                    .pt_1()
                    .border_t_1()
                    .border_color(rgb(colors::BORDER()))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("以上为最近一轮请求 · 输入框下方小字为会话累计"),
            );
        // 压缩按钮：流式中 / 压缩中不可点（同 vite 的 disabled 口径）
        let compressing = self.ctx_compressing;
        let can_compress = self.current_session.is_some()
            && ctx_usage.is_some()
            && !compressing
            && !self.streaming;
        let btn_color = if can_compress {
            colors::TEXT_SECONDARY()
        } else {
            colors::TEXT_TERTIARY()
        };
        let mut compress_btn = div()
            .id("ctx-compress")
            .w_full()
            .py(px(5.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1()
            .text_color(rgb(btn_color))
            .child(
                svg()
                    .path(if compressing {
                        IconName::LoaderCircle.path()
                    } else {
                        IconName::Minimize2.path()
                    })
                    .size(px(12.))
                    .text_color(rgb(btn_color)),
            )
            .child(if compressing {
                "压缩中…".to_string()
            } else if ctx_usage.is_some() {
                format!("压缩会话（当前占用 {ctx_pct}%）")
            } else {
                "压缩会话".to_string()
            });
        if can_compress {
            compress_btn = compress_btn
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _window, cx| this.compress_session(cx)));
        }
        ctx_panel = ctx_panel.child(compress_btn);
        if let Some((ok, msg)) = &self.ctx_compress {
            ctx_panel = ctx_panel.child(
                div()
                    .text_color(rgb(if *ok {
                        colors::SUCCESS()
                    } else {
                        colors::ERROR()
                    }))
                    .child(msg.clone()),
            );
        }
        ctx_panel.into_any_element()
    }

    /// 思考强度选择器（对齐 vite `ThinkingSelect`）：档位来自当前模型声明，
    /// 模型不支持思考时**整块不渲染**（返回 None，不占位）；`off` 档显示为「default」。
    fn render_thinking_select(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 思考强度选择器（vite `ThinkingSelect` 同口径）：档位来自当前模型声明，
        // 模型不支持思考时**整块不渲染**（不占位）；`off` 档如实显示为「default」。
        let thinking_open = self.thinking_menu_open;
        let thinking_label = self.thinking_label();
        let mut thinking_select: Option<AnyElement> = None;
        if let Some(opts) = self.thinking_options() {
            let mut dd = div()
                .absolute()
                .bottom(px(26.))
                .right(px(0.))
                .w(px(160.))
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_PRIMARY()))
                .py_1()
                .flex()
                .flex_col();
            for (ix, opt) in opts.iter().enumerate() {
                // `off` = 不附加思考参数（None）；其余档位值原样发后端
                let value: Option<String> = if opt == "off" {
                    None
                } else {
                    Some(opt.clone())
                };
                let label = if opt == "off" {
                    "default".to_string()
                } else {
                    opt.clone()
                };
                let is_current = value == self.thinking;
                let mut row = div()
                    .id(("thinking-opt", ix))
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_xs();
                row = if is_current {
                    row.text_color(rgb(colors::ACCENT()))
                } else {
                    row.text_color(rgb(colors::TEXT_PRIMARY()))
                };
                dd = dd.child(
                    row.child(div().flex_1().child(label))
                        .children(if is_current {
                            Some(
                                svg()
                                    .path(IconName::Check.path())
                                    .size(px(12.))
                                    .text_color(rgb(colors::ACCENT())),
                            )
                        } else {
                            None
                        })
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_thinking(value.clone(), cx);
                        })),
                );
            }
            thinking_select = Some(
                div()
                    .id("thinking-select")
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(format!("思考 {thinking_label}"))
                    .child(
                        svg()
                            .path(IconName::ChevronDown.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.thinking_menu_open = !this.thinking_menu_open;
                        // 三个浮层互斥：开一个收另外两个
                        this.model_menu_open = false;
                        this.ctx_panel_open = false;
                        cx.notify();
                    }))
                    .children(if thinking_open {
                        Some(deferred(dd))
                    } else {
                        None
                    })
                    .into_any_element(),
            );
        }
        thinking_select
    }

    /// 模型下拉浮层（按钮在输入卡片里，这里只造浮层内容）：
    /// 只列带 `chat` 能力的模型（capabilities 是 kebab-case 数组，后端拼成 `/` 分隔串）。
    fn render_model_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        // 模型下拉候选：只列带 `chat` 能力的模型（与 Tauri 版 `ModelSelector` 同口径，
        // capabilities 是 kebab-case 数组，后端拼成 `/` 分隔的串）
        let chat_models: Vec<(String, String)> = self
            .models
            .iter()
            .filter(|m| m.capabilities.split('/').any(|c| c == "chat"))
            .map(|m| (m.provider.clone(), m.name.clone()))
            .collect();
        let mut menu = div()
            .absolute()
            // 相对**模型按钮**定位：浮层贴在它正上方（不再统一吊在控件行右端）
            .bottom(px(30.))
            .right(px(0.))
            .w(px(256.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_PRIMARY()))
            .py_1()
            .flex()
            .flex_col();
        for (ix, (provider, name)) in chat_models.iter().enumerate() {
            let provider = provider.clone();
            let name = name.clone();
            let is_current = self
                .session_model
                .as_ref()
                .map(|(p, m)| p == &provider && m == &name)
                .unwrap_or(false);
            let mut row = div()
                .id(("model-option", ix))
                .px_3()
                .py_2()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .cursor_pointer()
                .text_xs();
            if is_current {
                row = row.text_color(rgb(colors::ACCENT()));
            } else {
                row = row.text_color(rgb(colors::TEXT_PRIMARY()));
            }
            let label = name.clone();
            let prov = provider.clone();
            menu = menu.child(
                row.child(div().flex_1().child(name))
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(provider),
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.select_chat_model(prov.clone(), label.clone(), cx);
                    })),
            );
        }
        menu.into_any_element()
    }
}

impl Render for ClientApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        root
    }
}

/// 子代理活动流的一行（把消息摊平后的投影）。
enum ActivityRow {
    Thinking { key: String, text: String },
    Text { key: String, text: String },
    Tool { key: String, call: ToolCall },
}

impl ClientApp {
    /// 子代理活动流（对齐 Web 版 `DelegateActivity`）。
    ///
    /// **紧凑单行列表**，不是消息气泡：图标 + 标签 + 摘要 + 字数/耗时，点开看原文。
    /// 相邻同类段合并 —— 流式逐 delta 会切出大量碎片段，不合并会一行一个 token。
    fn render_delegate_activity(
        &self,
        task_id: &str,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(messages) = self.task_messages.get(task_id) else {
            return div()
                .px_1()
                .py_1()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(if live {
                    "子智能体工作中…"
                } else {
                    "加载中…"
                })
                .into_any_element();
        };
        let mut rows: Vec<ActivityRow> = Vec::new();
        for (mi, m) in messages.iter().enumerate() {
            for (si, seg) in m.segs.iter().enumerate() {
                let key = format!("{task_id}:{mi}:{si}");
                match seg {
                    Seg::Thinking(t) => {
                        if let Some(ActivityRow::Thinking { text, .. }) = rows.last_mut() {
                            text.push_str(t);
                        } else {
                            rows.push(ActivityRow::Thinking {
                                key,
                                text: t.clone(),
                            });
                        }
                    }
                    Seg::Text(t) => {
                        if let Some(ActivityRow::Text { text, .. }) = rows.last_mut() {
                            text.push_str(t);
                        } else {
                            rows.push(ActivityRow::Text {
                                key,
                                text: t.clone(),
                            });
                        }
                    }
                    Seg::Tool { id, name, .. } => {
                        let call = m
                            .tool_calls
                            .iter()
                            .find(|c| &c.id == id)
                            .or_else(|| m.tool_calls.iter().find(|c| &c.name == name))
                            .cloned()
                            .unwrap_or_else(|| ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: String::new(),
                                presentation: "generic".to_string(),
                                result: None,
                                duration_ms: None,
                                error: None,
                            });
                        rows.push(ActivityRow::Tool { key, call });
                    }
                }
            }
        }
        if rows.is_empty() {
            return div()
                .px_1()
                .py_1()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(if live {
                    "子智能体工作中…"
                } else {
                    "无过程记录"
                })
                .into_any_element();
        }
        let mut list = div().max_h(px(360.)).flex().flex_col().gap_1().w_full();
        for (ix, row) in rows.into_iter().enumerate() {
            list = list.child(self.render_activity_row(ix, row, cx));
        }
        list.overflow_y_scrollbar().into_any_element()
    }

    /// 活动流的一行：收起态一行（图标 + 标签 + 摘要 + meta + 箭头），展开态一块原文。
    fn render_activity_row(
        &self,
        ix: usize,
        row: ActivityRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open_key = self.expand.borrow().activity.clone();
        let (key, is_tool) = match &row {
            ActivityRow::Thinking { key, .. } | ActivityRow::Text { key, .. } => {
                (key.clone(), false)
            }
            ActivityRow::Tool { key, .. } => (key.clone(), true),
        };
        let open = open_key.as_deref() == Some(key.as_str());
        let chevron = if open {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };

        // 头：图标 + 标签 + 摘要 + meta + 箭头
        let mut head = div()
            .id(("act", ix))
            .w_full()
            .px_1()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .text_size(px(11.));
        let mut details: Vec<AnyElement> = Vec::new();
        match row {
            ActivityRow::Tool { call, .. } => {
                let (icon, label) = presentation_of(&call.presentation);
                let failed = call.error.is_some();
                let settled = call.duration_ms.is_some() || call.result.is_some();
                let target = summarize_tool_target(&call.arguments);
                head = head
                    .child(if failed {
                        svg()
                            .path(IconName::CircleX.path())
                            .size(px(11.))
                            .text_color(rgb(colors::ERROR()))
                            .into_any_element()
                    } else {
                        svg()
                            .path(icon.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(target),
                    );
                if let Some(ms) = call.duration_ms {
                    head = head.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format_duration(ms)),
                    );
                } else if !settled {
                    head = head.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("…"),
                    );
                }
                details.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(format_arguments(&call.arguments))
                        .into_any_element(),
                );
                if let Some(err) = call.error {
                    details.push(
                        div()
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(colors::ERROR()))
                            .child(err)
                            .into_any_element(),
                    );
                }
                if let Some(result) = call.result.filter(|r| !r.is_empty()) {
                    details.push(
                        div()
                            .px_2()
                            .py_1()
                            .max_h(px(220.))
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(result)
                            .into_any_element(),
                    );
                }
            }
            ActivityRow::Thinking { text, .. } => {
                // 思考行摘要取**最后一行**（模型的当前念头），输出行取第一行
                let banner = text
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                head = head
                    .child(
                        svg()
                            .path(IconName::Brain.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("思考"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(banner),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{} 字", text.chars().count())),
                    );
                details.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(text)
                        .into_any_element(),
                );
            }
            ActivityRow::Text { text, .. } => {
                let banner = text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                head = head
                    .child(
                        svg()
                            .path(IconName::TextAlignStart.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("输出"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(banner),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{} 字", text.chars().count())),
                    );
                details.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(text)
                        .into_any_element(),
                );
            }
        }
        head = head.child(
            svg()
                .path(chevron.path())
                .size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY())),
        );
        let _ = is_tool;
        let head = head.on_click(cx.listener(move |this, _, _, cx| {
            let key = key.clone();
            let cur = this.expand.borrow().activity.clone();
            this.expand.borrow_mut().activity = if cur.as_deref() == Some(key.as_str()) {
                None
            } else {
                Some(key.clone())
            };
            cx.notify();
        }));

        let mut outer = div().flex().flex_col().w_full().child(head);
        if open {
            for d in details {
                outer = outer.child(d);
            }
        }
        outer.into_any_element()
    }
}

fn main() {
    let config = tianyan_service_config::load_and_install();
    if let Err(e) = tianyan::common::logging::init_logging(&config.logging) {
        eprintln!("日志初始化失败：{e}");
    }
    tianyan::common::panic_hook::install(tianyan::common::panic_hook::default_log_dir());

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

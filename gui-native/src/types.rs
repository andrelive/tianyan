//! 界面数据模型：各页面的行结构、DTO、枚举。
//!
//! 全是**后端响应的最小子集**（够渲染即可，不追求与 server DTO 逐字段对齐）：
//! 多一个字段就要多维护一处映射，而这些结构只在 GUI 里活着。

use gpui_kit::assets::IconName;

/// 一级导航项（与 `gui-vite/src/components/sidebar/Sidebar.tsx` 的 `NAV_ITEMS` 逐项对齐）。
///
/// 第一列是 rail 上显示的 **Lucide 图标**：`gpui_kit::assets::IconName` 就是完整 Lucide
/// 目录（1800+ 个，`gpui-kit-assets` 内嵌 SVG），与 Tauri 版 `lucide-react` 同一套图形，
/// 逐项对齐（MessageSquare / FileText / Wrench / Users / Hammer / BookOpen /
/// MemoryStick / ShieldCheck / CalendarClock / Gauge / Settings）。第三列是路由目标，
/// 未实现的入口渲染占位页。
pub(crate) const NAV_ITEMS: [(IconName, &str, View); 11] = [
    (IconName::MessageSquare, "会话", View::Chat),
    (IconName::FileText, "文件", View::Workspace),
    (IconName::Wrench, "技能", View::Skills),
    (IconName::Users, "子智能体", View::Roles),
    (IconName::Hammer, "工具", View::Tools),
    (IconName::BookOpen, "知识", View::Knowledge),
    (IconName::MemoryStick, "记忆", View::Memory),
    (IconName::ShieldCheck, "审批", View::Approval),
    (IconName::CalendarClock, "定时任务", View::Scheduled),
    (IconName::Gauge, "洞察", View::Insights),
    (IconName::Settings, "设置", View::Settings),
];

/// 当前视图（一级导航的 11 个入口；已实现的只有「会话」与「设置」）
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    /// 会话：列表 + 消息 + 输入
    Chat,
    /// 文件（workspace）
    Workspace,
    /// 技能
    Skills,
    /// 子智能体（角色）
    Roles,
    /// 工具
    Tools,
    /// 知识
    Knowledge,
    /// 记忆
    Memory,
    /// 审批
    Approval,
    /// 定时任务
    Scheduled,
    /// 洞察
    Insights,
    /// 设置
    Settings,
}

impl View {
    /// 全部一级入口（顺序即侧栏顺序）。
    pub(crate) const ALL: [View; 11] = [
        View::Chat,
        View::Workspace,
        View::Skills,
        View::Roles,
        View::Tools,
        View::Knowledge,
        View::Memory,
        View::Approval,
        View::Scheduled,
        View::Insights,
        View::Settings,
    ];

    /// 稳定标识（自动化 API 的**对外契约值**：`state.view` 与 `switch_tab` 的参数）。
    /// 改名等于改契约，验证脚本会跟着断。
    pub(crate) fn key(self) -> &'static str {
        match self {
            View::Chat => "chat",
            View::Workspace => "workspace",
            View::Skills => "skills",
            View::Roles => "roles",
            View::Tools => "tools",
            View::Knowledge => "knowledge",
            View::Memory => "memory",
            View::Approval => "approval",
            View::Scheduled => "scheduled",
            View::Insights => "insights",
            View::Settings => "settings",
        }
    }

    /// 由标识反查（未知标识返回 `None`，报错交给调用方）。
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.key() == key)
    }
}

/// 设置页的模型行（只取展示与切换需要的字段）。
#[derive(Clone, Debug)]
pub(crate) struct ModelRow {
    pub(crate) provider: String,
    pub(crate) name: String,
    /// 能力标签（kebab-case，逗号拼接；`chat` / `vision` / `text-embedding` …）
    pub(crate) capabilities: String,
    pub(crate) context: Option<u64>,
    /// 该模型自己声明的思考强度档位（`/config/models` 的 `reasoning_efforts`）。
    /// 空 = 不支持思考 → 输入区的「思考」选择器整体隐藏（对齐 vite `ThinkingSelect`）
    pub(crate) reasoning_efforts: Vec<String>,
    /// 是否为当前 chat 首选模型（`preferences.chat` 命中）
    pub(crate) is_chat: bool,
}

/// 定时任务的间隔预设（秒；`0` = 自定义）。对齐 Web 版 `INTERVAL_PRESETS`。
pub(crate) const SCHED_PRESETS: [(u64, &str); 7] = [
    (1800, "每 30 分钟"),
    (3600, "每 1 小时"),
    (21600, "每 6 小时"),
    (43200, "每 12 小时"),
    (86400, "每天"),
    (604800, "每周"),
    (0, "自定义…"),
];

// ===== 会话列表 =====

/// 会话（`GET /api/v1/sessions` 的子集，够左栏分组渲染）。
#[derive(Clone, Debug)]
pub(crate) struct SessionRow {
    pub(crate) id: String,
    pub(crate) title: String,
    /// 工作目录绝对路径；`""` = 未绑定（默认组）。后端字段 `working_directory` 可为 null。
    pub(crate) working_directory: String,
    /// RFC3339 时间戳（列表展示相对时间）
    pub(crate) updated_at: String,
    /// 会话**首条请求**的输入 token（= 系统前缀基线）。
    /// 压缩摘要消息的 `prompt_tokens` 是压缩请求的输入（≈ 压缩前上下文），
    /// 压缩后占用只能估：基线 + 摘要输出。见 `ClientApp::ctx_usage()`。
    pub(crate) first_request_tokens: Option<u64>,
}

/// 左栏的一行 —— **分组行 / 会话行 / 「查看更多」行混排**。
///
/// 与 `SessionRow`（原始会话数据）分开：一行的「是什么」由折叠状态、选中态、
/// 后台任务数共同决定，不能从会话数据直接推出来。
#[derive(Clone, Debug)]
pub(crate) enum SessionListRow {
    /// 工作区分组行（目录名 + 会话数 + 折叠箭头）
    Group {
        key: String,
        label: String,
        count: usize,
        collapsed: bool,
    },
    /// 会话行
    Session {
        id: String,
        title: String,
        /// 副标题：相对时间（右对齐信息，空串则不画）
        meta: String,
        /// 该会话正在跑的后台任务数（>0 显示红色角标）
        tasks: usize,
        selected: bool,
    },
    /// 组内折叠的「查看更多 (n)」行
    More { key: String, hidden: usize },
}
/// 目录选择器选中的目录用在哪里。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum PickerTarget {
    /// 绑定到当前会话（`PUT /sessions/{id}/workspace`）
    Session,
    /// 作为下一个新会话的待绑定目录（本地状态，首条消息时下发）
    NewSession,
}

impl PickerTarget {
    /// 稳定标识（自动化 API 契约值）。
    pub(crate) fn key(self) -> &'static str {
        match self {
            PickerTarget::Session => "session",
            PickerTarget::NewSession => "new_session",
        }
    }
}

/// 文件选择弹窗的用途：**同一套浏览 UI，选中之后做什么不同**。
///
/// 复用一个弹窗而不是写两个：目录导航 / 名称过滤 / 隐藏项开关 / 大小格式化
/// 逐字相同，分叉只在「确认动作」上。多一个枚举比重写一遍弹窗安全。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilePickPurpose {
    /// 知识导入：多选，勾满后点「导入选中」，逐个 ingest
    KbIngest,
    /// @ 文件引用：单选，**点中即插入**输入框并关闭（不需要勾选 + 确认两步）
    Mention,
}

impl FilePickPurpose {
    /// 稳定标识（自动化 API 契约值；`state.file_picker.purpose`）。
    pub(crate) fn key(self) -> &'static str {
        match self {
            FilePickPurpose::KbIngest => "kb_ingest",
            FilePickPurpose::Mention => "mention",
        }
    }
}

#[derive(Clone)]
/// 需要用户确认的模态对话框（自绘覆盖层，不依赖组件库 Dialog）。
pub(crate) enum Dialog {
    /// 删除会话确认
    DeleteSession { id: String, title: String },
    /// 重命名会话（输入框实体在 `ClientApp::dialog_input`）
    RenameSession { id: String },
    /// 新建会话并绑定工作目录（「＋新目录」；输入框实体同上）
    /// 目录选择器（逐级浏览后选定）
    WorkspacePicker { target: PickerTarget },
    /// 回退内置种子（`POST /roles/{name}/reset`）
    ResetRole { name: String },
    /// 退役删除角色（`DELETE /roles/{name}`）
    DeleteRole { name: String },
    /// 文件选择弹窗（**GPUI 自绘**，不是 OS 对话框 —— 所以自动化点得到）
    FilePicker { purpose: FilePickPurpose },
    /// 回退到某条用户输入之前（`POST /sessions/{id}/messages/delete`，ADR-040 事务）
    RollbackMessage {
        session_id: String,
        message_id: String,
        /// 该消息开头若干字，确认框里给用户看「回退到哪」
        preview: String,
    },
}

impl Dialog {
    /// 稳定标识（自动化 API 契约值；`state.dialog.kind`）。
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Dialog::DeleteSession { .. } => "delete_session",
            Dialog::RenameSession { .. } => "rename_session",
            Dialog::WorkspacePicker { .. } => "workspace_picker",
            Dialog::ResetRole { .. } => "reset_role",
            Dialog::DeleteRole { .. } => "delete_role",
            // 用途直接体现在 kind 里：自动化断言要能区分「导入弹窗」与「引用弹窗」
            Dialog::FilePicker { purpose } => match purpose {
                FilePickPurpose::KbIngest => "kb_file_picker",
                FilePickPurpose::Mention => "mention_file_picker",
            },
            Dialog::RollbackMessage { .. } => "rollback_message",
        }
    }
}

// 左栏行由 `ClientApp::rebuild_session_rows` 直接产生（行类型混排 + 行高不一，
// 虚拟列表的等高测量在这里只会帮倒忙：实测「查看更多」行会与上一会话行重叠）。
// ===== 消息（不再用虚拟列表：高度不定，几十到 200 条直接布局更简单可靠）=====

/// 会话目标行。
///
/// 注意 `GET /goals` 的元素是**嵌套结构**：`{ goal: {title, status, ...},
/// progress, todo_total, todo_done }`（`server/src/api/goals/handlers.rs` 现场组装，
/// 不是 core 的 `Goal` 平铺）。早先按平铺解析导致标题空白、进度丢失。
#[derive(Clone, Debug)]
pub(crate) struct GoalRow {
    pub(crate) title: String,
    pub(crate) status: String,
    /// 0-100（`todo_total == 0` 时为 0，表示「还没有子待办」）
    pub(crate) progress: u64,
    pub(crate) todo_total: u64,
    pub(crate) todo_done: u64,
}

/// 子智能体（角色）列表项：`GET /roles` 的 `roles[]`（`RoleSummary` 子集）。
#[derive(Clone, Debug)]
pub(crate) struct RoleRow {
    pub(crate) name: String,
    /// 来源：builtin / user / learned
    pub(crate) source: String,
    /// 状态：active / experimental
    pub(crate) status: String,
    pub(crate) version: u32,
    /// 职责一句话（系统提示首行）
    pub(crate) purpose: String,
    /// 工具白名单数量（None = 不限制）
    pub(crate) tool_count: Option<u64>,
    /// 模型（None = 回落主 Agent）
    pub(crate) model: Option<String>,
    /// `(调用次数, 成功率)`；无记录时 None
    pub(crate) usage: Option<(u32, f32)>,
}

/// 子智能体统计（`GET /roles/stats`）：页面顶部那条汇总。
#[derive(Clone, Debug, Default)]
pub(crate) struct RolesStats {
    pub(crate) total_calls: u32,
    /// 0-1
    pub(crate) success_rate: f32,
    /// `(任务类型, 次数)`
    pub(crate) by_task_type: Vec<(String, usize)>,
    /// `(角色名, 调用次数)`
    pub(crate) by_role: Vec<(String, u32)>,
}

/// 工作区目录树的一条（`GET /workspace/tree` 的 `entries[]`）。
#[derive(Clone, Debug)]
pub(crate) struct WsEntry {
    pub(crate) name: String,
    /// true = 目录（点进去），false = 文件（点开读）
    pub(crate) is_dir: bool,
    /// 相对工作区根的路径（如 `src/main.rs`）
    pub(crate) path: String,
    /// 文件大小（目录为 0）
    pub(crate) size: u64,
}

/// 工作区改动文件（`GET /workspace/diff` 的列表项）。
#[derive(Clone, Debug)]
pub(crate) struct WsDiffFile {
    /// 相对工作区根的路径
    pub(crate) path: String,
    /// modified / added / removed / binary / unchanged
    pub(crate) status: String,
    /// 旧文件行数（新增文件为 0）
    pub(crate) old_lines: u64,
    /// 新文件行数（删除文件为 0）
    pub(crate) new_lines: u64,
}

/// 角色详情（`GET /roles/{name}`）：完整系统提示 + 工具白名单。
#[derive(Clone, Debug, Default)]
pub(crate) struct RoleDetail {
    pub(crate) system_prompt: String,
    pub(crate) tools: Vec<String>,
}

/// 工具目录的一行（`GET /api/v1/tools`）。
///
/// 工具 = 原子化操作原语（系统内置），与「技能」（方法论）是两套概念：
/// 工具页展示可调用的原子操作，技能页展示 GEPA 学到的方法论。
#[derive(Clone, Debug)]
pub(crate) struct ToolRow {
    pub(crate) name: String,
    pub(crate) description: String,
    /// 参数 JSON Schema（详情页原样 pretty 打印，不另做表单）
    pub(crate) parameters: serde_json::Value,
}

/// 学习方法论技能（`GET /api/v1/skills` 的子集）。
///
/// 只列 `category == "custom"`：其余类是内置能力（文件/命令/网络），由工具面板覆盖。
/// 技能是**方法论文档**（无执行语义），所以只读查看，不做「运行」按钮。
#[derive(Clone, Debug)]
pub(crate) struct SkillRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) version: String,
    pub(crate) updated_at: Option<String>,
}

/// 技能使用统计的一行（`GET /api/v1/skills/stats` 的 `skills[]`）。
///
/// 方法论技能没有「执行成败」语义，这里只反映**采纳频率**与 handler 层异常。
#[derive(Clone, Debug)]
pub(crate) struct SkillStatRow {
    pub(crate) skill_id: String,
    pub(crate) total_calls: u64,
    pub(crate) success_calls: u64,
    pub(crate) avg_time_ms: f64,
}

/// 定时智能体任务（`GET /scheduled-tasks`）。
///
/// 间隔制（ADR-024）：`interval_secs` 是「距上次执行 ≥ 此值即触发」，
/// 服务未运行期间超期的任务在重启后补跑一次。时间戳是 **Unix 秒**（不是 RFC3339）。
#[derive(Clone, Debug)]
pub(crate) struct SchedTaskRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) interval_secs: u64,
    pub(crate) workspace: String,
    pub(crate) prompt: String,
    pub(crate) enabled: bool,
    /// 上次执行时间（RFC3339；统一口径——历史上这里是 epoch 秒）
    pub(crate) last_run_at: Option<String>,
    /// 下次预计执行时间（RFC3339）
    pub(crate) next_run_at: Option<String>,
    pub(crate) last_result: Option<String>,
}

/// 记忆条目（`GET /memory` 的 `memories[]`）。
///
/// 后端已把三类来源聚合齐（`memory/` 事实与决策、`user/` 画像、`agent/patterns` 模式），
/// 并只返回叶子（目录只是导航中间节点）。三层内容是 VFS 的 L0/L1/L2：
/// 列表看 L0（abstract），详情默认展 L1（overview），L2（detail）按需展开。
#[derive(Clone, Debug)]
pub(crate) struct MemoryRow {
    pub(crate) uri: String,
    pub(crate) name: String,
    pub(crate) relative_path: String,
    pub(crate) importance: f32,
    pub(crate) updated_at: String,
    pub(crate) tags: Vec<String>,
    pub(crate) abstract_text: Option<String>,
    pub(crate) overview_text: Option<String>,
    pub(crate) detail_text: Option<String>,
}

/// 洞察页总览（`GET /stats`）。
///
/// 四个追踪面：技能（方法论采纳）、工具（原子操作调用）、文档（VFS 访问）、搜索。
#[derive(Clone, Debug, Default)]
pub(crate) struct InsightStats {
    pub(crate) total_skills_tracked: u64,
    pub(crate) total_skill_calls: u64,
    pub(crate) total_tools_tracked: u64,
    pub(crate) total_tool_calls: u64,
    pub(crate) total_docs_tracked: u64,
    pub(crate) total_searches: u64,
}

/// 模型用量总计（`GET /usage/stats` 的 `total`）。
#[derive(Clone, Debug, Default)]
pub(crate) struct UsageTotal {
    pub(crate) calls: u64,
    pub(crate) uncached_input: u64,
    pub(crate) cached_input: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) total_tokens: u64,
    pub(crate) cache_hit_rate: f64,
}

/// 模型用量分组（`grouped[]`：按 model 或 provider）。
#[derive(Clone, Debug)]
pub(crate) struct UsageGroup {
    pub(crate) group: String,
    pub(crate) calls: u64,
    pub(crate) total_tokens: u64,
    pub(crate) cache_hit_rate: f64,
}

/// 调度器任务状态（`GET /scheduler/status` 的 `tasks[]`）。
#[derive(Clone, Debug)]
pub(crate) struct SchedStatusRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) interval_secs: u64,
    pub(crate) run_count: u64,
    pub(crate) last_run_ago_secs: u64,
    pub(crate) next_due_in_secs: u64,
}

/// 审批自动规则（`config.auto_approval_rules[]` 的展示子集）。
#[derive(Clone, Debug)]
pub(crate) struct ApprovalRuleRow {
    pub(crate) name: String,
    pub(crate) decision: String,
    pub(crate) enabled: bool,
}

/// 审批审计记录（`recent_records[]` 的展示子集）。
///
/// 「结果」是三元：`None` = 还没执行完 / `Some(true)` = 执行成功 / `Some(false)` = 失败。
#[derive(Clone, Debug)]
pub(crate) struct ApprovalHistRow {
    pub(crate) description: String,
    pub(crate) risk: String,
    pub(crate) decision: String,
    pub(crate) session_id: String,
    pub(crate) result: Option<bool>,
}

/// 知识条目（`GET /knowledge/entries`）。三层是否齐备决定详情能读哪些层。
#[derive(Clone, Debug)]
pub(crate) struct KbEntryRow {
    pub(crate) uri: String,
    pub(crate) name: String,
    pub(crate) is_dir: bool,
    pub(crate) has_abstract: bool,
    pub(crate) has_overview: bool,
    pub(crate) has_detail: bool,
}

/// 知识检索结果（`GET /knowledge/search` 的 `results[]`）。
#[derive(Clone, Debug)]
pub(crate) struct KbSearchRow {
    pub(crate) id: String,
    pub(crate) content: String,
    pub(crate) source: String,
    pub(crate) score: f32,
}

/// 内嵌文件浏览器的一行（`GET /workspace/dirs?include_files=true`）。
#[derive(Clone, Debug)]
pub(crate) struct KbBrowseEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) is_dir: bool,
    pub(crate) size: Option<u64>,
}

/// 设置页的 provider 行。
///
/// **含 api_key**：编辑时要把它回填进输入框，而 `set_value` 需要 `&mut Window`
/// （只有在点击处理器这种同步上下文里才有），所以不能在异步回调里回填——
/// 只能随列表一起取回来。这条缓存**只用于展示与回填**；保存时仍然重新 GET 配置
/// 再改（见 `save_provider`），不拿这份缓存去写。
#[derive(Clone, Debug)]
pub(crate) struct ProviderRow {
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) api_key: String,
    pub(crate) model_count: usize,
}

/// 解析 `ask_user` 工具调用参数里的问题列表。
///
/// 形态**唯一**（对齐后端 `AskUserParams`）：`arguments` 里只有 `questions` 数组，
/// 每个问题 `{question, options?}`。`arguments` 在事件里可能是 JSON 字符串或对象，
/// 两种都接（Tauri 版 `parseAskUserQuestions` 同口径）。
pub(crate) fn parse_ask_user(args: &serde_json::Value) -> Vec<(String, Vec<String>)> {
    let parsed: serde_json::Value = match args {
        serde_json::Value::String(s) => serde_json::from_str(s).unwrap_or(serde_json::Value::Null),
        v => v.clone(),
    };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(qs) = parsed.get("questions").and_then(|v| v.as_array()) {
        for q in qs {
            let question = q
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let options: Vec<String> = q
                .get("options")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|o| o.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if !question.is_empty() {
                out.push((question, options));
            }
        }
    }
    // 兼容「单个问题直接给 question/options」的朴素形态（没有 questions 数组）
    if out.is_empty() {
        if let Some(q) = parsed.get("question").and_then(|v| v.as_str()) {
            let options: Vec<String> = parsed
                .get("options")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|o| o.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            out.push((q.to_string(), options));
        }
    }
    out
}

/// 待审批操作（对齐 `GET /api/v1/approval/status` 的 `pending_approvals[]`）。
#[derive(Clone, Debug)]
pub(crate) struct ApprovalRow {
    pub(crate) request_id: String,
    pub(crate) description: String,
    pub(crate) risk: String,
}

/// 后台任务面板的一行（对齐 Tauri 版 `AgentTasksPanel` 的展示字段）。
#[derive(Clone, Debug)]
pub(crate) struct TaskRow {
    /// 任务 id（`bt_xxx` 委托 / `cmd_xxx` 终端；也是子代理的**子会话 id**）
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) description: String,
    /// 归属会话 id（左栏角标按它聚合；子代理/终端任务才有）
    pub(crate) parent_session_id: String,
    /// 终端输出尾部（后端 `OUTPUT_TAIL_MAX_BYTES` 截断；委托任务为空）
    pub(crate) output_tail: String,
    /// 失败原因（`failed` 时非空）
    pub(crate) error: String,
}

/// 消息内的一个展示块（口径对齐 Tauri 版 `MessageSegments` 的分段渲染）。
///
/// **工具执行结果不在段里**：后端 `MessageSegment` 只有 thinking / text / tool
/// 三种，结果挂在消息级 `tool_calls`（`ToolCallWithResult`），按 `id` 关联。
#[derive(Clone, Debug)]
pub(crate) enum Seg {
    /// 正文（Markdown 渲染）
    Text(String),
    /// 思考过程（折叠卡片；收起时显示最后一行）
    Thinking(String),
    /// 工具调用（卡片：展示意图 + 名称 + 参数；结果按 `id` 从 `tool_calls` 取）
    Tool {
        id: String,
        name: String,
        arguments: String,
        presentation: String,
    },
}

/// 工具调用 + 执行结果（与后端 `ToolCallWithResult` 同构）。
///
/// `result` 是**完整**内容（后端不截断），`duration_ms` / `error` 由
/// `Part::ToolResult` 推导 —— 三者共同决定卡片的状态角标与展开内容。
#[derive(Clone, Debug)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: String,
    pub(crate) presentation: String,
    pub(crate) result: Option<String>,
    pub(crate) duration_ms: Option<i64>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct MsgRow {
    /// 消息 id（服务端稳定键）：回退/删除按它定位。
    /// 本地刚发出、还没落库的占位消息是空串（此时不提供回退入口）。
    pub(crate) id: String,
    pub(crate) role: String,
    /// SharedString（Arc 语义）：clone 是 O(1)，避免每帧复制正文
    pub(crate) text: gpui_kit::SharedString,
    /// 结构化分段：历史消息来自 `segments`，流式增量只有正文（segs 为空，用 text 兜底）
    pub(crate) segs: Vec<Seg>,
    /// 本条消息的工具调用（含结果）；段里的 `Tool` 按 `id` 找它取结果与耗时
    pub(crate) tool_calls: Vec<ToolCall>,
    /// 压缩点（`compression_marker`）：显示徽章，**不**渲染成用户气泡
    pub(crate) compression: bool,
    /// 本轮请求的 token 用量（`GET /messages` 每条都带）：上下文占用按会话恢复的取数点
    pub(crate) usage: Option<MsgUsage>,
    /// 用户消息携带的图片（data URL）：`data:image/png;base64,...`。
    /// 渲染时解码成 GPUI `Image`（不落盘——`Image::from_bytes` 直接吃字节）
    pub(crate) images: Vec<String>,
    /// 显示用时间（`HH:mm:ss`）。历史消息后端不下发 `timestamp`，只有本轮
    /// 新产生的 assistant 消息由本地打点（与 vite `startNewAssistantTurn` 同口径）
    pub(crate) ts: Option<String>,
}

/// 一条消息携带的 token 用量（`/messages` 的 `usage` 与流式 `usage` 块同形）。
///
/// 上下文占用 = 最近一轮请求的 `prompt_tokens`——**不是** `total_tokens`
/// （后者含输出，语义是「本轮计费」，会把占用算高）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct MsgUsage {
    pub(crate) prompt_tokens: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) cache_read: u64,
}

//! 设置页的「声明式可编辑字段」引擎：字段表 + 控件渲染 + 字段级保存。
//!
//! 为什么要有它：vite 版 14 个栏目是 14 份手写受控表单（各自维护 `onUpdateField('xxx')`
//! 的 key、控件与校验），字段一多就漂移；native 端若逐栏目手写，等于把同一套
//! 输入框/开关/校验/保存逻辑抄 10 遍。这里把「字段」声明成数据（[`FieldDef`]），
//! 渲染与保存各只写一遍：
//!
//! ```text
//! 字段表(FIELDS) → 渲染(开关/分段/数字/文本/多行/只读) → 脏检测 → 字段级保存
//! ```
//!
//! **保存语义（比 vite 更保守）**：`PUT /config` 是**整份替换**（服务端
//! `persist_and_reload`）。所以保存时先 GET 最新配置、只覆盖**真正改动过**的字段、
//! 再整份提交——把「读-改-写」的窗口压到一次往返，避免冲掉期间被别处改过的字段
//! （例如智能体刚切换的模型、MCP 预热注入的东西）。
//!
//! **需重启标注**：`PUT /config` 会热重载配置，但只有「每次读 `get_config()`」的
//! 字段立即生效；日志级别、数据目录、嵌入维度这类在启动期构造的组件不会跟着变，
//! 因此字段表里带 `restart: true` 的项在 UI 上打「需重启」徽标，保存后状态行也会
//! 汇总数量——不假装热生效。

use crate::theme::colors;
use crate::types::*;
use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::input::Textarea;
use gpui_kit::component::scroll::ScrollableElement as _;
use std::collections::HashMap;

// ─────────────────────────── 声明 ───────────────────────────

/// 控件形态。
pub(crate) enum FieldKind {
    /// 开关（布尔）。
    Bool,
    /// 分段选择（枚举）：`(配置值, 文案)`。
    Enum(&'static [(&'static str, &'static str)]),
    /// 整数（文本编辑 + 范围校验）。
    Int {
        min: i64,
        max: i64,
        unit: &'static str,
    },
    /// 浮点（文本编辑 + 范围校验）。
    Float {
        min: f64,
        max: f64,
        unit: &'static str,
    },
    /// 单行文本（`empty_as` 决定空串怎么写回配置）。
    Text {
        placeholder: &'static str,
        empty_as: EmptyAs,
    },
    /// 多行文本 ↔ 字符串数组（按行/逗号切分）。
    Lines {
        placeholder: &'static str,
        height: f32,
    },
    /// 只读展示（当前生效值，不可编辑）。
    ReadOnly,
}

/// 文本字段空值的语义。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyAs {
    /// 空串写 `null`（对应 Rust 的 `Option<...>`）。
    Null,
    /// 不允许为空（保存时拒绝）。
    Forbid,
}

/// 一个可编辑字段。
pub(crate) struct FieldDef {
    /// 所属栏目。
    pub(crate) tab: SettingsTab,
    /// 卡片分组标题（同组连续排列）。
    pub(crate) section: &'static str,
    /// `config` 内的点路径（支持一层嵌套，如 `storage.vector.collection_name`）。
    pub(crate) path: &'static str,
    /// 行标签。
    pub(crate) label: &'static str,
    /// 说明（小字，解释语义与取值范围）。
    pub(crate) hint: &'static str,
    /// 控件形态。
    pub(crate) kind: FieldKind,
    /// 改动是否需重启客户端才生效。
    pub(crate) restart: bool,
}

impl FieldKind {
    /// 是否需要文本编辑器实体（Bool/Enum/ReadOnly 不要）。
    fn has_editor(&self) -> bool {
        matches!(
            self,
            FieldKind::Int { .. }
                | FieldKind::Float { .. }
                | FieldKind::Text { .. }
                | FieldKind::Lines { .. }
        )
    }

    /// 编辑器占位文案。
    fn placeholder(&self) -> &'static str {
        match self {
            FieldKind::Int { .. } | FieldKind::Float { .. } => "数字",
            FieldKind::Text { placeholder, .. } => placeholder,
            FieldKind::Lines { placeholder, .. } => placeholder,
            _ => "",
        }
    }

    /// 编辑器高度（单行 32 / 多行自定义）。
    fn height(&self) -> f32 {
        match self {
            FieldKind::Lines { height, .. } => *height,
            _ => 32.0,
        }
    }
}

// ─────────────────────────── 字段表 ───────────────────────────

/// 全部可编辑字段（**顺序即渲染顺序**；同 `section` 的值必须连续）。
///
/// 字段名以 core 的 `TianyanConfig` 序列化口径为准（`core/src/config/*`，
/// 代码是唯一事实来源）：例如 `agent.max_turns` 默认 200、`security` 段下的
/// `safety_mode` 是 `relaxed|strict|transform|permissive`。
pub(crate) const FIELDS: &[FieldDef] = &[
    // ── 数据存储 ──
    FieldDef {
        tab: SettingsTab::Storage,
        section: "数据目录",
        path: "storage.data_dir",
        label: "数据目录",
        hint: "数据库 / 快照 / 向量索引的根目录。搬迁请用下方「搬迁数据目录」（复制 + 校验 + 改配置）。",
        kind: FieldKind::Text {
            placeholder: "%LOCALAPPDATA%\\tianyan",
            empty_as: EmptyAs::Forbid,
        },
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Storage,
        section: "数据目录",
        path: "storage.backend",
        label: "存储后端",
        hint: "主存储实现（sqlite 为生产默认；local 仅用于排障）。",
        kind: FieldKind::ReadOnly,
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Storage,
        section: "数据目录",
        path: "storage.sqlite_path",
        label: "SQLite 路径",
        hint: "空 = 数据目录下的默认位置。指向自定义文件时须保证父目录存在。",
        kind: FieldKind::ReadOnly,
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Storage,
        section: "容量与清理",
        path: "storage.max_storage_size",
        label: "容量上限",
        hint: "字节；0 = 不限制。超过后按「保留天数」清理过期产物。",
        kind: FieldKind::Int {
            min: 0,
            max: i64::MAX,
            unit: "字节",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Storage,
        section: "容量与清理",
        path: "storage.auto_cleanup",
        label: "自动清理",
        hint: "定时任务按「保留天数」回收过期数据（快照 GC / 日志轮转各有一套）。",
        kind: FieldKind::Bool,
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Storage,
        section: "容量与清理",
        path: "storage.cleanup_days",
        label: "保留天数",
        hint: "过期数据的最长保留期（天）。",
        kind: FieldKind::Int {
            min: 1,
            max: 3650,
            unit: "天",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Storage,
        section: "向量索引",
        path: "storage.vector.collection_name",
        label: "集合名",
        hint: "LanceDB 集合名（换名 = 换一套向量索引，旧索引留在原处）。",
        kind: FieldKind::Text {
            placeholder: "tianyan_data",
            empty_as: EmptyAs::Forbid,
        },
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Storage,
        section: "向量索引",
        path: "storage.vector.vector_dimension",
        label: "向量维度",
        hint: "必须与嵌入模型输出维度一致（改维度需重建索引）。",
        kind: FieldKind::Int {
            min: 64,
            max: 8192,
            unit: "维",
        },
        restart: true,
    },
    // ── Agent 行为 ──
    FieldDef {
        tab: SettingsTab::Agent,
        section: "回合与检索",
        path: "agent.max_turns",
        label: "最大轮次",
        hint: "单次任务 Agent 循环上限（工具调用轮数）。",
        kind: FieldKind::Int {
            min: 1,
            max: 500,
            unit: "轮",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Agent,
        section: "回合与检索",
        path: "agent.default_top_k",
        label: "默认检索条数",
        hint: "注入上下文时的默认 Top-K（上限 100）。",
        kind: FieldKind::Int {
            min: 1,
            max: 100,
            unit: "条",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Agent,
        section: "回合与检索",
        path: "agent.learned_rules_top_k",
        label: "习得规则条数",
        hint: "上下文管线注入的 learned rules 数量（0 = 不注入）。",
        kind: FieldKind::Int {
            min: 0,
            max: 50,
            unit: "条",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Agent,
        section: "回合与检索",
        path: "agent.background_self_review",
        label: "后台结果自审",
        hint: "后台任务完成前先经 LLM 自审（每个任务多一次小调用，未通过会标「自审未通过」）。",
        kind: FieldKind::Bool,
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Agent,
        section: "工作目录",
        path: "agent.working_directory",
        label: "基础工作目录",
        hint: "命令执行与文件读写的基础目录，也是会话回退的文件快照根。空 = 用会话工作目录。",
        kind: FieldKind::Text {
            placeholder: "F:\\work\\project",
            empty_as: EmptyAs::Null,
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Agent,
        section: "并发上限",
        path: "agent.max_background_concurrency",
        label: "委托并发",
        hint: "子智能体同时运行上限（超出进入排队）。",
        kind: FieldKind::Int {
            min: 1,
            max: 100,
            unit: "个",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Agent,
        section: "并发上限",
        path: "agent.max_background_queue",
        label: "委托排队",
        hint: "排队上限，必须 ≥ 委托并发；超出直接拒绝。",
        kind: FieldKind::Int {
            min: 1,
            max: 500,
            unit: "个",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Agent,
        section: "并发上限",
        path: "agent.max_command_concurrency",
        label: "后台命令并发",
        hint: "`execute_command background` 同时运行上限。",
        kind: FieldKind::Int {
            min: 1,
            max: 64,
            unit: "个",
        },
        restart: false,
    },
    // ── 安全 ──
    FieldDef {
        tab: SettingsTab::Security,
        section: "审批与检查",
        path: "security.approval_mode",
        label: "审批模式",
        hint: "autonomous = 黑名单外全放行（默认）；confirm = 危险操作拒绝并转为询问；interactive = 挂起等审批面板。",
        kind: FieldKind::Enum(&[
            ("autonomous", "全自主"),
            ("confirm", "确认式"),
            ("interactive", "交互式"),
        ]),
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "审批与检查",
        path: "security.safety_mode",
        label: "检查模式",
        hint: "relaxed = 跳过元字符检查、黑名单仍生效（默认）；strict = 全检查；transform = 危险命令改写为回收站；permissive = 全放开。",
        kind: FieldKind::Enum(&[
            ("relaxed", "放宽"),
            ("strict", "严格"),
            ("transform", "转换"),
            ("permissive", "放开"),
        ]),
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "审批与检查",
        path: "security.enabled",
        label: "安全层",
        hint: "关闭后命令/路径检查整体短路（仍有审批层）。",
        kind: FieldKind::Bool,
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "审批与检查",
        path: "security.audit_logging",
        label: "审计日志",
        hint: "记录每次审批判定（面板「审批」页可见）。",
        kind: FieldKind::Bool,
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "审批与检查",
        path: "security.max_file_size",
        label: "单文件上限",
        hint: "单次文件读写允许的最大字节数（默认 100MB）。",
        kind: FieldKind::Int {
            min: 0,
            max: i64::MAX,
            unit: "字节",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "路径",
        path: "security.trash_directory",
        label: "回收站目录",
        hint: "「转换」模式把危险删除改写到这个目录（留后悔药）。",
        kind: FieldKind::Text {
            placeholder: "%USERPROFILE%\\.tianyan\\trash",
            empty_as: EmptyAs::Forbid,
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "路径",
        path: "security.blocked_directories",
        label: "禁止目录",
        hint: "每行一个绝对路径；命中即拒绝（优先级高于允许目录）。",
        kind: FieldKind::Lines {
            placeholder: "C:\\Windows\\System32",
            height: 64.0,
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "路径",
        path: "security.allowed_directories",
        label: "允许目录",
        hint: "每行一个绝对路径；空 = 不限制（仍受禁止目录约束）。",
        kind: FieldKind::Lines {
            placeholder: "F:\\work",
            height: 64.0,
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "命令",
        path: "security.blocked_commands",
        label: "禁止命令",
        hint: "每行一个命令名；命中即拒绝，任何模式与自动规则都盖不住。",
        kind: FieldKind::Lines {
            placeholder: "rm\nformat\ndiskpart",
            height: 64.0,
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "命令",
        path: "security.allowed_commands",
        label: "放行命令",
        hint: "每行一个命令名；命中即自动放行（不弹审批）。",
        kind: FieldKind::Lines {
            placeholder: "ls\ngit",
            height: 64.0,
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "命令",
        path: "security.prompt_commands",
        label: "总是询问",
        hint: "每行一个命令名；命中强制走人工询问，不被自动规则覆盖。",
        kind: FieldKind::Lines {
            placeholder: "npm\ncargo",
            height: 64.0,
        },
        restart: false,
    },
    // ── 剪贴板（归在安全栏目：隐私语义一致） ──
    FieldDef {
        tab: SettingsTab::Security,
        section: "剪贴板（复制即记忆）",
        path: "clipboard.enabled",
        label: "启用监听",
        hint: "隐私敏感，默认关闭。开启后监听系统剪贴板，内容经确认后沉淀到记忆/知识库（仅本机处理）。",
        kind: FieldKind::Bool,
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "剪贴板（复制即记忆）",
        path: "clipboard.auto_capture",
        label: "自动沉淀",
        hint: "跳过确认条直接写入（默认关：先问再存）。",
        kind: FieldKind::Bool,
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Security,
        section: "剪贴板（复制即记忆）",
        path: "clipboard.prompt_confirm",
        label: "显示确认条",
        hint: "捕获后弹出确认条，可选择写入记忆/知识库或丢弃。",
        kind: FieldKind::Bool,
        restart: true,
    },
    // ── 日志 ──
    FieldDef {
        tab: SettingsTab::Logging,
        section: "级别与格式",
        path: "logging.level",
        label: "日志级别",
        hint: "trace / debug / info / warn / error。调试完记得改回 info（debug 会让日志暴涨）。",
        kind: FieldKind::Enum(&[
            ("trace", "trace"),
            ("debug", "debug"),
            ("info", "info"),
            ("warn", "warn"),
            ("error", "error"),
        ]),
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Logging,
        section: "级别与格式",
        path: "logging.format",
        label: "格式",
        hint: "text = 人读；json = 机器解析（含结构化字段）。",
        kind: FieldKind::Enum(&[("text", "text"), ("json", "json")]),
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Logging,
        section: "级别与格式",
        path: "logging.include_timestamp",
        label: "时间戳",
        hint: "每行带时间（排障时基本必开）。",
        kind: FieldKind::Bool,
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Logging,
        section: "级别与格式",
        path: "logging.include_location",
        label: "文件行号",
        hint: "每行带源码位置（定位日志来源用，略增体积）。",
        kind: FieldKind::Bool,
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Logging,
        section: "文件轮转",
        path: "logging.max_file_size",
        label: "单文件上限",
        hint: "超过后轮转（单位 MB）。",
        kind: FieldKind::Int {
            min: 1,
            max: 1024,
            unit: "MB",
        },
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Logging,
        section: "文件轮转",
        path: "logging.max_files",
        label: "保留文件数",
        hint: "轮转后保留的旧文件数量（超出删最旧）。",
        kind: FieldKind::Int {
            min: 1,
            max: 100,
            unit: "个",
        },
        restart: true,
    },
    // ── 记忆 ──
    FieldDef {
        tab: SettingsTab::Memory,
        section: "巩固",
        path: "memory.auto_consolidation",
        label: "自动巩固",
        hint: "演化综述任务在写入前做查重与合并归纳（关掉 = 记忆只增不合并）。",
        kind: FieldKind::Bool,
        restart: false,
    },
    // ── 检索 ──
    FieldDef {
        tab: SettingsTab::Retrieval,
        section: "召回",
        path: "retrieval.default_top_k",
        label: "召回条数",
        hint: "单次检索返回的结果数（>0）。",
        kind: FieldKind::Int {
            min: 1,
            max: 100,
            unit: "条",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Retrieval,
        section: "召回",
        path: "retrieval.min_score",
        label: "相关性阈值",
        hint: "0.0 – 1.0；低于阈值的结果直接丢弃（调高更精、调低更全）。",
        kind: FieldKind::Float {
            min: 0.0,
            max: 1.0,
            unit: "",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Retrieval,
        section: "召回",
        path: "retrieval.two_stage_retrieval",
        label: "两阶段检索",
        hint: "先按 L0 摘要粗筛、再按 L1 概览精排（关掉 = 只查一层，快但粗）。",
        kind: FieldKind::Bool,
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Retrieval,
        section: "召回",
        path: "retrieval.l0_multiplier",
        label: "L0 倍数",
        hint: "第一阶段多召回多少倍候选（用于重排，>0）。",
        kind: FieldKind::Int {
            min: 1,
            max: 10,
            unit: "倍",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Retrieval,
        section: "上下文",
        path: "retrieval.max_context_tokens",
        label: "注入上限",
        hint: "检索结果注入上下文的最大 token（>0，超了按分数截断）。",
        kind: FieldKind::Int {
            min: 100,
            max: 32000,
            unit: "token",
        },
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Retrieval,
        section: "缓存",
        path: "retrieval.enable_cache",
        label: "检索缓存",
        hint: "同 query 短时间内复用结果（省一次嵌入调用）。",
        kind: FieldKind::Bool,
        restart: false,
    },
    FieldDef {
        tab: SettingsTab::Retrieval,
        section: "缓存",
        path: "retrieval.cache_ttl",
        label: "缓存 TTL",
        hint: "缓存存活秒数（>0）。",
        kind: FieldKind::Int {
            min: 1,
            max: 86400,
            unit: "秒",
        },
        restart: false,
    },
    // ── Web 搜索 ──
    FieldDef {
        tab: SettingsTab::Web,
        section: "搜索后端",
        path: "web.enabled",
        label: "Web 工具",
        hint: "关闭后 web_search / web_fetch 从工具表里消失（模型看不到）。",
        kind: FieldKind::Bool,
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Web,
        section: "搜索后端",
        path: "web.search_backend",
        label: "后端",
        hint: "360 = 国内可达无降级（默认）；duckduckgo；searxng = 自建端点；deepseek = 官方搜索接口（按次计费）。",
        kind: FieldKind::Enum(&[
            ("360", "360"),
            ("duckduckgo", "DuckDuckGo"),
            ("searxng", "SearXNG"),
            ("deepseek", "DeepSeek"),
        ]),
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Web,
        section: "端点与密钥",
        path: "web.searxng_endpoint",
        label: "SearXNG 端点",
        hint: "后端选 searxng 时必填，如 https://searx.be。",
        kind: FieldKind::Text {
            placeholder: "https://searx.be",
            empty_as: EmptyAs::Null,
        },
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Web,
        section: "端点与密钥",
        path: "web.deepseek_api_key",
        label: "DeepSeek 密钥",
        hint: "后端选 deepseek 时必填（仅返回非空结果时计费）。",
        kind: FieldKind::Text {
            placeholder: "sk-…",
            empty_as: EmptyAs::Null,
        },
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Web,
        section: "端点与密钥",
        path: "web.deepseek_endpoint",
        label: "DeepSeek 端点",
        hint: "留空用官方 https://api.deepseek.com；可指向兼容网关。",
        kind: FieldKind::Text {
            placeholder: "https://api.deepseek.com",
            empty_as: EmptyAs::Null,
        },
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Web,
        section: "限额",
        path: "web.timeout_secs",
        label: "超时",
        hint: "单次搜索/抓取的 HTTP 超时（秒）。",
        kind: FieldKind::Int {
            min: 1,
            max: 300,
            unit: "秒",
        },
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Web,
        section: "限额",
        path: "web.cache_ttl_secs",
        label: "缓存 TTL",
        hint: "搜索结果与抓取内容缓存存活秒数。",
        kind: FieldKind::Int {
            min: 0,
            max: 86400,
            unit: "秒",
        },
        restart: true,
    },
    FieldDef {
        tab: SettingsTab::Web,
        section: "限额",
        path: "web.max_fetch_bytes",
        label: "抓取上限",
        hint: "单次响应体最大字节（默认 2MB，超了截断）。",
        kind: FieldKind::Int {
            min: 1024,
            max: 52428800,
            unit: "字节",
        },
        restart: true,
    },
];

// ─────────────────────────── JSON 点路径 ───────────────────────────

/// 按点路径取配置值。
pub(crate) fn json_get<'a>(
    root: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut cur = root;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// 按点路径写配置值（中间层不存在时补空对象）。
pub(crate) fn json_set(root: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let segs: Vec<&str> = path.split('.').collect();
    let mut cur = root;
    for seg in &segs[..segs.len() - 1] {
        if !cur.is_object() {
            *cur = serde_json::json!({});
        }
        // 中间层缺失时补对象（配置段一定存在，这里只是兜底）
        if cur.get(*seg).is_none() {
            if let Some(map) = cur.as_object_mut() {
                map.insert((*seg).to_string(), serde_json::json!({}));
            }
        }
        match cur.get_mut(*seg) {
            Some(next) => cur = next,
            None => return,
        }
    }
    if let Some(map) = cur.as_object_mut() {
        map.insert(segs[segs.len() - 1].to_string(), value);
    }
}

/// 配置值 → 编辑框文本（数组按行展开，null → 空串）。
fn value_to_text(v: Option<&serde_json::Value>) -> String {
    match v {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|it| match it {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
    }
}

// ─────────────────────────── impl ClientApp ───────────────────────────

impl ClientApp {
    /// 按字段表建文本编辑器实体（Bool/Enum/ReadOnly 不需要）。
    ///
    /// 为什么一次全建：GPUI 的编辑态是**实体**（`TextareaState`），而 render 是
    /// `&self`——不可能在渲染时 `cx.new` 造实体。字段表是静态的，所以启动时按表
    /// 一次建好，渲染期只按 `path` 取引用。
    pub(crate) fn create_settings_editors(
        window: &mut Window,
        cx: &mut Context<ClientApp>,
    ) -> HashMap<&'static str, Entity<TextareaState>> {
        let mut map: HashMap<&'static str, Entity<TextareaState>> = HashMap::new();
        for def in FIELDS {
            if !def.kind.has_editor() {
                continue;
            }
            let placeholder = def.kind.placeholder();
            let editor = cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder));
            map.insert(def.path, editor);
        }
        map
    }

    /// 把当前配置值回填到编辑器与 dirty 表（打开设置页 / 放弃修改时调用）。
    ///
    /// 不依赖 `render`：经窗口句柄现取 `&mut Window`。早期版本在 `Render::render`
    /// 里消费一个「待回填」标志，结果**隐藏窗口不渲染**时回填永远不发生（自动化
    /// 验收里 `TIANYAN_GUI_START_HIDDEN=1` 一跑就暴露：人设/字段全是空的）。
    pub(crate) fn backfill_settings_fields(&mut self, cx: &mut Context<Self>) {
        self.settings_dirty.clear();
        let Some(cfg) = self.raw_config.clone() else {
            return;
        };
        let editors: Vec<(Entity<TextareaState>, String)> = FIELDS
            .iter()
            .filter_map(|def| {
                let editor = self.settings_editors.get(def.path)?.clone();
                Some((editor, value_to_text(json_get(&cfg, def.path))))
            })
            .collect();
        let handle = self.window_handle;
        let applied = handle.update(cx, move |_, window, cx| {
            for (editor, text) in editors {
                editor.update(cx, |state, cx| state.set_value(text, window, cx));
            }
        });
        // 只有**真的写进编辑器**才算回填过。
        //
        // 早先无论 `update` 成败都置位：窗口句柄还不接受更新时（启动早期、窗口尚未
        // 显示），回填静默失败而标志已经 true —— 之后进设置页、点栏目都不再回填，
        // 用户看到「数据目录」等文本框永远是 placeholder（实测复现：`backfilled=True`
        // 但 `editor=''`）。标志位抢跑比不置位更糟：它把一次性失败固化成永久状态。
        if applied.is_ok() {
            self.settings_backfilled = true;
        }
        cx.notify();
    }

    /// 配置里该字段的当前值（`None` = 配置未加载或缺键）。
    fn field_config_value(&self, path: &str) -> Option<serde_json::Value> {
        self.raw_config
            .as_ref()
            .and_then(|cfg| json_get(cfg, path))
            .cloned()
    }

    /// 该字段的「编辑态值」：Bool/Enum 走 dirty 表，文本/数字走编辑器。
    ///
    /// 返回 `Result` 是因为数字/多行文本需要解析——解析失败要把**哪一项**不合法
    /// 报给用户（比笼统的「保存失败」有用）。
    fn field_edit_value(&self, def: &FieldDef, cx: &App) -> Result<serde_json::Value, String> {
        // 从未回填过（没打开过设置页）：编辑器里是建实体时的占位空串，**不是**用户
        // 输入。此时一律回落配置值，否则每个数字/文本字段都报「不能为空」：
        // ① 顶部误报「已修改 27 项」；② 点保存时 `save_settings_fields` 收集到一堆
        // 伪错误，整体中止（用户看到的是「保存失败：数据目录不能为空；…」，实际
        // 一个字段都没改）。这条与「编辑器缺失回落」是同一条防线的两道口子。
        if !self.settings_backfilled {
            return Ok(self.field_config_fallback(def.path));
        }
        match &def.kind {
            FieldKind::Bool | FieldKind::Enum(_) | FieldKind::ReadOnly => Ok(self
                .settings_dirty
                .get(def.path)
                .cloned()
                .or_else(|| self.field_config_value(def.path))
                .unwrap_or(serde_json::Value::Null)),
            FieldKind::Int { min, max, .. } => {
                let Some(raw) = self.field_editor_text(def.path, cx) else {
                    return Ok(self.field_config_fallback(def.path));
                };
                let text = raw.trim();
                if text.is_empty() {
                    return Err(format!("{}：不能为空", def.label));
                }
                let n: i64 = text
                    .parse()
                    .map_err(|_| format!("{}：需要整数（当前「{}」）", def.label, text))?;
                if n < *min || n > *max {
                    return Err(format!("{}：超出范围 {min}–{max}", def.label));
                }
                Ok(serde_json::Value::from(n))
            }
            FieldKind::Float { min, max, .. } => {
                let Some(raw) = self.field_editor_text(def.path, cx) else {
                    return Ok(self.field_config_fallback(def.path));
                };
                let text = raw.trim();
                if text.is_empty() {
                    return Err(format!("{}：不能为空", def.label));
                }
                let n: f64 = text
                    .parse()
                    .map_err(|_| format!("{}：需要数字（当前「{}」）", def.label, text))?;
                if n < *min || n > *max {
                    return Err(format!("{}：超出范围 {min}–{max}", def.label));
                }
                Ok(serde_json::Value::from(n))
            }
            FieldKind::Text { empty_as, .. } => {
                let Some(raw) = self.field_editor_text(def.path, cx) else {
                    return Ok(self.field_config_fallback(def.path));
                };
                // 单行字段：去掉换行（Textarea 的 Enter 会插 \n，对路径/端点无意义）
                let text = raw.trim().replace(['\n', '\r'], "");
                if text.is_empty() {
                    match empty_as {
                        EmptyAs::Null => Ok(serde_json::Value::Null),
                        EmptyAs::Forbid => Err(format!("{}：不能为空", def.label)),
                    }
                } else {
                    Ok(serde_json::Value::String(text))
                }
            }
            FieldKind::Lines { .. } => {
                let Some(raw) = self.field_editor_text(def.path, cx) else {
                    return Ok(self.field_config_fallback(def.path));
                };
                let items: Vec<serde_json::Value> = raw
                    .split(['\n', '\r', ','])
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect();
                Ok(serde_json::Value::Array(items))
            }
        }
    }

    /// 读编辑器当前文本；**编辑器还不存在时返回 `None`**（不是空串）。
    ///
    /// 编辑器是懒创建的：只有渲染到那个栏目才会建，而窗口隐藏时不渲染。所以
    /// 「没有编辑器」必须与「编辑器里是空串」分开：
    /// ① 当成空串 → 每个数字字段都报「不能为空」→ 设置页顶部误报
    ///    「已修改 27 项」（实测：state 里一片 `<非法值>`）；
    /// ② 更糟的是保存会把这种伪编辑态当成用户输入写进配置。
    /// 正确语义：没渲染过 = 用户不可能编辑过 = 回落到配置值（= 不脏）。
    /// 编辑器里的**原始内容**（`None` = 该字段没有编辑器实体）。
    ///
    /// 与 `field_edit_value` 的区别是**不回落配置值**：未回填时这里是空串，
    /// 而界面渲染的正是编辑器 —— 用户看到「数据目录」是一个 placeholder，
    /// 只有这一项能断言到（`/state` 的 `value` 走回落，看起来永远是好的）。
    pub(crate) fn field_editor_text(&self, path: &str, cx: &App) -> Option<String> {
        self.settings_editors
            .get(path)
            .map(|e| e.read(cx).value().to_string())
    }

    /// 无编辑器时的回落值：配置里该字段的当前值（缺失则 `null`）。
    fn field_config_fallback(&self, path: &str) -> serde_json::Value {
        self.field_config_value(path)
            .unwrap_or(serde_json::Value::Null)
    }

    /// 脏字段数量（顶部状态行用）。
    ///
    /// 判定口径与保存一致：编辑态值 ≠ 配置值即脏。文本类字段先做一次规范化
    /// （trim / 数组切分），避免「多敲一个空格」被算成改动。
    pub(crate) fn settings_dirty_count(&self, cx: &App) -> usize {
        if self.raw_config.is_none() {
            return 0;
        }
        FIELDS
            .iter()
            .filter(|def| {
                let Ok(next) = self.field_edit_value(def, cx) else {
                    // 解析失败也算「有未保存改动」，否则用户看不到任何反馈
                    return true;
                };
                let cur = self
                    .field_config_value(def.path)
                    .unwrap_or(serde_json::Value::Null);
                next != cur
            })
            .count()
    }

    /// 渲染一个栏目的全部字段（按 `section` 分组为卡片）。
    ///
    /// `extra` 是追加在字段卡片之后的栏目专属区块（如「数据存储」的搬迁卡片）——
    /// 放进同一个滚动容器，避免嵌套滚动条。
    pub(crate) fn render_settings_fields(
        &self,
        tab: SettingsTab,
        extra: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let defs: Vec<&FieldDef> = FIELDS.iter().filter(|d| d.tab == tab).collect();
        // 按 section 切组（字段表保证同 section 连续）
        let mut groups: Vec<(&'static str, Vec<&FieldDef>)> = Vec::new();
        for def in defs {
            match groups.last_mut() {
                Some((section, items)) if *section == def.section => items.push(def),
                _ => groups.push((def.section, vec![def])),
            }
        }
        let mut col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_4()
            .overflow_y_scrollbar()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(
                        "改完点右上角「保存」——只提交真正改动过的字段（整份 PUT 前会先 \
                         重读一次配置，避免冲掉别处刚改的东西）。带「需重启」的项在重启客户端后生效。",
                    ),
            );
        for (section, items) in groups {
            let mut card = div()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .p_3()
                .flex()
                .flex_col()
                .child(
                    div()
                        .pb_2()
                        .text_size(px(12.))
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child(section),
                );
            for def in items {
                card = card.child(self.render_field_row(def, cx));
            }
            col = col.child(card);
        }
        if let Some(extra) = extra {
            col = col.child(extra);
        }
        col.into_any_element()
    }

    /// 一行字段：左标签 + 说明，右控件（多行文本整行铺开）。
    fn render_field_row(&self, def: &FieldDef, cx: &mut Context<Self>) -> AnyElement {
        let path = def.path;
        let cfg = self.field_config_value(path);
        let edit = self
            .settings_dirty
            .get(path)
            .cloned()
            .unwrap_or_else(|| cfg.clone().unwrap_or(serde_json::Value::Null));

        // 标签行（所有控件共用）
        let mut label_row = div().flex().flex_row().items_center().gap_2().child(
            div()
                .text_size(px(12.))
                .text_color(rgb(colors::TEXT_PRIMARY()))
                .child(def.label),
        );
        if def.restart {
            label_row = label_row.child(
                div()
                    .px_1()
                    .rounded_sm()
                    .bg(rgba(0xffffff14))
                    .text_size(px(9.))
                    .text_color(rgb(colors::WARNING()))
                    .child("需重启"),
            );
        }
        let hint = div()
            .text_size(px(10.))
            .text_color(rgb(colors::TEXT_TERTIARY()))
            .child(def.hint);

        // 控件
        let control: AnyElement = match &def.kind {
            FieldKind::Bool => {
                let on = edit.as_bool().unwrap_or(false);
                div()
                    .id(path)
                    .w(px(40.))
                    .h(px(22.))
                    .rounded_full()
                    .bg(rgb(if on {
                        colors::ACCENT()
                    } else {
                        colors::BG_TERTIARY()
                    }))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .ml(px(if on { 21. } else { 3. }))
                            .size(px(16.))
                            .rounded_full()
                            .bg(rgb(0xffffff))
                            .into_any_element(),
                    )
                    .on_click(cx.listener(move |this, _, _w, cx| {
                        let next = !this
                            .settings_dirty
                            .get(path)
                            .and_then(|v| v.as_bool())
                            .or_else(|| this.field_config_value(path).and_then(|v| v.as_bool()))
                            .unwrap_or(false);
                        this.settings_dirty
                            .insert(path, serde_json::Value::Bool(next));
                        cx.notify();
                    }))
                    .into_any_element()
            }
            FieldKind::Enum(options) => {
                let current = edit.as_str().unwrap_or_default().to_string();
                let mut seg = div()
                    .flex()
                    .flex_row()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .overflow_hidden();
                for (ix, (value, label)) in options.iter().enumerate() {
                    let active = current == *value;
                    seg = seg.child(
                        div()
                            .id((path, ix))
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(if active {
                                colors::ACCENT()
                            } else {
                                colors::BG_PRIMARY()
                            }))
                            .text_size(px(11.))
                            .text_color(rgb(if active {
                                0xffffff
                            } else {
                                colors::TEXT_SECONDARY()
                            }))
                            .child(*label)
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.settings_dirty
                                    .insert(path, serde_json::Value::String((*value).to_string()));
                                cx.notify();
                            })),
                    );
                }
                seg.into_any_element()
            }
            FieldKind::Int { unit, .. } | FieldKind::Float { unit, .. } => {
                let editor = self.settings_editors.get(path).cloned();
                let mut row = div().flex().flex_row().items_center().gap_2().w_full();
                if let Some(editor) = editor {
                    row = row.child(editor_box(&editor, def.kind.height()));
                }
                if !unit.is_empty() {
                    row = row.child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(*unit),
                    );
                }
                row.into_any_element()
            }
            FieldKind::Text { .. } => {
                let editor = self.settings_editors.get(path).cloned();
                match editor {
                    Some(editor) => editor_box(&editor, def.kind.height()),
                    None => div().into_any_element(),
                }
            }
            FieldKind::Lines { .. } => {
                let editor = self.settings_editors.get(path).cloned();
                match editor {
                    Some(editor) => editor_box(&editor, def.kind.height()),
                    None => div().into_any_element(),
                }
            }
            FieldKind::ReadOnly => div()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child(value_to_text(cfg.as_ref()))
                .into_any_element(),
        };

        // 布局：多行字段竖排（控件占整行宽），其余横排（控件定宽 240）
        let stacked = matches!(def.kind, FieldKind::Lines { .. });
        let mut row = div()
            .py_2()
            .border_t_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .gap_3();
        if stacked {
            row = row.flex_col().child(label_row).child(hint).child(control);
        } else {
            row = row
                .flex_row()
                .items_start()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(label_row)
                        .child(hint),
                )
                .child(div().w(px(240.)).flex_shrink_0().child(control));
        }
        row.into_any_element()
    }

    /// 保存：GET 最新配置 → 只应用改动过的字段 → PUT 整份。
    pub(crate) fn save_settings_fields(&mut self, cx: &mut Context<Self>) {
        let mut patches: Vec<(&'static str, serde_json::Value)> = Vec::new();
        let mut restart_count = 0usize;
        let mut errors: Vec<String> = Vec::new();
        for def in FIELDS {
            let next = match self.field_edit_value(def, cx) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            let cur = self
                .field_config_value(def.path)
                .unwrap_or(serde_json::Value::Null);
            if next == cur {
                continue;
            }
            if def.restart {
                restart_count += 1;
            }
            patches.push((def.path, next));
        }
        if !errors.is_empty() {
            self.settings_msg = errors.join("；");
            self.settings_msg_err = true;
            self.settings_error = true;
            cx.notify();
            return;
        }
        if patches.is_empty() {
            self.settings_msg = "没有需要保存的改动".to_string();
            self.settings_msg_err = false;
            self.settings_error = false;
            cx.notify();
            return;
        }
        let count = patches.len();
        let port = self.port;
        self.settings_msg = format!("正在保存 {count} 项…");
        self.settings_msg_err = false;
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let mut cfg = match fetch_json(format!("{base}/config")).await {
                Ok(body) => body
                    .get("config")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_msg = format!("保存失败（重读配置）：{e}");
                        this.settings_msg_err = true;
                        this.settings_error = true;
                        cx.notify();
                    });
                    return;
                }
            };
            if !cfg.is_object() {
                let _ = this.update(cx, |this, cx| {
                    this.settings_msg = "保存失败：后端未返回配置对象".to_string();
                    this.settings_msg_err = true;
                    this.settings_error = true;
                    cx.notify();
                });
                return;
            }
            for (path, value) in &patches {
                json_set(&mut cfg, path, value.clone());
            }
            let res = put_json(
                format!("{base}/config"),
                serde_json::json!({ "config": cfg }),
            )
            .await;
            match res {
                Ok(_) => {
                    // 保存成功后重读：让「只读展示」与脏计数基线跟上服务端解析结果
                    let fresh = fetch_json(format!("{base}/config")).await.ok();
                    let _ = this.update(cx, |this, cx| {
                        this.settings_dirty.clear();
                        if let Some(body) = fresh {
                            if let Some(cfg) = body.get("config").cloned() {
                                this.raw_config = Some(cfg);
                            }
                        }
                        this.settings_msg = if restart_count > 0 {
                            format!("已保存 {count} 项；其中 {restart_count} 项需重启客户端后生效")
                        } else {
                            format!("已保存 {count} 项")
                        };
                        this.settings_msg_err = false;
                        this.settings_error = false;
                        cx.notify();
                    });
                    // 模型服务栏目依赖 provider 列表，顺带刷新（无害）
                    let _ = this.update(cx, |this, cx| this.load_models(cx));
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_msg = format!("保存失败：{e}");
                        this.settings_msg_err = true;
                        this.settings_error = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 放弃修改：清 dirty + 把编辑器回填成配置值。
    pub(crate) fn discard_settings_changes(&mut self, cx: &mut Context<Self>) {
        self.backfill_settings_fields(cx);
        self.settings_msg = "已放弃未保存的改动".to_string();
        self.settings_msg_err = false;
        self.settings_error = false;
        cx.notify();
    }

    /// 字段快照（自动化/调试用）：`(path, label, 编辑态文本, 配置态文本, 是否脏, 需重启)`。
    ///
    /// 为什么不直接把内部字段暴露给 `automation.rs`：脏判定的口径（规范化 / 数字解析 /
    /// 数组切分）只应该有一份实现——快照走的就是保存时用的同一套 `field_edit_value`。
    pub(crate) fn settings_field_snapshot(
        &self,
        cx: &App,
    ) -> Vec<(&'static str, &'static str, String, String, bool, bool)> {
        FIELDS
            .iter()
            .map(|def| {
                let edit = self.field_edit_value(def, cx).ok();
                let edit_text = match &edit {
                    Some(v) => value_to_text(Some(v)),
                    None => "<非法值>".to_string(),
                };
                let cfg = self.field_config_value(def.path);
                let cfg_text = value_to_text(cfg.as_ref());
                let dirty = match &edit {
                    Some(e) => Some(e) != cfg.as_ref(),
                    None => true,
                };
                (def.path, def.label, edit_text, cfg_text, dirty, def.restart)
            })
            .collect()
    }

    /// 设置某个字段的「直接值」（Bool/Enum 用；文本/数字请用 `set_settings_text`）。
    pub(crate) fn set_settings_direct_value(
        &mut self,
        path: &str,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(def) = FIELDS.iter().find(|d| d.path == path) else {
            return Err(format!("未知字段：{path}"));
        };
        if !matches!(def.kind, FieldKind::Bool | FieldKind::Enum(_)) {
            return Err(format!("{path} 是文本/数字字段，请用 set_settings_text"));
        }
        self.settings_dirty.insert(def.path, value);
        cx.notify();
        Ok(())
    }

    /// 设置某个字段的编辑器文本（文本/数字字段用；写值需 `&mut Window`，经窗口句柄取）。
    pub(crate) fn set_settings_text(
        &mut self,
        path: &str,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(editor) = self.settings_editors.get(path).cloned() else {
            return Err(format!("{path} 没有编辑器（Bool/Enum/只读字段不支持）"));
        };
        let handle = self.window_handle;
        handle
            .update(cx, move |_, window, cx| {
                editor.update(cx, |state, cx| state.set_value(text, window, cx));
            })
            .map_err(|e| format!("窗口已关闭，无法写编辑器：{e}"))?;
        cx.notify();
        Ok(())
    }
}

/// 编辑器外壳（统一边框 / 底色 / 内边距；高度固定，`Textarea` 自带外观关掉）。
fn editor_box(editor: &Entity<TextareaState>, height: f32) -> AnyElement {
    div()
        .w_full()
        .h(px(height))
        .overflow_hidden()
        .rounded_md()
        .border_1()
        .border_color(rgb(colors::BORDER()))
        .bg(rgb(colors::BG_PRIMARY()))
        .px_2()
        .child(Textarea::new(editor).appearance(false))
        .into_any_element()
}

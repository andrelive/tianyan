use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::executor::edit::ContentEdit;
use crate::executor::search::OutputMode;

/// 读取文件参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileParams {
    /// 文件路径。
    pub path: String,
    /// 起始行号（1 起始，默认 1）；大文件按需读取时用它定位目标范围。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// 返回行数上限（默认 2000，单窗口钳制至 2000）；按需读取目标范围时收窄。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// 写入文件参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileParams {
    /// 文件路径。
    pub path: String,
    /// 文件内容。
    pub content: String,
    /// 是否自动创建缺失的父目录（默认 false）。
    ///
    /// 默认**不创建**：父目录不存在即报错（防止路径写错时静默新建目录）；
    /// 确需新建目录时显式传 true。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_dirs: Option<bool>,
}

/// 应用编辑参数（内容匹配编辑，1..=20 条）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApplyEditParams {
    /// 文件路径。
    pub path: String,
    /// 编辑列表（每条 old_string 须唯一，除非 replace_all）。
    pub edits: Vec<ContentEdit>,
}

/// 执行命令参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteCommandParams {
    /// 命令。
    pub command: String,
    /// 工作目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 后台运行（true 时立即返回 task_id/log_file，进程独立运行，不等待退出）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// 后台就绪探测（**长驻服务应配**）：端口监听或日志关键词匹配后自动通知
    /// 主 agent「服务已就绪」。**必须同时给 `background:true`**——在同步命令上
    /// 配置就绪探测没有意义（会被参数校验直接拒绝，而不是静默忽略）。
    /// **就绪 = 该任务的「完成」**（长驻服务进程不退，"进程退出"不是它的完成
    /// 信号）：就绪后不再计入「等待全部完成」的阻塞（其余任务完成后即可唤醒
    /// 收尾），服务继续运行、异常退出仍会通知。不配探测则按普通任务语义
    /// （进程退出才出终态通知）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<ReadyProbeParams>,
}

/// 后台命令就绪探测参数。
///
/// `port` 与 `pattern` **至少给一个**（可同时给：任一命中即就绪，双保险）；
/// 两者都缺——含 `pattern` 为空串/纯空白——会被参数校验拒绝：否则探测永远
/// 不满足，只能静默等到总超时。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadyProbeParams {
    /// 就绪判定端口：TCP 连接成功即就绪（如 3000）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// 就绪判定日志关键词：日志出现该文本即就绪（如 "Listening"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// 首次探测等待（毫秒；指数退避起点；缺省 500）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_delay_ms: Option<u64>,
    /// 就绪探测总超时（毫秒；缺省 60000）：超时未就绪 → 通知主 agent「等待
    /// 结束」并解除完成阻塞（服务继续运行、不杀进程）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// 角色匹配建议参数（ADR-016 P3 向量路由）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SuggestRoleParams {
    /// 任务描述（委托前调用：让系统按角色摘要语义匹配给出建议）。
    pub task: String,
    /// 返回前几名（默认 3）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<usize>,
}

/// 搜索代码参数。
///
/// 字段命名对齐 ripgrep 参数：`pattern` 为正式字段名（serde alias 兼容旧载荷的
/// `query`），`path` 兼容旧 `scope`；其余为可选的 ripgrep 高级参数。
///
/// 形态收敛（依真实调用分布）：删 `line_number`（content 模式恒输出行号，
/// 该字段默认 true 且每次调用都被显式传 true——零信息量）；`before_context` /
/// `after_context` 并入 `context`（`-C` 本就同时管前后，实测从未使用）；
/// `output_mode` 类型化（非法值解析期即拒，见 [`OutputMode`]）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchCodeParams {
    /// 搜索模式（正则；兼容旧字段名 `query`）。
    #[serde(alias = "query")]
    pub pattern: String,
    /// 搜索根目录（相对路径按会话工作目录解析；缺省为会话工作目录，
    /// 无会话时回退进程当前目录；兼容旧字段名 `scope`）。
    #[serde(default, alias = "scope", skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// glob 过滤器（如 `*.rs`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// 输出模式（缺省 files_with_matches，即只列命中文件名）；
    /// 要看匹配行必须显式传 content。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<OutputMode>,
    /// 文件类型过滤器（rg --type，如 "rust"）。
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    /// 忽略大小写（-i）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_case: Option<bool>,
    /// 匹配行前后各显示的行数（-C；仅 content 模式；缺省 0）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<usize>,
    /// 结果条数上限（默认 200）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_limit: Option<usize>,
    /// 分页偏移（0 起始）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// 多行匹配（-U）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiline: Option<bool>,
}

/// 调用技能参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CallSkillParams {
    /// 技能 ID。
    pub skill_id: String,
    /// 参数。
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
}

/// 运行测试参数（**显式命令**）。
///
/// 与 [`RunProjectTestsParams`] 是**单一职责分离**的两个工具：本工具跑
/// **你给的命令**（完全控制），那个按项目类型探测构造命令。此前两者挤在同一
/// 工具里（`command` 优先、缺省才探测）——同一意图两种写法，模型每次都要选
/// （形态分叉 = 参数生成抖动源），故拆开。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunTestsParams {
    /// 测试命令（原样执行，经安全检查）：如 `cargo test --lib`、`pytest -k smoke`。
    pub command: String,
    /// 工作目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// 运行项目测试参数（**探测版**：不指定命令，按项目类型构造）。
///
/// 探测 `cwd`（缺省为会话工作目录）的项目格式：Cargo → `cargo test`、
/// Python → `pytest`、TypeScript → `vitest run`；`framework` 可覆盖探测结果，
/// `suite`/`filter` 缩小范围。项目类型无法识别时明确报错并指引用 `run_tests`。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunProjectTestsParams {
    /// 工作目录（项目探测起点；缺省为会话工作目录）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 框架覆盖："auto"（默认，按项目探测）| "cargo" | "pytest" | "vitest"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    /// 测试过滤（cargo test <filter> / pytest -k <filter>）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// 测试套件（cargo -p <suite> / pytest <suite-file>）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
}

/// 验证构建参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VerifyBuildParams {
    /// 构建命令。
    pub command: String,
    /// 工作目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// 追问选项（对齐 DSH ask_user_question：N 个选项 + 1 个自定义输入）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AskUserOption {
    /// 选项标签（用户选择后作为回答值）。
    pub label: String,
    /// 选项说明（可选；展示用，不参与回答值）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 单个追问问题（多问题分步：每个问题一个 tab，对齐 DSH questions 数组）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AskUserQuestion {
    /// 问题内容。
    pub question: String,
    /// 候选选项（可选；提供时前端渲染为选项列表 + 自定义输入）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<AskUserOption>>,
}

/// 追问用户参数。
///
/// **单一形态**（对齐 DSH `ask_user_question`）：只有 `questions` 数组——
/// 单问题即长度为 1 的数组。此前曾并行提供「单问题快捷形态」（顶层
/// `question` + `options`），于是同一意图存在两种写法，还带出「必填却被
/// 忽略」的字段（`questions` 提供时忽略 `question`）——**形态分叉是模型生成
/// 参数时的主要抖动源**（同一份意图要在两套 schema 间做选择），故收敛为一种。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AskUserParams {
    /// 问题列表（必填；每个问题一个 tab 分步追问，末尾附「补充信息」tab）。
    /// 单个问题也放入数组（长度 1）。
    pub questions: Vec<AskUserQuestion>,
}

/// 子代理提交结果参数（委托循环显式完成信号）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SubmitResultParams {
    /// 最终结果全文（Markdown 报告 / 结论）。
    pub result: String,
    /// 一句话摘要（可选；通知展示用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// 搜索知识库参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchVfsParams {
    /// 检索查询（自然语言）。
    pub query: String,
    /// 返回结果数量上限。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<usize>,
}

/// 读取 VFS 条目参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VfsReadParams {
    /// VFS URI（如 tianyan://knowledge/doc.md）。
    pub uri: String,
}

/// 列出 VFS 目录参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VfsListParams {
    /// VFS URI（如 tianyan://knowledge/），默认列出根目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// 委托子 Agent 参数。
///
/// 委托采用单轮调用：LLM 返回工具调用请求后，由外层 AgentLoop 自主决策。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DelegateToAgentParams {
    /// 子任务描述。
    pub task: String,
    /// 系统提示。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 子 Agent 最大循环轮数（默认 200，范围 1-500）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
    /// 委托整体超时（秒）。超时返回错误，子 Agent 循环被中断。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 子 Agent 角色名（内置 researcher / editor / reviewer，或 tianyan.toml
    /// `[agent_roles]` 节自定义角色）。角色提供模型、系统提示、工具白名单、
    /// max_turns 与 timeout_secs；同名参数显式指定时优先于角色值。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 子 Agent 模型名称（与主 Agent 模型同空间；优先级：
    /// 显式 model > role.model > 主 Agent 模型）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// 后台任务状态查询参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskStatusParams {
    /// 后台任务 ID（delegate_to_agent(background) 返回的 task_id）；缺省列出全部任务。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// 列表过滤（delegate | command；缺省全部）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 查询范围（U5 视野）：workspace（缺省）= 当前会话工作目录下所有会话的
    /// 任务（同目录跨会话的协调面）；global = 全局（跨目录协调专用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// 后台任务取消参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskCancelParams {
    /// 后台任务 ID。
    pub task_id: String,
}

/// 自我检查参数（无参数 — Agent 自省查询内部指标）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SelfCheckParams {}

/// 知识导入参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeIngestParams {
    /// 要导入的文件或目录路径。
    pub path: String,
    /// 可选的分类（如 technical, business, references, screenshots, photos, diagrams, code, data, projects, external）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// Web 搜索参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchParams {
    /// 搜索查询词。
    pub query: String,
    /// 返回结果上限（默认 8，最大 20）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
}

/// Web 抓取参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchParams {
    /// 目标 URL（仅 http/https；本地/内网地址被拒绝）。
    pub url: String,
    /// 返回内容字符上限（默认 50000，范围 500-200000）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_chars: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_file_params_schema() {
        let def = crate::model::FunctionDefinition::from_schema::<ReadFileParams>(
            "read_file",
            "Read a file",
        );
        assert_eq!(def.name, "read_file");
        assert!(def.parameters.get("properties").is_some());
    }

    #[test]
    fn test_ask_user_params_single_shape() {
        // 单一形态：questions 数组（单问题 = 长度 1）
        let params: AskUserParams = serde_json::from_str(
            r#"{"questions":[{"question":"确认吗？","options":[{"label":"是","description":"继续"},{"label":"否"}]}]}"#,
        )
        .unwrap();
        assert_eq!(params.questions.len(), 1);
        assert_eq!(params.questions[0].question, "确认吗？");
        assert_eq!(params.questions[0].options.as_ref().map(Vec::len), Some(2));

        // 判别力：旧「单问题快捷形态」（顶层 question）现在必须失败——
        // 该字段已移除，而 questions 为必填
        assert!(serde_json::from_str::<AskUserParams>(r#"{"question":"旧形态"}"#).is_err());

        // 序列化只产出 questions
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.starts_with(r#"{"questions":["#), "{json}");
    }
}

/// glob 查找文件参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GlobParams {
    /// glob 模式（如 `**/*.rs`、`*.toml`）。
    pub pattern: String,
    /// 搜索根目录（默认当前工作目录）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// 列出目录条目参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListDirParams {
    /// 目录路径。
    pub path: String,
    /// 分页偏移（0 起始，排序后应用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// 分页条数上限。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// 应用统一 diff 补丁参数（codex 风格 `*** Update File` 信封格式，可含多文件）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApplyPatchParams {
    /// 补丁文本（可能包含多个文件的补丁块）。
    pub patch: String,
}

/// 符号大纲提取参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SymbolOutlineParams {
    /// 源码文件路径。
    pub path: String,
}
/// 仓库结构地图参数（repo_map 工具）。
///
/// 引用度为**文本级近似**（不做名称解析：宏展开、重导出、动态分发不可见），
/// 仅用于排序权重；精确影响面靠编译器（改完跑 `verify_build`）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RepoMapParams {
    /// 扫描根目录（相对路径按会话工作目录解析；缺省会话工作目录）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// 关键字：名称命中的符号优先排序（如 "session"——聚焦某模块的子图）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    /// 骨架 token 预算（缺省 1000，上限 4000；约 4 字符/token）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    /// 是否包含测试文件（缺省 false：测试代码不进地图）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_tests: Option<bool>,
}

/// 发现测试参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscoverTestsParams {
    /// 项目根目录（或项目内任意目录，向上探测）。
    pub path: String,
}

/// 执行统计查询参数（execution_stats；GEPA 数据层，ADR-017）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionStatsParams {
    /// 起始时间（RFC3339，如 2026-08-18T00:00:00Z；缺省 = 全部历史）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// 类别过滤（file_operation / code_operation / search_operation /
    /// test_operation / deploy_operation / analysis_operation / general_operation）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// 执行明细查询参数（execution_detail；GEPA 数据层，ADR-017）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionDetailParams {
    /// 起始时间（RFC3339；缺省 = 全部历史）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// 类别过滤。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// 最大返回条数（默认 20，上限 50）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// 角色委托统计查询参数（delegation_stats；GEPA 数据层，ADR-017）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DelegationStatsParams {
    /// 起始时间（RFC3339；缺省 = 全部历史）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

/// 会话回忆查询参数（session_recall；FTS5 倒排索引，ADR-017 决策 6）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionRecallParams {
    /// 回忆关键词（中文子串匹配，无需分词；至少 3 个字符）。
    pub query: String,
    /// 最大命中数（默认 5，上限 20）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// 每个命中附近的窗口半径（前后各 N 条消息，默认 5）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<i64>,
    /// 只看最近 N 天的消息（缺省不限；用于"最近/这几天"类回忆）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_days: Option<u32>,
}

mod actions;
/// 命令执行（ExecuteCommand 动作，无安全策略依赖）。
pub mod command;
/// 命令输出解析工具（构建错误提取 / 测试输出解析）。
mod output_parse;
/// Executor 安全策略（`pub(crate)`：`check_path_rules` 供 skills 路径沙箱复用）。
pub(crate) mod security;

/// 审批工作流模块。
pub mod approval;
/// LLM-as-Judge 语义验证模块。
pub mod judge;
/// 执行器类型定义。
pub mod types;
/// 验证门控模块。
pub mod verification;

/// 内容匹配编辑（apply_edit：old_string/new_string 唯一匹配，原子批量）。
pub mod edit;
/// 文件系统浏览工具核心逻辑（glob 查找 / list_dir 目录列出）。
pub mod fs;
/// 统一 diff 补丁解析与应用（apply_patch：codex 风格 `*** Update File` 信封）。
pub mod patch;
/// 项目格式探测注册表（verify_build / discover_tests 共享）。
pub mod project;
/// 代码搜索执行器（内嵌引擎：输出守卫 + 分页语义）。
pub mod search;
/// 内嵌搜索引擎实现（ignore 遍历 + regex 匹配，替代外部 ripgrep）。
pub mod search_engine;
/// 符号大纲提取引擎（tree-sitter）。
pub mod symbols;
/// 测试发现与测试结果解析（discover_tests 工具 + run_tests 结果增强）。
pub mod test_discovery;
/// 统一截断层（read/grep 与命令输出模式的 UTF-8 安全截断）。
pub mod truncate;
/// Web 工具执行器（web_search / web_fetch：搜索后端 + SSRF 防护 + 缓存）。
pub mod web;

pub use actions::{
    execute_command_action, execute_command_action_cancellable, execute_read_file,
    execute_verify_build, execute_write_file, kill_all_running_children, CommandEventSink,
    CommandManager, CommandNotifier, CommandTask, CommandTaskStatus, CommandWaker, SecurityPolicy,
};
pub use judge::LlmJudge;
pub use security::DEFAULT_BLOCKED_COMMANDS;
pub use types::Action;
pub use verification::{VerificationGate, VerificationResult};

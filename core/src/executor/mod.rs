mod actions;
/// 命令执行（ExecuteCommand 动作，无安全策略依赖）。
pub mod command;
/// 命令输出解析工具（构建错误提取 / 测试输出解析）。
mod output_parse;
/// Executor 安全策略（`pub(crate)`：`check_path_rules` 供 skills 路径沙箱复用）。
pub(crate) mod security;
/// 命令执行底层（shell provider）：解析/探测/提示词（ADR-037）。
pub mod shell;

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
pub use shell::{ShellKind, ShellSpec};
pub use types::Action;
pub use verification::{VerificationGate, VerificationResult};

/// 原子写文件（同目录临时文件 + rename 替换）——避免“写一半”留下损坏文件。
///
/// 用途：工具对**用户源码**的写入（`write_file` / `apply_edit` / `apply_patch`）。
/// `fs::write` 是“截断原文件后写入”：中途崩溃 / 断电 / 磁盘满 → 文件残缺，
/// 而内容是用户代码，代价高。临时文件与目标**同目录**（保证 rename 在同一
/// 文件系统上原子）；rename 在 Windows 上覆盖已存在文件（Rust 用 MoveFileEx）。
///
/// 失败语义：任何一步失败都清理临时文件并返回错误，**原文件保持不变**。
///
/// `create_dirs`：父目录缺失时是否自动创建。`write_file` 默认 `false`——
/// 路径写错（本该写已有目录却拼了新目录名）时宁可报错，也不静默新建目录；
/// 「缺目录报错 + 修法指引」由工具层（`file_ops::execute_write_file`）给出。
pub(crate) async fn write_file_atomic(
    path: impl AsRef<std::path::Path>,
    content: &str,
    create_dirs: bool,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let path = path.as_ref();
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    if create_dirs {
        tokio::fs::create_dir_all(dir).await?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let tmp = dir.join(format!(
        ".{name}.tianyan-tmp-{}",
        uuid::Uuid::new_v4().simple()
    ));

    let mut file = tokio::fs::File::create(&tmp).await?;
    if let Err(e) = file.write_all(content.as_bytes()).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    let _ = file.flush().await;
    drop(file);

    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod atomic_tests {
    use super::*;

    async fn leftovers(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("tianyan-tmp"))
            .collect()
    }

    #[tokio::test]
    async fn test_write_file_atomic_creates_and_replaces_without_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.txt");

        write_file_atomic(&target, "第一版", false).await.unwrap();
        assert_eq!(tokio::fs::read_to_string(&target).await.unwrap(), "第一版");

        write_file_atomic(&target, "第二版内容", false)
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&target).await.unwrap(),
            "第二版内容"
        );
        assert!(
            leftovers(dir.path()).await.is_empty(),
            "不应残留临时文件：{:?}",
            leftovers(dir.path()).await
        );
    }

    #[tokio::test]
    async fn test_write_file_atomic_failure_keeps_target_and_cleans_tmp() {
        // 目标是**目录**：rename 必然失败 → 原目录不受影响 + 无临时文件残留
        let dir = tempfile::tempdir().unwrap();
        let target_dir = dir.path().join("sub");
        std::fs::create_dir(&target_dir).unwrap();

        assert!(write_file_atomic(&target_dir, "x", false).await.is_err());
        assert!(target_dir.is_dir(), "失败时目标目录应保持不变");
        assert!(
            leftovers(dir.path()).await.is_empty(),
            "失败后不应残留临时文件：{:?}",
            leftovers(dir.path()).await
        );
    }

    #[tokio::test]
    async fn test_write_file_atomic_create_dirs_flag() {
        // 默认（false）：父目录缺失 → 报错，不静默建目录
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested").join("deep").join("a.txt");
        assert!(
            write_file_atomic(&target, "x", false).await.is_err(),
            "默认不应创建缺失的父目录"
        );
        assert!(!target.parent().unwrap().exists(), "失败后不应留下目录");

        // 显式 true：创建父目录并写入
        write_file_atomic(&target, "内容", true).await.unwrap();
        assert_eq!(tokio::fs::read_to_string(&target).await.unwrap(), "内容");
        assert!(target.parent().unwrap().is_dir());
    }
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::Result;

use crate::executor::fs::execute_list_dir;
use crate::skills::definition::SkillHandler;
use crate::skills::executor::validate_path;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 文件列表处理器。
///
/// 薄 adapter：沙箱（allowed_paths / max_entries / timeout）与文本输出契约在此层，
/// 实际列举委托 [`execute_list_dir`]（目录优先排序、分页——与 list_dir 工具同实现）。
pub struct FileListHandler {
    allowed_paths: Vec<PathBuf>,
    /// 单次列表最大条目数，默认 10000。
    max_entries: usize,
    /// 列表操作超时（秒），默认 15。
    timeout_secs: u64,
}

impl FileListHandler {
    /// 创建新的文件列表处理器。
    pub fn new(allowed_paths: Vec<PathBuf>) -> Self {
        Self {
            allowed_paths,
            max_entries: 10000,
            timeout_secs: 15,
        }
    }

    /// 设置最大条目数。
    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    /// 设置超时。
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

impl Default for FileListHandler {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[async_trait]
impl SkillHandler for FileListHandler {
    async fn execute(
        &self,
        params: HashMap<String, Value>,
        _context: ExecutionContext,
    ) -> Result<SkillExecutionResult> {
        let start = Instant::now();

        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        let path = PathBuf::from(path);
        validate_path(&path, &self.allowed_paths)?;

        let timeout = Duration::from_secs(self.timeout_secs);
        let list_op = execute_list_dir(&path, None, Some(self.max_entries));
        match tokio::time::timeout(timeout, list_op).await {
            Ok(Ok(output)) => {
                let mut lines: Vec<String> =
                    output.entries.iter().map(|e| e.name.clone()).collect();
                if output.truncated {
                    lines.push("... (已达到条目上限)".to_string());
                }
                Ok(super::result_success(lines.join("\n"), start))
            }
            Ok(Err(e)) => Ok(super::result_failure(e.to_string(), start)),
            Err(_) => Ok(super::result_timeout(self.timeout_secs, start)),
        }
    }

    fn skill_id(&self) -> &str {
        "file_list"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn params(path: &str) -> HashMap<String, Value> {
        HashMap::from([("path".to_string(), Value::String(path.to_string()))])
    }

    #[tokio::test]
    async fn test_list_directory_lists_files_and_dirs() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let handler = FileListHandler::new(vec![dir.path().to_path_buf()]);
        let result = handler
            .execute(
                params(dir.path().to_str().unwrap()),
                ExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.success);
        let output = result.output.unwrap();
        assert!(output.contains("a.txt"));
        assert!(output.contains("b.txt"));
        // 子目录带 / 后缀
        assert!(output.contains("sub/"), "子目录应带 / 后缀: {output}");
    }

    #[tokio::test]
    async fn test_list_defaults_to_current_dir() {
        // 缺省 path 参数时使用 "."，应成功返回而不是报错
        let handler = FileListHandler::new(Vec::new());
        let result = handler
            .execute(HashMap::new(), ExecutionContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.is_some());
    }

    #[tokio::test]
    async fn test_list_outside_allowed_denied() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s").unwrap();

        let handler = FileListHandler::new(vec![dir.path().to_path_buf()]);
        let err = handler
            .execute(
                params(outside.path().to_str().unwrap()),
                ExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("不在允许的操作范围内"),
            "越权列目录必须被拒绝: {err}"
        );
    }

    #[tokio::test]
    async fn test_list_max_entries_capped() {
        let dir = tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "x").unwrap();
        }

        let handler = FileListHandler::new(vec![dir.path().to_path_buf()]).with_max_entries(2);
        let result = handler
            .execute(
                params(dir.path().to_str().unwrap()),
                ExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.success);
        let output = result.output.unwrap();
        assert!(
            output.contains("已达到条目上限"),
            "超过条目上限时应截断并提示: {output}"
        );
    }

    #[tokio::test]
    async fn test_list_nonexistent_dir_returns_failure() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");

        let handler = FileListHandler::new(vec![dir.path().to_path_buf()]);
        let result = handler
            .execute(
                params(missing.to_str().unwrap()),
                ExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap().contains("目录不存在"),
            "错误信息应说明目录不存在: {:?}",
            result.error
        );
    }

    #[test]
    fn test_skill_id() {
        assert_eq!(FileListHandler::new(Vec::new()).skill_id(), "file_list");
    }
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::executor::validate_path;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 文件删除处理器。
pub struct FileDeleteHandler {
    allowed_paths: Vec<PathBuf>,
}

impl FileDeleteHandler {
    /// 创建新的文件删除处理器。
    pub fn new(allowed_paths: Vec<PathBuf>) -> Self {
        Self { allowed_paths }
    }
}

impl Default for FileDeleteHandler {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[async_trait]
impl SkillHandler for FileDeleteHandler {
    async fn execute(
        &self,
        params: HashMap<String, Value>,
        _context: ExecutionContext,
    ) -> Result<SkillExecutionResult> {
        let start = Instant::now();

        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TianyanError::Custom("[file_delete] 缺少 'path' 参数".to_string()))?;

        let path = PathBuf::from(path);
        validate_path(&path, &self.allowed_paths)?;

        let result = if path.is_dir() {
            tokio::fs::remove_dir_all(&path).await
        } else {
            tokio::fs::remove_file(&path).await
        };

        match result {
            Ok(()) => Ok(super::result_success(
                format!("成功删除 {}", path.display()),
                start,
            )),
            Err(e) => Ok(super::result_failure(format!("删除失败: {}", e), start)),
        }
    }

    fn skill_id(&self) -> &str {
        "file_delete"
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
    async fn test_delete_file_success() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("victim.txt");
        std::fs::write(&file, "x").unwrap();

        let handler = FileDeleteHandler::new(vec![dir.path().to_path_buf()]);
        let result = handler
            .execute(params(file.to_str().unwrap()), ExecutionContext::default())
            .await
            .unwrap();

        assert!(result.success);
        assert!(!file.exists(), "文件应被删除");
    }

    #[tokio::test]
    async fn test_delete_directory_recursive() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(sub.join("deep")).unwrap();
        std::fs::write(sub.join("deep").join("f.txt"), "x").unwrap();

        let handler = FileDeleteHandler::new(vec![dir.path().to_path_buf()]);
        let result = handler
            .execute(params(sub.to_str().unwrap()), ExecutionContext::default())
            .await
            .unwrap();

        assert!(result.success);
        assert!(!sub.exists(), "目录应被递归删除");
    }

    #[tokio::test]
    async fn test_delete_missing_path_param() {
        let handler = FileDeleteHandler::new(Vec::new());
        let err = handler
            .execute(HashMap::new(), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("缺少 'path'"));
    }

    #[tokio::test]
    async fn test_delete_outside_allowed_denied() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let file = outside.path().join("precious.txt");
        std::fs::write(&file, "keep me").unwrap();

        let handler = FileDeleteHandler::new(vec![dir.path().to_path_buf()]);
        let err = handler
            .execute(params(file.to_str().unwrap()), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("不在允许的操作范围内"),
            "越权删除必须被拒绝: {err}"
        );
        assert!(file.exists(), "文件不应被删除");
    }

    #[tokio::test]
    async fn test_delete_nonexistent_returns_failure() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("ghost.txt");

        let handler = FileDeleteHandler::new(vec![dir.path().to_path_buf()]);
        let result = handler
            .execute(
                params(missing.to_str().unwrap()),
                ExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_skill_id() {
        assert_eq!(FileDeleteHandler::new(Vec::new()).skill_id(), "file_delete");
    }
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::executor::validate_path;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 文件写入处理器。
pub struct FileWriteHandler {
    allowed_paths: Vec<PathBuf>,
    /// 单次写入最大内容大小（字节），默认 10MB。
    max_size: u64,
    /// 写入超时（秒），默认 30。
    timeout_secs: u64,
}

impl FileWriteHandler {
    /// 创建新的文件写入处理器。
    pub fn new(allowed_paths: Vec<PathBuf>) -> Self {
        Self {
            allowed_paths,
            max_size: 10 * 1024 * 1024,
            timeout_secs: 30,
        }
    }

    /// 设置最大文件大小。
    pub fn with_max_size(mut self, max_size: u64) -> Self {
        self.max_size = max_size;
        self
    }

    /// 设置写入超时。
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

impl Default for FileWriteHandler {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[async_trait]
impl SkillHandler for FileWriteHandler {
    async fn execute(
        &self,
        params: HashMap<String, Value>,
        _context: ExecutionContext,
    ) -> Result<SkillExecutionResult> {
        let start = Instant::now();

        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TianyanError::Custom("[file_write] 缺少 'path' 参数".to_string()))?;

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TianyanError::Custom("[file_write] 缺少 'content' 参数".to_string()))?;

        // Size check
        let content_len = content.len() as u64;
        if content_len > self.max_size {
            return Err(TianyanError::Custom(format!(
                "操作不被允许：内容大小 {} 超过写入上限 {} 字节",
                content_len, self.max_size
            )));
        }

        let path = PathBuf::from(path);
        validate_path(&path, &self.allowed_paths)?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| TianyanError::Custom(format!("技能执行错误：创建目录失败: {}", e)))?;
        }

        let timeout = Duration::from_secs(self.timeout_secs);
        let write_op = async {
            tokio::fs::write(&path, content)
                .await
                .map_err(|e| TianyanError::Custom(format!("技能执行错误：写入文件失败: {}", e)))
        };

        match tokio::time::timeout(timeout, write_op).await {
            Ok(Ok(())) => Ok(super::result_success(
                format!("成功写入 {} 字节到 {}", content_len, path.display()),
                start,
            )),
            Ok(Err(e)) => Ok(super::result_failure(e.to_string(), start)),
            Err(_) => Ok(super::result_timeout(self.timeout_secs, start)),
        }
    }

    fn skill_id(&self) -> &str {
        "file_write"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn params(path: &str, content: &str) -> HashMap<String, Value> {
        HashMap::from([
            ("path".to_string(), Value::String(path.to_string())),
            ("content".to_string(), Value::String(content.to_string())),
        ])
    }

    #[tokio::test]
    async fn test_write_file_success() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("out.txt");

        let handler = FileWriteHandler::new(vec![dir.path().to_path_buf()]);
        let result = handler
            .execute(
                params(file.to_str().unwrap(), "content 内容"),
                ExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "content 内容",
            "文件应被真实写入"
        );
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("nested").join("deep").join("out.txt");

        let handler = FileWriteHandler::new(vec![dir.path().to_path_buf()]);
        let result = handler
            .execute(
                params(file.to_str().unwrap(), "x"),
                ExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
    }

    #[tokio::test]
    async fn test_write_missing_params() {
        let handler = FileWriteHandler::new(Vec::new());
        // 缺 path
        let err = handler
            .execute(
                HashMap::from([("content".to_string(), Value::String("x".into()))]),
                ExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("缺少 'path'"));

        // 缺 content
        let err = handler
            .execute(
                HashMap::from([("path".to_string(), Value::String("x".into()))]),
                ExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("缺少 'content'"));
    }

    #[tokio::test]
    async fn test_write_outside_allowed_denied() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let file = outside.path().join("hacked.txt");

        let handler = FileWriteHandler::new(vec![dir.path().to_path_buf()]);
        let err = handler
            .execute(
                params(file.to_str().unwrap(), "pwn"),
                ExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("不在允许的操作范围内"),
            "越权写入必须被拒绝: {err}"
        );
        assert!(!file.exists(), "文件不应被创建");
    }

    #[tokio::test]
    async fn test_write_exceeds_max_size_rejected() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("big.txt");

        let handler = FileWriteHandler::new(vec![dir.path().to_path_buf()]).with_max_size(10);
        let err = handler
            .execute(
                params(file.to_str().unwrap(), "x".repeat(100).as_str()),
                ExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("超过写入上限"));
        assert!(!file.exists(), "超限内容不应写入");
    }

    #[tokio::test]
    async fn test_write_to_directory_returns_failure() {
        let dir = tempdir().unwrap();
        // 目标是已存在的目录：写入必定失败（EISDIR/拒绝访问），应降级为失败结果而非崩溃
        let handler = FileWriteHandler::new(vec![dir.path().to_path_buf()]);
        let result = handler
            .execute(
                params(dir.path().to_str().unwrap(), "x"),
                ExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_skill_id() {
        assert_eq!(FileWriteHandler::new(Vec::new()).skill_id(), "file_write");
    }
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::executor::validate_path;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 文件读取处理器。
pub struct FileReadHandler {
    allowed_paths: Vec<PathBuf>,
    /// 单次读取最大文件大小（字节），默认 50MB。
    max_file_size: u64,
    /// 读取超时（秒），默认 30。
    timeout_secs: u64,
}

impl FileReadHandler {
    /// 创建新的文件读取处理器。
    pub fn new(allowed_paths: Vec<PathBuf>) -> Self {
        Self {
            allowed_paths,
            max_file_size: 50 * 1024 * 1024,
            timeout_secs: 30,
        }
    }

    /// 设置最大文件大小。
    pub fn with_max_size(mut self, max_size: u64) -> Self {
        self.max_file_size = max_size;
        self
    }

    /// 设置读取超时。
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

impl Default for FileReadHandler {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[async_trait]
impl SkillHandler for FileReadHandler {
    async fn execute(
        &self,
        params: HashMap<String, Value>,
        _context: ExecutionContext,
    ) -> Result<SkillExecutionResult> {
        let start = Instant::now();

        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TianyanError::Custom("[file_read] 缺少 'path' 参数".to_string()))?;

        let path = PathBuf::from(path);
        validate_path(&path, &self.allowed_paths)?;

        // Check file size before reading
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|e| TianyanError::Custom(format!("技能执行错误：无法获取文件信息: {}", e)))?;
        if metadata.len() > self.max_file_size {
            return Err(TianyanError::Custom(format!(
                "操作不被允许：文件大小 {} 超过读取上限 {} 字节",
                metadata.len(),
                self.max_file_size
            )));
        }

        let timeout = Duration::from_secs(self.timeout_secs);
        match tokio::time::timeout(timeout, tokio::fs::read_to_string(&path)).await {
            Ok(Ok(content)) => Ok(super::result_success(content, start)),
            Ok(Err(e)) => Ok(super::result_failure(format!("读取文件失败: {}", e), start)),
            Err(_) => Ok(super::result_timeout(self.timeout_secs, start)),
        }
    }

    fn skill_id(&self) -> &str {
        "file_read"
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
    async fn test_read_file_success() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "hello 天演").unwrap();

        let handler = FileReadHandler::new(vec![dir.path().to_path_buf()]);
        let result = handler
            .execute(params(file.to_str().unwrap()), ExecutionContext::default())
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output.as_deref(), Some("hello 天演"));
    }

    #[tokio::test]
    async fn test_read_missing_path_param() {
        let handler = FileReadHandler::new(Vec::new());
        let err = handler
            .execute(HashMap::new(), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("缺少 'path'"));
    }

    #[tokio::test]
    async fn test_read_path_outside_allowed_denied() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let file = outside.path().join("secret.txt");
        std::fs::write(&file, "secret").unwrap();

        let handler = FileReadHandler::new(vec![dir.path().to_path_buf()]);
        let err = handler
            .execute(params(file.to_str().unwrap()), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("不在允许的操作范围内"),
            "应为越权错误: {err}"
        );
    }

    #[tokio::test]
    async fn test_read_path_traversal_denied() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let file = outside.path().join("secret.txt");
        std::fs::write(&file, "secret").unwrap();

        // 通过相对路径 ../ 尝试逃逸 allowed_paths
        let allowed = dir.path().join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        let traversal = format!("{}{}", allowed.to_str().unwrap(), std::path::MAIN_SEPARATOR);
        // 构造 ../../<tempdir>/secret.txt 形式的相对路径
        let rel = format!(
            "..{0}..{0}{1}",
            std::path::MAIN_SEPARATOR,
            file.to_str().unwrap()
        );
        let full = format!("{traversal}{rel}");

        let handler = FileReadHandler::new(vec![allowed.clone()]);
        let result = handler
            .execute(params(&full), ExecutionContext::default())
            .await;
        // 规范化后应判定不在允许范围（或解析失败），绝不能读到外部文件
        assert!(
            result.is_err() || !result.unwrap().success,
            "路径穿越必须被拒绝"
        );
    }

    #[tokio::test]
    async fn test_read_exceeds_max_size_rejected() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("big.txt");
        std::fs::write(&file, "x".repeat(1024)).unwrap();

        let handler = FileReadHandler::new(vec![dir.path().to_path_buf()]).with_max_size(100);
        let err = handler
            .execute(params(file.to_str().unwrap()), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("超过读取上限"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file_returns_error() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing.txt");

        let handler = FileReadHandler::new(vec![dir.path().to_path_buf()]);
        let err = handler
            .execute(
                params(missing.to_str().unwrap()),
                ExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("无法获取文件信息"),
            "错误信息应说明无法获取文件信息: {err}"
        );
    }

    #[test]
    fn test_skill_id() {
        assert_eq!(FileReadHandler::new(Vec::new()).skill_id(), "file_read");
    }
}

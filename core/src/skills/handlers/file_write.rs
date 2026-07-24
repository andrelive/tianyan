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

        let path = params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            TianyanError::InvalidSkillParameters {
                skill: "file_write".to_string(),
                message: "缺少 'path' 参数".to_string(),
            }
        })?;

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TianyanError::InvalidSkillParameters {
                skill: "file_write".to_string(),
                message: "缺少 'content' 参数".to_string(),
            })?;

        // Size check
        let content_len = content.len() as u64;
        if content_len > self.max_size {
            return Err(TianyanError::OperationNotAllowed(format!(
                "内容大小 {} 超过写入上限 {} 字节",
                content_len, self.max_size
            )));
        }

        let path = PathBuf::from(path);
        validate_path(&path, &self.allowed_paths)?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| TianyanError::SkillExecution(format!("创建目录失败: {}", e)))?;
        }

        let timeout = Duration::from_secs(self.timeout_secs);
        let write_op = async {
            tokio::fs::write(&path, content).await.map_err(|e| {
                TianyanError::SkillExecution(format!("写入文件失败: {}", e))
            })
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

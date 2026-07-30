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

        let path = params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            TianyanError::Custom("[file_read] 缺少 'path' 参数".to_string())
        })?;

        let path = PathBuf::from(path);
        validate_path(&path, &self.allowed_paths)?;

        // Check file size before reading
        let metadata = tokio::fs::metadata(&path).await.map_err(|e| {
            TianyanError::Custom(format!("技能执行错误：无法获取文件信息: {}", e))
        })?;
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

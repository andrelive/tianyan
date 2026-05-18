use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::executor::validate_path;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 文件写入处理器。
pub struct FileWriteHandler {
    allowed_paths: Vec<PathBuf>,
}

impl FileWriteHandler {
    /// 创建新的文件写入处理器。
    pub fn new(allowed_paths: Vec<PathBuf>) -> Self {
        Self { allowed_paths }
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

        let path = PathBuf::from(path);
        validate_path(&path, &self.allowed_paths)?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| TianyanError::SkillExecution(format!("创建目录失败: {}", e)))?;
        }

        match tokio::fs::write(&path, content).await {
            Ok(()) => Ok(SkillExecutionResult {
                success: true,
                output: Some(format!(
                    "成功写入 {} 字节到 {}",
                    content.len(),
                    path.display()
                )),
                error: None,
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
            Err(e) => Ok(SkillExecutionResult {
                success: false,
                output: None,
                error: Some(format!("写入文件失败: {}", e)),
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
        }
    }

    fn skill_id(&self) -> &str {
        "file_write"
    }
}

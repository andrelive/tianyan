use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::executor::validate_path;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 文件读取处理器。
pub struct FileReadHandler {
    allowed_paths: Vec<PathBuf>,
}

impl FileReadHandler {
    /// 创建新的文件读取处理器。
    pub fn new(allowed_paths: Vec<PathBuf>) -> Self {
        Self { allowed_paths }
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
            TianyanError::InvalidSkillParameters {
                skill: "file_read".to_string(),
                message: "缺少 'path' 参数".to_string(),
            }
        })?;

        let path = PathBuf::from(path);
        validate_path(&path, &self.allowed_paths)?;

        match tokio::fs::read_to_string(&path).await {
            Ok(content) => Ok(SkillExecutionResult {
                success: true,
                output: Some(content),
                error: None,
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
            Err(e) => Ok(SkillExecutionResult {
                success: false,
                output: None,
                error: Some(format!("读取文件失败: {}", e)),
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
        }
    }

    fn skill_id(&self) -> &str {
        "file_read"
    }
}

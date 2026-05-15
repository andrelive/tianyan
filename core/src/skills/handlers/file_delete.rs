use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::executor::validate_path;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

pub struct FileDeleteHandler {
    allowed_paths: Vec<PathBuf>,
}

impl FileDeleteHandler {
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

        let path = params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            TianyanError::InvalidSkillParameters {
                skill: "file_delete".to_string(),
                message: "缺少 'path' 参数".to_string(),
            }
        })?;

        let path = PathBuf::from(path);
        validate_path(&path, &self.allowed_paths)?;

        let result = if path.is_dir() {
            tokio::fs::remove_dir_all(&path).await
        } else {
            tokio::fs::remove_file(&path).await
        };

        match result {
            Ok(()) => Ok(SkillExecutionResult {
                success: true,
                output: Some(format!("成功删除 {}", path.display())),
                error: None,
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
            Err(e) => Ok(SkillExecutionResult {
                success: false,
                output: None,
                error: Some(format!("删除失败: {}", e)),
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
        }
    }

    fn skill_id(&self) -> &str {
        "file_delete"
    }
}

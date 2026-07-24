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
            Ok(()) => Ok(super::result_success(format!("成功删除 {}", path.display()), start)),
            Err(e) => Ok(super::result_failure(format!("删除失败: {}", e), start)),
        }
    }

    fn skill_id(&self) -> &str {
        "file_delete"
    }
}

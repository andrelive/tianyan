use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::Result;

use crate::skills::definition::SkillHandler;
use crate::skills::executor::validate_path;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 文件列表处理器。
pub struct FileListHandler {
    allowed_paths: Vec<PathBuf>,
}

impl FileListHandler {
    /// 创建新的文件列表处理器。
    pub fn new(allowed_paths: Vec<PathBuf>) -> Self {
        Self { allowed_paths }
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

        match tokio::fs::read_dir(&path).await {
            Ok(mut entries) => {
                let mut files = Vec::new();
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                    files.push(if is_dir { format!("{}/", name) } else { name });
                }

                Ok(SkillExecutionResult {
                    success: true,
                    output: Some(files.join("\n")),
                    error: None,
                    exit_code: None,
                    execution_time_ms: start.elapsed().as_millis() as u64,
                    data: HashMap::new(),
                })
            }
            Err(e) => Ok(SkillExecutionResult {
                success: false,
                output: None,
                error: Some(format!("列出目录失败: {}", e)),
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
        }
    }

    fn skill_id(&self) -> &str {
        "file_list"
    }
}

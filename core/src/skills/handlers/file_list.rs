use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::executor::validate_path;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 文件列表处理器。
pub struct FileListHandler {
    allowed_paths: Vec<PathBuf>,
    /// 单次列表最大条目数，默认 10000。
    max_entries: usize,
    /// 列表操作超时（秒），默认 15。
    timeout_secs: u64,
}

impl FileListHandler {
    /// 创建新的文件列表处理器。
    pub fn new(allowed_paths: Vec<PathBuf>) -> Self {
        Self {
            allowed_paths,
            max_entries: 10000,
            timeout_secs: 15,
        }
    }

    /// 设置最大条目数。
    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    /// 设置超时。
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
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

        let timeout = Duration::from_secs(self.timeout_secs);
        let list_op = async {
            match tokio::fs::read_dir(&path).await {
                Ok(mut entries) => {
                    let mut files = Vec::new();
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        if files.len() >= self.max_entries {
                            files.push("... (已达到条目上限)".to_string());
                            break;
                        }
                        let name = entry.file_name().to_string_lossy().to_string();
                        let is_dir =
                            entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                        files.push(if is_dir { format!("{}/", name) } else { name });
                    }
                    Ok(files)
                }
                Err(e) => Err(TianyanError::Custom(format!(
                    "技能执行错误：列出目录失败: {}",
                    e
                ))),
            }
        };

        match tokio::time::timeout(timeout, list_op).await {
            Ok(Ok(files)) => Ok(super::result_success(files.join("\n"), start)),
            Ok(Err(e)) => Ok(super::result_failure(e.to_string(), start)),
            Err(_) => Ok(super::result_timeout(self.timeout_secs, start)),
        }
    }

    fn skill_id(&self) -> &str {
        "file_list"
    }
}

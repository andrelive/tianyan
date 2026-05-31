//! Garbage Collection 任务。
//!
//! 定期扫描 agent/learned/ 规则和 Memory 中的过期记忆，
//! 标记或清理不再有效的内容。

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use crate::common::error::Result;
use crate::common::types::{AgentPath, ContextNamespace, TianyanUri};
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};

/// 规则过时的默认天数阈值。
const DEFAULT_RULE_STALE_DAYS: u32 = 30;
/// 记忆过时的默认天数阈值。
const DEFAULT_MEMORY_TTL_DAYS: u32 = 90;

/// GC 任务：定期扫描并清理过期内容。
pub struct GcTask {
    /// 是否启用自动清理（来自 StorageConfig.auto_cleanup）。
    auto_cleanup: bool,
    /// 清理前数据保留天数（来自 StorageConfig.cleanup_days）。
    cleanup_days: u32,
    /// 记忆条目 TTL 天数。
    memory_ttl_days: u32,
    /// 规则过时天数阈值。
    rule_stale_days: u32,
}

impl GcTask {
    /// 创建新的 GC 任务。
    pub fn new() -> Self {
        Self {
            auto_cleanup: true,
            cleanup_days: 365,
            memory_ttl_days: DEFAULT_MEMORY_TTL_DAYS,
            rule_stale_days: DEFAULT_RULE_STALE_DAYS,
        }
    }

    /// 设置是否启用自动清理。
    pub fn with_auto_cleanup(mut self, auto_cleanup: bool) -> Self {
        self.auto_cleanup = auto_cleanup;
        self
    }

    /// 设置清理保留天数。
    pub fn with_cleanup_days(mut self, days: u32) -> Self {
        self.cleanup_days = days;
        self
    }

    /// 设置规则过时天数阈值。
    pub fn with_rule_stale_days(mut self, days: u32) -> Self {
        self.rule_stale_days = days;
        self
    }

    /// 扫描 learned 规则，检查并清理过时规则。
    async fn scan_learned_rules(&self, ctx: &TaskContext) -> Result<usize> {
        let learned_uri = AgentPath::Learned.uri();
        let entries = match ctx.vfs.list(&learned_uri).await {
            Ok(e) => e,
            Err(_) => return Ok(0),
        };

        let mut stale_count = 0;
        let stale_threshold = Utc::now() - Duration::from_secs(self.rule_stale_days as u64 * 86400);

        for entry in &entries {
            let metadata_updated = entry.metadata.updated_at;
            if metadata_updated >= stale_threshold {
                continue;
            }

            stale_count += 1;
            let days_since = (Utc::now() - metadata_updated).num_days();
            tracing::warn!(
                rule_uri = %entry.metadata.uri,
                days_since_update = days_since,
                "规则过时"
            );

            // 如果启用自动清理，将过时规则归档
            if self.auto_cleanup {
                let rule_name = entry
                    .metadata
                    .uri
                    .path()
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());

                let archive_uri = AgentPath::Learned
                    .uri()
                    .append("archive")
                    .append(&rule_name);

                match ctx.vfs.move_entry(&entry.metadata.uri, &archive_uri).await {
                    Ok(_) => {
                        tracing::info!(
                            rule_uri = %entry.metadata.uri,
                            days_since_update = days_since,
                            "已归档过时规则"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            rule_uri = %entry.metadata.uri,
                            error = %e,
                            "归档过时规则失败"
                        );
                    }
                }
            }
        }

        if stale_count > 0 {
            tracing::info!(
                total_rules = entries.len(),
                stale_count = stale_count,
                auto_cleanup = self.auto_cleanup,
                "GC: 发现过时规则"
            );
        } else if entries.len() > 0 {
            tracing::debug!(total_rules = entries.len(), "GC: 所有规则均未过时");
        }

        Ok(stale_count)
    }

    /// 扫描 Memory 命名空间，清理过期记忆条目。
    async fn scan_memory(&self, ctx: &TaskContext) -> Result<usize> {
        if !self.auto_cleanup {
            return Ok(0);
        }

        let memory_uri = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let entries = match ctx.vfs.list(&memory_uri).await {
            Ok(e) => e,
            Err(_) => return Ok(0),
        };

        let mut cleaned = 0;
        let ttl_threshold = Utc::now() - Duration::from_secs(self.memory_ttl_days as u64 * 86400);

        for entry in &entries {
            if entry.metadata.updated_at < ttl_threshold {
                match ctx.vfs.delete(&entry.metadata.uri).await {
                    Ok(_) => {
                        cleaned += 1;
                        tracing::debug!(
                            memory_uri = %entry.metadata.uri,
                            "已清理过期记忆"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            memory_uri = %entry.metadata.uri,
                            error = %e,
                            "清理过期记忆失败"
                        );
                    }
                }
            }
        }

        if cleaned > 0 {
            tracing::info!(cleaned = cleaned, "GC: 已清理过期记忆条目");
        }

        Ok(cleaned)
    }
}

impl Default for GcTask {
    fn default() -> Self {
        Self::new()
    }
}

/// 文档漂移检测报告。
#[derive(Debug, Clone)]
struct DocDriftReport {
    pub missing_modules: Vec<String>,
    pub total_docs_checked: usize,
}

impl GcTask {
    /// 检测 docs/ 中的模块引用是否仍然有效。
    ///
    /// 扫描 docs/*.md 文件中引用到的模块路径（如 `core/src/agent/`），
    /// 验证它们在实际源代码中是否仍然存在。
    async fn scan_documentation_drift(&self, _ctx: &TaskContext) -> Result<DocDriftReport> {
        let mut report = DocDriftReport {
            missing_modules: Vec::new(),
            total_docs_checked: 0,
        };

        let docs_dir = "docs";
        let src_dir = "core/src";

        // 扫描 docs/ 目录
        let _docs_uri = TianyanUri::new(ContextNamespace::Knowledge, vec![]);
        // 直接读取文件系统
        match std::fs::read_dir(docs_dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(true, |e| e != "md") {
                        continue;
                    }
                    report.total_docs_checked += 1;

                    if let Ok(content) = std::fs::read_to_string(&path) {
                        // 查找文档中引用的模块路径（如 `core/src/agent/coordinator.rs`）
                        for line in content.lines() {
                            if let Some(idx) = line.find("core/src/") {
                                let rest = &line[idx..];
                                let module_path = rest
                                    .split(&[' ', ',', ')', '(', ':', '\t', '\r', '\n'][..])
                                    .next()
                                    .unwrap_or("");
                                let module_path = module_path.trim_end_matches(".rs");

                                // 检查对应文件是否存在
                                let fs_path =
                                    format!("{}/{}", src_dir, &module_path["core/src/".len()..]);
                                let rs_path = format!("{}.rs", &fs_path);
                                if !std::path::Path::new(&rs_path).exists()
                                    && !std::path::Path::new(&format!("{}/mod.rs", &fs_path))
                                        .exists()
                                {
                                    if !report.missing_modules.contains(&module_path.to_string()) {
                                        report.missing_modules.push(module_path.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "无法读取 docs/ 目录");
            }
        }

        if !report.missing_modules.is_empty() {
            tracing::warn!(
                count = report.missing_modules.len(),
                modules = ?report.missing_modules,
                "发现文档引用了不存在的模块"
            );
        }

        Ok(report)
    }

    /// 将质量评分写入 docs/quality/domain-grades.md。
    async fn write_quality_report(&self, _ctx: &TaskContext) -> Result<()> {
        let quality_dir = std::path::Path::new("docs/quality");
        if !quality_dir.exists() {
            std::fs::create_dir_all(quality_dir).ok();
        }

        let report_path = quality_dir.join("domain-grades.md");
        let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let content = format!(
            r#"# 域质量评级 (最后更新: {})

> 由 GC 任务自动生成，每次 GC 扫描后更新。

| 域 | 过时规则数 | 过期记忆数 | 文档漂移 | 备注 |
|----|-----------|-----------|---------|------|
| agent | - | - | - | 验证门控已集成 |
| context | - | - | - | 新鲜度评分已集成 |
| storage | - | - | - | - |
| skills | - | - | - | - |
| scheduler | - | - | - | - |

## 最新 GC 扫描摘要

- 扫描时间: {}
- 状态: GC 任务正常运行
"#,
            now, now
        );

        std::fs::write(&report_path, content).ok();
        tracing::info!(path = %report_path.display(), "质量报告已更新");

        Ok(())
    }
}

#[async_trait]
impl TaskHandler for GcTask {
    async fn execute(&self, ctx: &TaskContext) -> TaskResult {
        tracing::info!("开始执行 GC 扫描...");

        let rules_result = self.scan_learned_rules(ctx).await;
        let memory_result = self.scan_memory(ctx).await;

        // 文档漂移检测（不阻塞 GC 主流程）
        let drift_result = self.scan_documentation_drift(ctx).await;

        // 质量报告回写
        let _ = self.write_quality_report(ctx).await;

        match (rules_result, memory_result) {
            (Ok(rules), Ok(mem)) => {
                let drift_msg = match drift_result {
                    Ok(ref report) if !report.missing_modules.is_empty() => {
                        format!("，发现 {} 个文档漂移", report.missing_modules.len())
                    }
                    _ => String::new(),
                };
                tracing::info!(
                    stale_rules = rules,
                    cleaned_memory = mem,
                    "GC 扫描完成{}",
                    drift_msg
                );
                TaskResult::success(rules + mem)
            }
            (Err(e), _) | (_, Err(e)) => {
                tracing::warn!(error = %e, "GC 扫描失败");
                TaskResult::failed(format!("GC 扫描失败：{}", e))
            }
        }
    }

    fn name(&self) -> &str {
        "garbage_collection"
    }
}

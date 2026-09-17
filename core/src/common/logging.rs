//! Logging utilities for the Tianyan agent system.

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter,
};

use crate::common::error::{Result, TianyanError};

/// 日志配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// 日志级别（trace、debug、info、warn、error）。
    #[serde(default = "default_log_level")]
    pub level: String,
    /// 日志格式（text、json）。
    #[serde(default = "default_log_format")]
    pub format: String,
    /// 日志文件路径（可选，未设置则输出到控制台）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// 最大日志文件大小（MB）。
    #[serde(default = "default_max_log_size")]
    pub max_file_size: u64,
    /// 保留的日志文件数量。
    #[serde(default = "default_log_files")]
    pub max_files: u32,
    /// 包含时间戳。
    #[serde(default = "default_true")]
    pub include_timestamp: bool,
    /// 包含文件和行号信息。
    #[serde(default)]
    pub include_location: bool,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "text".to_string()
}

fn default_max_log_size() -> u64 {
    10
}

fn default_log_files() -> u32 {
    5
}

fn default_true() -> bool {
    true
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
            file: None,
            max_file_size: default_max_log_size(),
            max_files: default_log_files(),
            include_timestamp: true,
            include_location: false,
        }
    }
}

impl LoggingConfig {
    /// 验证日志配置。
    ///
    /// 返回 `std::result::Result` 而非 [`crate::common::error::Result`]，
    /// 与配置校验链（`TianyanConfig::validate` 的 `Result<(), String>`）保持一致。
    pub fn validate(&self) -> std::result::Result<(), String> {
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.level.as_str()) {
            return Err(format!(
                "无效的日志级。
  {}，有效值为: {:?}",
                self.level, valid_levels
            ));
        }

        let valid_formats = ["text", "json"];
        if !valid_formats.contains(&self.format.as_str()) {
            return Err(format!(
                "无效的日志格。
  {}，有效值为: {:?}",
                self.format, valid_formats
            ));
        }

        if self.max_file_size == 0 {
            return Err("max_file_size 必须大于 0".to_string());
        }

        if self.max_files == 0 {
            return Err("max_files 必须大于 0".to_string());
        }

        Ok(())
    }
}

/// Initialize the logging system.
///
/// 控制台 layer 恒装配；配置了 `[logging].file` 时额外装配**按大小轮转的文件 layer**
/// （`max_file_size` MB / `max_files` 保留数——两者此前为死配置，见 T2 项）。
pub fn init_logging(config: &LoggingConfig) -> Result<()> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    let subscriber = tracing_subscriber::registry().with(env_filter);

    // 文件 layer（可选）：路径取 `[logging].file`；目录 = 其父目录，文件族前缀 =
    // 文件名主体（`tianyan.log` / `tianyan.1.log` 同族，按 max_files 保留最新 N 个）。
    let file_layer = match config.file.as_ref() {
        Some(path) => {
            let dir = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "tianyan".to_string());
            let writer =
                RotatingWriter::new(dir, stem.clone(), stem, LogRotation::from_config(config))
                    .map_err(|e| TianyanError::config(format!("Failed to open log file: {e}")))?;
            Some(
                fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false)
                    .with_target(config.include_location)
                    .with_file(config.include_location)
                    .with_line_number(config.include_location)
                    .boxed(),
            )
        }
        None => None,
    };

    match config.format.as_str() {
        "json" => {
            let json_layer = fmt::layer()
                .json()
                .with_span_events(FmtSpan::CLOSE)
                .with_target(config.include_location)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .boxed();

            let result = match file_layer {
                Some(file_layer) => tracing::subscriber::set_global_default(
                    subscriber.with(json_layer).with(file_layer),
                ),
                None => tracing::subscriber::set_global_default(subscriber.with(json_layer)),
            };
            result.map_err(|e| {
                TianyanError::config(format!("Failed to set logging subscriber: {e}"))
            })?;
        }
        _ => {
            let text_layer = fmt::layer()
                .with_target(config.include_location)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_thread_ids(false)
                .with_thread_names(false)
                .boxed();

            let result = match file_layer {
                Some(file_layer) => tracing::subscriber::set_global_default(
                    subscriber.with(text_layer).with(file_layer),
                ),
                None => tracing::subscriber::set_global_default(subscriber.with(text_layer)),
            };
            result.map_err(|e| {
                TianyanError::config(format!("Failed to set logging subscriber: {e}"))
            })?;
        }
    }

    Ok(())
}

/// 文件日志轮转参数（对齐 [`LoggingConfig`] 的 `max_file_size` / `max_files`）。
#[derive(Debug, Clone, Copy)]
pub struct LogRotation {
    /// 单文件大小上限（字节）。
    pub max_bytes: u64,
    /// 文件族保留数量上限（含当前打开的文件）。
    pub max_files: usize,
}

impl LogRotation {
    /// 从日志配置换算（MB → 字节；0 值按 1 兜底，`validate` 已拒绝 0）。
    pub fn from_config(config: &LoggingConfig) -> Self {
        Self {
            max_bytes: config.max_file_size.max(1) * 1024 * 1024,
            max_files: config.max_files.max(1) as usize,
        }
    }
}

/// 按大小轮转的日志 writer（多线程安全：内部 `Arc<Mutex<..>>`）。
///
/// - 当前文件 `{stem}.log`；第 n 次轮转的文件 `{stem}.{n}.log`；
/// - 每次打开/轮转时按 `family_prefix` 清理文件族，仅保留最新 `max_files` 个
///   （含当前文件；mtime 最旧优先；当前打开文件永不删除）；
/// - 单条日志不被切断：写入前判断（已写内容 + 本条会超上限则先轮转），
///   故单个文件最大 = `max_bytes`。
#[derive(Clone)]
pub struct RotatingWriter {
    inner: Arc<Mutex<RotatingInner>>,
}

/// 轮转 writer 的可变状态（受 `RotatingWriter::inner` 互斥保护）。
struct RotatingInner {
    dir: PathBuf,
    stem: String,
    family_prefix: String,
    seq: u32,
    file: File,
    written: u64,
    rotation: LogRotation,
}

impl RotatingWriter {
    /// 打开（或创建）`{dir}/{stem}.log` 并按参数轮转。
    ///
    /// `family_prefix` 用于识别「同族」历史日志文件（清理对象）：例如
    /// `"tianyan_"` 会匹配 `tianyan_20260916.log` 与 `tianyan_20260916.1.log`。
    pub fn new(
        dir: PathBuf,
        stem: String,
        family_prefix: String,
        rotation: LogRotation,
    ) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{stem}.log"));
        let file = open_append(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        let writer = Self {
            inner: Arc::new(Mutex::new(RotatingInner {
                dir,
                stem,
                family_prefix,
                seq: 0,
                file,
                written,
                rotation,
            })),
        };
        writer.prune();
        Ok(writer)
    }

    /// 立即轮转（显式触发；正常由写入上限触发）。
    pub fn rotate(&self) -> io::Result<()> {
        self.lock()?.rotate()
    }

    /// 清理文件族中超出 `max_files` 的最旧文件（best effort）。
    pub fn prune(&self) {
        if let Ok(inner) = self.lock() {
            let _ = prune_family(
                &inner.dir,
                &inner.family_prefix,
                inner.rotation.max_files,
                Some(&inner.current_path()),
            );
        }
    }

    /// 取内部状态锁（中毒时降级为 io 错误，不 panic）。
    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, RotatingInner>> {
        self.inner
            .lock()
            .map_err(|_| io::Error::other("logging: 日志写入锁中毒"))
    }
}

impl RotatingInner {
    /// 当前打开的文件路径。
    fn current_path(&self) -> PathBuf {
        if self.seq == 0 {
            self.dir.join(format!("{}.log", self.stem))
        } else {
            self.dir.join(format!("{}.{}.log", self.stem, self.seq))
        }
    }

    /// 关旧开新（`seq` 递增）；同名文件已存在时继续追加（不覆盖）。
    fn rotate(&mut self) -> io::Result<()> {
        let _ = self.file.flush();
        self.seq = self.seq.saturating_add(1);
        let path = self.current_path();
        self.file = open_append(&path)?;
        self.written = self.file.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = prune_family(
            &self.dir,
            &self.family_prefix,
            self.rotation.max_files,
            Some(&path),
        );
        Ok(())
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.lock()?;
        // 写前判断：已写内容 + 本条会超上限 → 先轮转（单条日志不跨文件）
        if inner.written > 0 && inner.written + buf.len() as u64 > inner.rotation.max_bytes {
            inner.rotate()?;
        }
        let n = inner.file.write(buf)?;
        inner.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.lock()?.file.flush()
    }
}

impl<'a> fmt::writer::MakeWriter<'a> for RotatingWriter {
    type Writer = RotatingWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// 以追加模式打开（不存在则创建）。
fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// 清理 `dir` 下 `prefix*` 且 `*.log` 的文件族，使**含当前打开文件在内**的总数
/// 不超过 `keep`（最旧优先删除）。`exclude` 指定的当前打开文件计入总数但永不
/// 删除——Windows 上删除已打开文件会延迟生效，不排除则当前日志可能被标记删除。
fn prune_family(dir: &Path, prefix: &str, keep: usize, exclude: Option<&Path>) -> io::Result<()> {
    if keep == 0 {
        return Ok(());
    }
    let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();
    let mut exclude_present = false;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(prefix) || !name.ends_with(".log") {
            continue;
        }
        let meta = entry.metadata()?;
        if !meta.is_file() {
            continue;
        }
        let path = entry.path();
        if exclude.is_some_and(|e| e == path) {
            exclude_present = true;
            continue;
        }
        files.push((path, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
    }
    let total = files.len() + if exclude_present { 1 } else { 0 };
    if total <= keep {
        return Ok(());
    }
    files.sort_by_key(|f| f.1);
    let remove_count = total - keep;
    for (path, _) in files.iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

/// Log level type alias for convenience.
pub type Level = tracing::Level;

/// Create a span for tracing.
#[macro_export]
macro_rules! span {
    ($level:expr, $name:expr) => {
        tracing::span!($level, $name)
    };
    ($level:expr, $name:expr, $($field:expr),+) => {
        tracing::span!($level, $name, $($field),+)
    };
}

/// Log an info message.
#[macro_export]
macro_rules! log_info {
    ($($arg:expr),+) => {
        tracing::info!($($arg),+)
    };
}

/// Log a debug message.
#[macro_export]
macro_rules! log_debug {
    ($($arg:expr),+) => {
        tracing::debug!($($arg),+)
    };
}

/// Log a warning message.
#[macro_export]
macro_rules! log_warn {
    ($($arg:expr),+) => {
        tracing::warn!($($arg),+)
    };
}

/// Log an error message.
#[macro_export]
macro_rules! log_error {
    ($($arg:expr),+) => {
        tracing::error!($($arg),+)
    };
}

/// Log a trace message.
#[macro_export]
macro_rules! log_trace {
    ($($arg:expr),+) => {
        tracing::trace!($($arg),+)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_logging() {
        let config = LoggingConfig::default();
        // Note: Can only initialize once per process
        // This test just verifies the config is valid
        assert!(!config.level.is_empty());
    }

    #[test]
    fn test_default_logging_config() {
        let config = LoggingConfig::default();
        assert_eq!(config.level, "info");
        assert_eq!(config.format, "text");
        assert!(config.include_timestamp);
    }

    #[test]
    fn test_logging_validation() {
        let mut config = LoggingConfig::default();
        assert!(config.validate().is_ok());

        config.level = "invalid".to_string();
        assert!(config.validate().is_err());

        config.level = "info".to_string();
        config.format = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    /// T2 接线：按大小轮转——超上限开新文件，且单条日志不跨文件、不丢内容。
    #[test]
    fn test_rotating_writer_rotates_by_size() {
        let dir = tempfile::tempdir().unwrap();
        let writer = RotatingWriter::new(
            dir.path().to_path_buf(),
            "tianyan_20260101_000000".to_string(),
            "tianyan_".to_string(),
            LogRotation {
                max_bytes: 64,
                max_files: 10,
            },
        )
        .unwrap();
        let mut w = writer.clone();
        for i in 0..20 {
            w.write_all(format!("line-{i:03}\n").as_bytes()).unwrap();
        }
        w.flush().unwrap();

        let mut files: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert!(files.len() > 1, "写满上限应轮转出多个文件：{files:?}");

        let mut all = String::new();
        for f in &files {
            let path = dir.path().join(f);
            let len = fs::metadata(&path).unwrap().len();
            assert!(len <= 64, "{f} 超过单文件上限：{len} 字节");
            all.push_str(&fs::read_to_string(&path).unwrap());
        }
        for i in 0..20 {
            assert!(
                all.contains(&format!("line-{i:03}")),
                "第 {i} 条日志不应丢失"
            );
        }
    }

    /// T2 接线：`max_files` 生效——含当前文件在内只保留最新 N 个（清理历史文件）。
    #[test]
    fn test_rotating_writer_prunes_to_max_files() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            fs::write(
                dir.path().join(format!("tianyan_2026010{i}_000000.log")),
                b"stale",
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let writer = RotatingWriter::new(
            dir.path().to_path_buf(),
            "tianyan_20260916_120000".to_string(),
            "tianyan_".to_string(),
            LogRotation {
                max_bytes: 1024 * 1024,
                max_files: 3,
            },
        )
        .unwrap();
        let mut w = writer.clone();
        w.write_all(b"fresh\n").unwrap();
        w.flush().unwrap();

        let files: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("tianyan_") && n.ends_with(".log"))
            .collect();
        assert_eq!(files.len(), 3, "含当前文件应只保留 3 个：{files:?}");
        assert!(
            dir.path().join("tianyan_20260916_120000.log").exists(),
            "当前打开的文件不得被清理"
        );
    }

    /// 清理范围限于文件族：非 `.log` / 非同前缀文件不受影响。
    #[test]
    fn test_prune_family_scope() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("other.log"), b"keep").unwrap();
        fs::write(dir.path().join("tianyan_keep.txt"), b"keep").unwrap();
        for i in 0..4 {
            fs::write(dir.path().join(format!("tianyan_old{i}.log")), b"stale").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let writer = RotatingWriter::new(
            dir.path().to_path_buf(),
            "tianyan_cur".to_string(),
            "tianyan_".to_string(),
            LogRotation {
                max_bytes: 1024,
                max_files: 2,
            },
        )
        .unwrap();
        let mut w = writer.clone();
        w.write_all(b"x\n").unwrap();
        w.flush().unwrap();

        assert!(dir.path().join("other.log").exists(), "非族文件不得清理");
        assert!(
            dir.path().join("tianyan_keep.txt").exists(),
            "非 .log 文件不得清理"
        );
        let family: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("tianyan_") && n.ends_with(".log"))
            .collect();
        assert_eq!(family.len(), 2, "族内应保留 2 个：{family:?}");
    }
}

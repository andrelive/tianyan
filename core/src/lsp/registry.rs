//! 语言服务器注册表：`ServerSpec` 元数据 + 内置服务器表 + 项目根探测。

use std::path::{Path, PathBuf};

/// 单个语言服务器的规格说明。
///
/// 仅描述"如何找到并启动"服务器，不含任何运行时状态。
#[derive(Debug, Clone, Copy)]
pub struct ServerSpec {
    /// LSP `languageId`（didOpen 通知使用）。
    pub language_id: &'static str,
    /// 关联文件扩展名（不含点，小写）。
    pub extensions: &'static [&'static str],
    /// 项目根标记文件（存在任一即视为项目根）。
    pub root_markers: &'static [&'static str],
    /// 启动命令（要求已在 PATH 中）。
    pub spawn_command: &'static str,
    /// 启动参数。
    pub args: &'static [&'static str],
    /// 人类可读的安装提示（错误消息中展示）。
    pub auto_install_hint: &'static str,
}

/// 内置语言服务器表（按扩展名查找）。
pub const BUILTIN_SERVERS: &[ServerSpec] = &[
    ServerSpec {
        language_id: "rust",
        extensions: &["rs"],
        root_markers: &["Cargo.toml"],
        spawn_command: "rust-analyzer",
        args: &[],
        auto_install_hint: "rustup component add rust-analyzer",
    },
    ServerSpec {
        language_id: "typescript",
        extensions: &["ts", "tsx"],
        root_markers: &["package.json", "tsconfig.json"],
        spawn_command: "typescript-language-server",
        args: &["--stdio"],
        auto_install_hint: "npm install -g typescript-language-server typescript",
    },
    ServerSpec {
        language_id: "javascript",
        extensions: &["js", "jsx", "mjs", "cjs"],
        root_markers: &["package.json", "tsconfig.json"],
        spawn_command: "typescript-language-server",
        args: &["--stdio"],
        auto_install_hint: "npm install -g typescript-language-server typescript",
    },
    ServerSpec {
        language_id: "python",
        extensions: &["py", "pyi"],
        root_markers: &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"],
        spawn_command: "pyright-langserver",
        args: &["--stdio"],
        auto_install_hint: "npm install -g pyright",
    },
    ServerSpec {
        language_id: "go",
        extensions: &["go"],
        root_markers: &["go.mod"],
        spawn_command: "gopls",
        args: &[],
        auto_install_hint: "go install golang.org/x/tools/gopls@latest",
    },
];

/// 按文件扩展名查找服务器规格（大小写不敏感）。
pub fn spec_for_extension(ext: &str) -> Option<&'static ServerSpec> {
    BUILTIN_SERVERS
        .iter()
        .find(|spec| spec.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// 按语言 ID 查找服务器规格。
pub fn spec_for_language(language_id: &str) -> Option<&'static ServerSpec> {
    BUILTIN_SERVERS
        .iter()
        .find(|spec| spec.language_id == language_id)
}

/// 从起点向上探测项目根（任一 `root_markers` 存在即命中）。
///
/// - `start` 为文件时从其父目录开始；为目录时从自身开始。
/// - 找不到任何标记返回 `None`。
pub fn probe_project_root(start: &Path, spec: &ServerSpec) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if spec
            .root_markers
            .iter()
            .any(|marker| dir.join(marker).is_file())
        {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;

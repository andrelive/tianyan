//! LSP 服务器注册表测试：规格查找 + 项目根探测。

use super::*;

#[test]
fn test_spec_for_extension_rs() {
    let spec = spec_for_extension("rs").expect("rs 应有 rust-analyzer 规格");
    assert_eq!(spec.spawn_command, "rust-analyzer");
    assert_eq!(spec.language_id, "rust");
}

#[test]
fn test_spec_for_extension_tsx() {
    let spec = spec_for_extension("tsx").expect("tsx 应有 typescript 规格");
    assert_eq!(spec.spawn_command, "typescript-language-server");
}

#[test]
fn test_spec_for_extension_case_insensitive() {
    assert!(spec_for_extension("RS").is_some());
    assert!(spec_for_extension("Py").is_some());
}

#[test]
fn test_spec_for_extension_unknown() {
    assert!(spec_for_extension("xyz").is_none());
    assert!(spec_for_extension("").is_none());
}

#[test]
fn test_spec_for_language() {
    let spec = spec_for_language("python").expect("python 应有 pyright 规格");
    assert_eq!(spec.spawn_command, "pyright-langserver");
    assert!(spec_for_language("unknown").is_none());
}

#[test]
fn test_probe_project_root_finds_marker_in_ancestor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src").join("main.rs");
    std::fs::create_dir_all(src.parent().expect("parent")).expect("create dirs");
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").expect("write marker");
    let spec = spec_for_extension("rs").expect("rs spec");
    assert_eq!(
        probe_project_root(&src, spec),
        Some(dir.path().to_path_buf())
    );
}

#[test]
fn test_probe_project_root_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("a").join("b.rs");
    std::fs::create_dir_all(file.parent().expect("parent")).expect("create dirs");
    let spec = spec_for_extension("rs").expect("rs spec");
    assert_eq!(probe_project_root(&file, spec), None);
}

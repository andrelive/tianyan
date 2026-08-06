//! 项目格式探测注册表测试（probe_project / verification_command）。

use std::path::Path;

use super::*;

#[test]
fn test_probe_cargo_project() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();
    let info = probe_project(dir.path()).unwrap();
    assert_eq!(info.format, ProjectFormat::Cargo);
    assert_eq!(info.marker, "Cargo.toml");
    assert_eq!(info.root, dir.path());
}

#[test]
fn test_probe_python_project() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"demo\"\n",
    )
    .unwrap();
    let info = probe_project(dir.path()).unwrap();
    assert_eq!(info.format, ProjectFormat::Python);
    assert_eq!(info.marker, "pyproject.toml");
}

#[test]
fn test_probe_typescript_project() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tsconfig.json"), "{}\n").unwrap();
    let info = probe_project(dir.path()).unwrap();
    assert_eq!(info.format, ProjectFormat::TypeScript);
    assert_eq!(info.marker, "tsconfig.json");
}

#[test]
fn test_probe_walks_up_to_parent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    let sub = dir.path().join("src").join("nested");
    std::fs::create_dir_all(&sub).unwrap();
    let info = probe_project(&sub).unwrap();
    assert_eq!(info.format, ProjectFormat::Cargo);
    assert_eq!(info.root, dir.path());
}

#[test]
fn test_probe_unknown_in_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let info = probe_project(dir.path()).unwrap();
    assert_eq!(info.format, ProjectFormat::Unknown);
    assert!(info.marker.is_empty());
}

#[test]
fn test_probe_cargo_wins_over_typescript() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(dir.path().join("tsconfig.json"), "{}\n").unwrap();
    let info = probe_project(dir.path()).unwrap();
    assert_eq!(info.format, ProjectFormat::Cargo);
    assert_eq!(info.marker, "Cargo.toml");
}

#[test]
fn test_verification_command_cargo() {
    let cmd = verification_command(&ProjectInfo {
        format: ProjectFormat::Cargo,
        root: Path::new("/tmp/demo").to_path_buf(),
        marker: "Cargo.toml".to_string(),
    })
    .unwrap();
    assert!(cmd.iter().any(|c| c == "cargo"));
    assert!(cmd
        .iter()
        .any(|c| c == "--message-format=json-render-diagnostics"));
}

#[test]
fn test_verification_command_typescript_and_python() {
    let ts = verification_command(&ProjectInfo {
        format: ProjectFormat::TypeScript,
        root: Path::new("/tmp/demo").to_path_buf(),
        marker: "tsconfig.json".to_string(),
    })
    .unwrap();
    assert!(ts.iter().any(|c| c == "tsc"));
    assert!(ts.iter().any(|c| c == "--noEmit"));

    let py = verification_command(&ProjectInfo {
        format: ProjectFormat::Python,
        root: Path::new("/tmp/demo").to_path_buf(),
        marker: "pyproject.toml".to_string(),
    })
    .unwrap();
    assert!(py.iter().any(|c| c == "pytest"));
}

#[test]
fn test_verification_command_unknown_is_none() {
    let cmd = verification_command(&ProjectInfo {
        format: ProjectFormat::Unknown,
        root: Path::new("/tmp/demo").to_path_buf(),
        marker: String::new(),
    });
    assert!(cmd.is_none());
}

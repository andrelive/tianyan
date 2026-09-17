//! `apply_patch` 测试：解析 + 纯函数应用 + 执行器（原子多文件）。
//!
//! 覆盖：中间替换、多块自底向上、空白漂移、行号偏移、CRLF 保持、
//! 删除块、上下文纯块、解析错误、路径逃逸、原子性（失败不落盘）、
//! 文件创建、文件缺失、rename 不支持。

use super::*;
use serde_json::json;

/// 便捷构造单个 `*** Update File` 补丁文本（仅一个文件）。
fn patch_for(body: &str) -> String {
    format!("*** Update File: f.txt\n{body}")
}

// ── parse_patch ──────────────────────────────────────────────────────────

#[test]
fn parse_patch_parses_multi_file() {
    let text = "\
*** Update File: src/a.rs
@@ -1,3 +1,3 @@
 ctx
-a
+A
 ctx
*** Update File: src/b.rs
@@ -1,1 +1,1 @@
-b
+B
";
    let files = parse_patch(text).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "src/a.rs");
    assert_eq!(files[0].hunks.len(), 1);
    let h = &files[0].hunks[0];
    // 文本 1 起始 → 内部 0 起始
    assert_eq!(h.old_start, 0);
    assert_eq!(h.old_count, 3);
    assert_eq!(h.new_count, 3);
    // 块体：ctx + 删除 + 新增 + ctx = 4 行
    assert_eq!(h.lines.len(), 4);
    assert_eq!(files[1].path, "src/b.rs");
    assert_eq!(files[1].hunks[0].old_count, 1);
}

#[test]
fn parse_patch_empty_text_errors() {
    let err = parse_patch("").unwrap_err();
    assert!(err.to_string().contains("解析失败"), "{err}");
}

#[test]
fn parse_patch_bare_hunk_header_accepted() {
    // 模型常漏写行号范围（裸 @@）：宽容解析为"位置未知"，不报错
    let files = parse_patch(&patch_for("@@\n x\n")).unwrap();
    let h = &files[0].hunks[0];
    assert_eq!(h.old_start, usize::MAX, "裸 @@ 应标记为位置未知");
}

#[test]
fn apply_patch_bare_hunk_header_locates_by_content() {
    // 裸 @@（无行号）：按内容定位，仍能正确应用（上下文 + 插入，对齐真实场景）
    let content = "a\nb\nc\n";
    let text = patch_for("@@\n b\n+B\n");
    let files = parse_patch(&text).unwrap();
    let result = apply_patch_to_content(content, &files[0].hunks).unwrap();
    assert_eq!(result, "a\nb\nB\nc\n");
}

#[test]
fn apply_patch_without_hunk_header_works() {
    // 对齐 omo：模型可不写 @@，直接 *** Update File + -/+ 行
    let content = "a\nb\nc\n";
    let text = "*** Update File: f.txt\n-b\n+B\n";
    let files = parse_patch(text).unwrap();
    let result = apply_patch_to_content(content, &files[0].hunks).unwrap();
    assert_eq!(result, "a\nB\nc\n");
}

#[test]
fn apply_patch_duplicate_file_header_errors() {
    // 模型重复写 *** Update File 头：明确报"重复头"错误（便于模型纠正）
    let text = "*** Update File: f.txt\n*** Update File: f.txt\n-b\n+B\n";
    let err = parse_patch(text).unwrap_err();
    assert!(
        err.to_string().contains("重复的 *** Update File 头"),
        "{err}"
    );
}

#[test]
fn apply_patch_empty_file_section_errors() {
    // 模型写了头但没写内容（异路径空段）：明确报"文件缺少补丁内容"，
    // 避免静默丢弃导致模型以为改过但实际没改
    let text = "*** Update File: a.txt\n*** Update File: b.txt\n-b\n+B\n";
    let err = parse_patch(text).unwrap_err();
    assert!(err.to_string().contains("文件缺少补丁内容"), "{err}");
}

#[test]
fn parse_patch_hunk_without_leading_header_errors() {
    let err = parse_patch("@@ -1,1 +1,1 @@\n x\n").unwrap_err();
    assert!(err.to_string().contains("解析失败"), "{err}");
}

#[test]
fn parse_patch_rename_operation_unsupported() {
    let err = parse_patch("*** Rename File: a.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n").unwrap_err();
    assert!(err.to_string().contains("不支持的补丁操作"), "{err}");
}

#[test]
fn parse_patch_ignores_claude_style_envelope_lines() {
    // Claude Code 风格围栏（模型输出惯性夹带 Begin/End）：本工具无围栏，
    // 已知围栏行跳过（与空行同策略），不报"未知的信封头部"。
    let text = "\
*** Begin Patch
*** Update File: a.txt
@@ -1,1 +1,1 @@
-a
+A
*** End Patch
";
    let files = parse_patch(text).unwrap();
    assert_eq!(files.len(), 1, "围栏行应被跳过，仅剩文件块");
    assert_eq!(files[0].path, "a.txt");
    assert_eq!(files[0].hunks.len(), 1);
    let out = apply_patch_to_content("a\n", &files[0].hunks).unwrap();
    assert_eq!(out, "A\n");
}

#[test]
fn parse_patch_ignores_envelope_variants_and_unknown_still_errors() {
    // Begin / End / Begin Patch / End Patch 四种变体（大小写不敏感）均可跳过
    let text = "\
*** begin patch
*** Update File: a.txt
@@ -1,1 +1,1 @@
-a
+A
*** end
*** Update File: b.txt
@@ -1,1 +1,1 @@
-b
+B
*** END PATCH
";
    let files = parse_patch(text).unwrap();
    assert_eq!(files.len(), 2, "文件间围栏也应跳过");
    // 未知 *** 头仍报错（围栏容错不放行任意头部）
    let err = parse_patch("*** Something Else\n").unwrap_err();
    assert!(err.to_string().contains("未知的信封头部"), "{err}");
}

#[test]
fn parse_patch_envelope_only_text_errors() {
    // 只有围栏没有实际文件块：仍按"补丁为空"报错（围栏不构成补丁内容）
    let err = parse_patch("*** Begin Patch\n*** End Patch\n").unwrap_err();
    assert!(err.to_string().contains("解析失败"), "{err}");
}

#[test]
fn parse_patch_hunk_body_without_prefix_errors() {
    let err = parse_patch(&patch_for("@@ -1,1 +1,1 @@\n裸行\n")).unwrap_err();
    assert!(err.to_string().contains("解析失败"), "{err}");
}

#[test]
fn first_patch_path_extracts_first_header() {
    let text = "\
*** Update File: src/a.rs
@@ -1,1 +1,1 @@
-a
+A
*** Update File: src/b.rs
@@ -1,1 +1,1 @@
-b
+B
";
    assert_eq!(first_patch_path(text), "src/a.rs");
    assert_eq!(first_patch_path("no headers here"), "");
}

// ── apply_patch_to_content 纯函数 ───────────────────────────────────────

#[test]
fn single_hunk_replace_in_middle_exact() {
    let content = "line1\nline2\nline3\nline4\n";
    let text = patch_for("@@ -2,3 +2,3 @@\n line2\n-line3\n+LINE3\n line4\n");
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content(content, &files[0].hunks).unwrap();
    assert_eq!(out, "line1\nline2\nLINE3\nline4\n");
}

#[test]
fn multi_hunk_same_file_applied_bottom_up() {
    let content = "a\nb\nc\nd\ne\n";
    let text = patch_for("@@ -1,2 +1,2 @@\n a\n-b\n+B\n@@ -4,2 +4,2 @@\n d\n-e\n+E\n");
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content(content, &files[0].hunks).unwrap();
    assert_eq!(out, "a\nB\nc\nd\nE\n");
}

#[test]
fn whitespace_drifted_context_matches_via_normalization() {
    // 文件中的上下文行带多余尾随空格（与哈希锚点同类的容错需求）
    let content = "fn main() {\n    let x = 1;   \n    println!(\"hi\");\n}\n";
    let text = patch_for(
        "@@ -1,3 +1,3 @@\n fn main() {\n-    let x = 1;\n+    let x = 2;\n     println!(\"hi\");\n",
    );
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content(content, &files[0].hunks).unwrap();
    assert_eq!(
        out,
        "fn main() {\n    let x = 2;\n    println!(\"hi\");\n}\n"
    );
}

#[test]
fn hunk_offset_off_by_n_recovered_by_full_scan() {
    // 块头声称起始行 5，实际内容在第 3 行（0 起始 2）→ 全文件搜索恢复
    let content = "a\nb\nc\nd\ne\nf\n";
    let text = patch_for("@@ -5,2 +5,2 @@\n c\n-d\n+D\n");
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content(content, &files[0].hunks).unwrap();
    // 窗口 [c, d]（上下文 c + 删除 d）被替换为 [c, D]
    assert_eq!(out, "a\nb\nc\nD\ne\nf\n");
}

#[test]
fn fuzzy_ratio_fallback_recovers_near_match() {
    // 1 行上下文漂移（lineX vs line6）：精确匹配失败 → 模糊匹配（ratio 0.8）
    let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\n";
    let text = patch_for("@@ -2,5 +2,5 @@\n line2\n-line3\n+CHANGED\n line4\n line5\n lineX\n");
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content(content, &files[0].hunks).unwrap();
    // 上下文行保留文件原内容（不覆盖）
    assert_eq!(out, "line1\nline2\nCHANGED\nline4\nline5\nline6\nline7\n");
}

#[test]
fn crlf_target_preserves_eol() {
    let content = "a\r\nb\r\nc\r\n";
    let text = patch_for("@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n");
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content(content, &files[0].hunks).unwrap();
    assert_eq!(out, "a\r\nB\r\nc\r\n");
}

#[test]
fn deletion_hunk_removes_lines() {
    let content = "a\nb\nc\n";
    let text = patch_for("@@ -2,2 +2,0 @@\n-b\n-c\n");
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content(content, &files[0].hunks).unwrap();
    assert_eq!(out, "a\n");
}

#[test]
fn no_match_errors_with_locate_message() {
    let content = "x\ny\nz\n";
    let text = patch_for("@@ -1,2 +1,2 @@\n a\n-b\n+c\n");
    let files = parse_patch(&text).unwrap();
    let err = apply_patch_to_content(content, &files[0].hunks).unwrap_err();
    assert!(err.to_string().contains("无法定位补丁块"), "{err}");
}

#[test]
fn context_only_hunk_applies_without_change() {
    let content = "a\nb\nc\n";
    let text = patch_for("@@ -2,1 +2,1 @@\n b\n");
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content(content, &files[0].hunks).unwrap();
    assert_eq!(out, "a\nb\nc\n");
}

#[test]
fn creation_hunk_on_empty_content() {
    let text = patch_for("@@ -0,0 +1,2 @@\n+l1\n+l2\n");
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content("", &files[0].hunks).unwrap();
    assert_eq!(out, "l1\nl2");
}

// ── apply_patch_action 执行器（原子多文件） ─────────────────────────────

#[tokio::test]
async fn executor_applies_multi_file_patch() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), "old\n")
        .await
        .unwrap();
    let patch = "\
*** Update File: a.txt
@@ -1,1 +1,1 @@
-old
+NEW
*** Update File: new.txt
@@ -0,0 +1,1 @@
+created
";
    let result = apply_patch_action(patch, dir.path()).await.unwrap();
    assert_eq!(result["total_files"].as_u64(), Some(2));
    let files = result["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["path"].as_str().unwrap(), "a.txt");
    assert_eq!(files[0]["hunks_applied"].as_u64(), Some(1));
    // 1 删除 + 1 新增 = 2 行变更
    assert_eq!(files[0]["lines_changed"].as_u64(), Some(2));
    assert_eq!(
        tokio::fs::read_to_string(dir.path().join("a.txt"))
            .await
            .unwrap(),
        "NEW\n"
    );
    let created = tokio::fs::read_to_string(dir.path().join("new.txt"))
        .await
        .unwrap();
    assert_eq!(created, "created");
}

#[tokio::test]
async fn executor_rejects_parent_dir_escape() {
    let dir = tempfile::tempdir().unwrap();
    // 正斜杠逃逸
    let patch = "*** Update File: ../evil.txt\n@@ -0,0 +1,1 @@\n+x\n";
    let err = apply_patch_action(patch, dir.path()).await.unwrap_err();
    assert!(err.to_string().contains("非法路径"), "{err}");
    // Windows 反斜杠逃逸（跨平台统一检测）
    let patch2 = "*** Update File: ..\\..\\evil.txt\n@@ -0,0 +1,1 @@\n+x\n";
    let err2 = apply_patch_action(patch2, dir.path()).await.unwrap_err();
    assert!(err2.to_string().contains("非法路径"), "{err2}");
    // 逃逸文件未被创建
    assert!(!dir.path().join("..").join("evil.txt").exists());
}

#[tokio::test]
async fn executor_missing_file_with_removes_errors() {
    let dir = tempfile::tempdir().unwrap();
    let patch = "*** Update File: missing.txt\n@@ -1,1 +1,1 @@\n-a\n+b\n";
    let err = apply_patch_action(patch, dir.path()).await.unwrap_err();
    assert!(err.to_string().contains("文件不存在"), "{err}");
}

#[tokio::test]
async fn executor_failure_is_atomic_no_partial_writes() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), "old\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("b.txt"), "bee\n")
        .await
        .unwrap();
    // 第二个文件无法定位 → 整个补丁中止，第一个文件不得被改写
    let patch = "\
*** Update File: a.txt
@@ -1,1 +1,1 @@
-old
+NEW
*** Update File: b.txt
@@ -1,1 +1,1 @@
-zzz
+B
";
    let err = apply_patch_action(patch, dir.path()).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("无法定位补丁块"), "{msg}");
    assert!(msg.contains("块 1"), "错误应点名块号: {msg}");
    assert_eq!(
        tokio::fs::read_to_string(dir.path().join("a.txt"))
            .await
            .unwrap(),
        "old\n"
    );
    assert_eq!(
        tokio::fs::read_to_string(dir.path().join("b.txt"))
            .await
            .unwrap(),
        "bee\n"
    );
}

#[tokio::test]
async fn executor_creates_new_file_from_patch() {
    let dir = tempfile::tempdir().unwrap();
    let patch = "*** Update File: new.txt\n@@ -0,0 +1,2 @@\n+l1\n+l2\n";
    let result = apply_patch_action(patch, dir.path()).await.unwrap();
    assert_eq!(result["total_files"].as_u64(), Some(1));
    assert_eq!(result["files"][0]["hunks_applied"].as_u64(), Some(1));
    assert_eq!(result["files"][0]["lines_changed"].as_u64(), Some(2));
    let on_disk = tokio::fs::read_to_string(dir.path().join("new.txt"))
        .await
        .unwrap();
    assert_eq!(on_disk, "l1\nl2");
}

#[tokio::test]
async fn executor_rejects_absolute_path() {
    // 绝对路径一律拒绝：补丁路径必须相对 base_dir（防多文件补丁以绝对路径
    // 绕过 registry 的逐文件路径安全检查）。
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("abs.txt");
    tokio::fs::write(&target, "old\n").await.unwrap();
    let patch = format!(
        "*** Update File: {}\n@@ -1,1 +1,1 @@\n-old\n+NEW\n",
        target.to_string_lossy()
    );
    let err = apply_patch_action(&patch, dir.path()).await.unwrap_err();
    assert!(err.to_string().contains("不支持绝对路径"), "{err}");
    // 目标文件未被改写（拒绝在落盘之前）
    assert_eq!(tokio::fs::read_to_string(&target).await.unwrap(), "old\n");
}

#[test]
fn leading_blank_context_line_does_not_misalign_apply() {
    // 防御性回归：直接构造含前导空 Context 的块（parse_patch 会跳过空格
    // 前缀空行，但 PatchLine 是公开类型，外部构造可能带入）——apply 阶段
    // 必须跳过被 match_window 裁剪的前导空 Context，否则 Remove 消费错位
    // （目标行未被删除，反而吞掉其相邻行）。
    let content = "keep1\nkeep2\ntarget\nkeep3\n";
    let hunk = PatchHunk {
        old_start: 1,
        old_count: 3,
        new_count: 2,
        lines: vec![
            PatchLine::Context(String::new()), // 前导空上下文（被裁剪）
            PatchLine::Context("keep2".to_string()),
            PatchLine::Remove("target".to_string()),
            PatchLine::Add("replacement".to_string()),
        ],
    };
    let out = apply_patch_to_content(content, &[hunk]).unwrap();
    assert_eq!(out, "keep1\nkeep2\nreplacement\nkeep3\n");
}

#[test]
fn fuzzy_threshold_constant_is_sane() {
    // 设计决策锁定：0.75 介于 codex 的 0.65 与严格 0.8 之间
    assert!((0.65..=0.8).contains(&FUZZY_RATIO_THRESHOLD));
    let _ = json!({});
}

/// 回归（2026-09-17 修复）：块内含「空格前缀的空上下文行」时，期望窗口必须
/// 保留该空行并与文件**精确匹配**，插入落在 `}` 之后（而非被模糊匹配上移
/// 一行、插到 `}` 之前）。
///
/// 修复前：解析层 `line.trim().is_empty()` 连「一个空格」的显式空上下文行
/// 也丢弃 → 期望窗口少一行 → 精确匹配失败 → 模糊匹配"整体平移一行"命中 →
/// 静默错位（无告警）。修复：解析层只忽略**完全空行**（`line.is_empty()`）。
#[test]
fn apply_patch_blank_context_line_must_not_shift_insertion() {
    let content = "impl Foo {\n    async fn load_before(&self) -> Result<()> {\n        Ok(self\n            .store\n            .load_before(x)\n            .await?)\n    }\n\n    /// `MAX(seq)` 覆盖（O(1)，不全量加载）。\n    async fn last_seq(&self) {}\n}\n";
    // 块内显式空上下文行（一个空格前缀）+ 尾部上下文
    let text = patch_for(
        "@@\n         Ok(self\n             .store\n             .load_before(x)\n             .await?)\n     }\n \n+    /// new\n+    async fn m(&self) {}\n+\n     /// `MAX(seq)` 覆盖（O(1)，不全量加载）。\n",
    );
    let files = parse_patch(&text).unwrap();
    let out = apply_patch_to_content(content, &files[0].hunks).unwrap();
    assert_eq!(
        out,
        "impl Foo {\n    async fn load_before(&self) -> Result<()> {\n        Ok(self\n            .store\n            .load_before(x)\n            .await?)\n    }\n\n    /// new\n    async fn m(&self) {}\n\n    /// `MAX(seq)` 覆盖（O(1)，不全量加载）。\n    async fn last_seq(&self) {}\n}\n",
        "空上下文行必须参与匹配：插入应落在 `}}` 之后、`/// MAX` 之前"
    );
}

/// 位置敏感守卫（2026-09-17 修复）：块内空行**未以空格前缀书写**（完全空行
/// → 被忽略）时，期望窗口少一行、与文件错开；此时模糊匹配的 ratio 仍可能
/// 达标（"整体平移一行"形态），必须**报"无法定位补丁块"**——宁可可见失败，
/// 绝不静默改错位置。
#[test]
fn fuzzy_fallback_rejects_shifted_window() {
    let content = "impl Foo {\n    async fn load_before(&self) -> Result<()> {\n        Ok(self\n            .store\n            .load_before(x)\n            .await?)\n    }\n\n    /// `MAX(seq)` 覆盖（O(1)，不全量加载）。\n    async fn last_seq(&self) {}\n}\n";
    // 纯空行写法（无空格前缀）→ 空上下文行丢失 → 期望窗口与文件错开一行
    let text = patch_for(
        "@@\n         Ok(self\n             .store\n             .load_before(x)\n             .await?)\n     }\n\n+    /// new\n+    async fn m(&self) {}\n+\n     /// `MAX(seq)` 覆盖（O(1)，不全量加载）。\n",
    );
    let files = parse_patch(&text).unwrap();
    let err = apply_patch_to_content(content, &files[0].hunks).unwrap_err();
    assert!(
        err.to_string().contains("无法定位补丁块"),
        "整体平移的窗口必须被拒（可见失败），而不是静默错位: {err}"
    );
}

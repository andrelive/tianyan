# 天演已知问题（Known Issues）

> 版本：0.4.0 · 更新日期：2026-09-13（0.2.0 遗留条目沿用；第 6 条已给出复核状态）

本文档记录当前版本已知的限制与问题，供用户与后续版本参考。按影响程度排序。

## 1. 模型生成截断工具调用（已护栏，未根治）

- **现象**：LLM 偶尔生成非法/截断的 JSON 工具参数，历史上曾导致整轮请求 400。
- **现状**：已加降级护栏（发送前校验，非法参数降级为 `{}`），不再 400 毒化整轮。
- **限制**：根因是模型行为（生成不完整 JSON），无法从源头避免；护栏兜住症状。
- **影响**：低（护栏生效后不影响使用）。

## 2. apply_edit 精确匹配的固有脆弱性

- **现象**：`apply_edit` 的 `old_string` 需**逐字节精确匹配**（含空行、缩进、行尾）。漏空行、抄错缩进、CRLF/LF 差异都会导致"未找到"。
- **现状**：已做行尾归一化（CRLF/LF 容差）+ "未找到"报错带最近行提示；工具描述引导优先用 `apply_patch`（上下文锚定，更稳）。
- **限制**：内容匹配本质要求精确复现，模型偶尔仍会失败（报错后可纠正）。
- **影响**：中（编辑失败需重试，但不改错文件）。

## 3. 超大单行文件不适用行编辑

- **现象**：压缩后的 JS/JSON 单行（几百 KB）无法用 `apply_edit`（整行替换不现实），`read_file` 也受 50KB 字节预算限制。
- **建议**：用 `apply_patch` 按 diff 片段修改，或重新生成文件。
- **影响**：低（病态场景）。

## 4. 会话内规则/记忆不刷新（ADR-012 缓存优先）

- **现象**：soul/rules/memories 前缀在会话**第一轮冻结**（快照），会话中途学到的规则/记忆**不注入当前轮**。
- **原因**：前缀零漂移以命中 DeepSeek 前缀缓存（省 token/延迟），是有意设计。
- **缓解**：agent 可用 `search_vfs` 按需检索规则（Agent 命名空间）/记忆（Memory 命名空间）。
- **影响**：低（单主题会话无感；长会话换话题时 agent 需主动检索）。

## 5. 自动更新依赖发布流水线

- **现象**：应用启动时检查 GitHub Releases 的 `latest.json`；未发布时记"更新检查失败"。
- **现状**：`tauri-plugin-updater` + 签名密钥 + GitHub Actions 流水线已就绪；需推送 `v*` 标签触发发布。
- **限制**：自签名更新包首次运行 Windows SmartScreen 可能警告（点"仍要运行"）。
- **影响**：低（发布后生效）。

## 6. workspace API 会话工作目录绑定（验证时发现）

- **现象**：`PUT /sessions/{id}/workspace` 绑定会话工作目录后，workspace API 部分场景仍回退到全局 `working_directory`。
- **影响**：中（多工作目录场景受限；单工作目录不受影响）。
- **状态**：复核中（0.3.15）——代码侧已按 `session_id` 解析绑定目录
  （`server/src/api/workspace/services.rs` 的 `resolve_workdir`），待实机验证多工作目录场景。

## 7. Windows 构建环境问题

- **现象**：`cargo test` 在 `opt-level=2` 下 rust-lld 崩溃（STATUS_ACCESS_VIOLATION）。
- **缓解**：`CARGO_PROFILE_TEST_OPT_LEVEL=0` 环境变量绕过；`cargo build`/`cargo check` 不受影响。
- **影响**：低（仅影响本地测试）。

## 8. 数据目录搬迁后旧目录可能残留被锁文件

- **现象**：迁移使用 copy 而非 rename（Windows 上 SQLite 文件可能被锁，无 FILE_SHARE_DELETE）。
- **现状**：**循环引用已彻底修复**（分层重构，ADR-021）：handler 经 TaskResultSink 接口回写（Weak）、ScheduleTaskTool 经 channel 解耦、manager 经 TaskRegistrar 接口注册——依赖单向向下，**不再依赖优雅关停特殊处理**；迁移验证旧目录完全清空（Agent/SqliteDb 全部释放）。
- **限制**：极端情况下（如外部进程占用）旧目录可能残留文件，需手动清理。
- **影响**：低（迁移后新目录数据完整可用）。

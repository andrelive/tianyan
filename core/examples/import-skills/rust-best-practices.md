---
id: rust-best-practices
abstract: Rust 惯用法基线（Apollo 手册要点）——借用/克隆取舍、错误处理、性能心态、lint 与文档；仓库标准优先 | 适用场景: 写 Rust 代码、审查/重构 Rust、借用与克隆取舍、错误处理设计
---
# Rust 最佳实践（基线）

**ID**: `rust-best-practices`

**版本**: 1.0.0

**描述**: 基于 Apollo Rust Best Practices Handbook 的精简操作基线；**仓库标准永远优先**。

**状态**: 正式

## 适用场景

- 写新的 Rust 函数 / 模块
- 审查或重构 Rust 代码
- 纠结借用 vs 克隆、错误处理、性能取舍
- 给 Rust 项目补测试与文档

## 使用说明

### 借用与所有权

- 优先 `&T` 而不是 `.clone()`（除非确需所有权转移）；
- 函数参数用 `&str` / `&[T]` 取代 `String` / `Vec<T>`；
- 小的 `Copy` 类型（≤24 字节）可以按值传；
- 所有权含糊时用 `Cow<'_, T>`。

### 错误处理

- 可失败操作返回 `Result<T, E>`；生产路径禁 `panic!`、禁 `unwrap()/expect()`（测试除外）；
- 错误用 `?` 传播，避免手写 match 链；
- **仓库覆盖（天演）**：统一 `TianyanError`（仅 4 变体 + 语义谓词 `not_found/conflict/...`）——**不要**给天演引入 thiserror/anyhow 或新错误类型；构造用谓词、判定用谓词、**禁止对错误消息做字符串匹配分类**。

### 性能心态

- 基准一律 `--release`；
- 循环里避免克隆；对 `Copy` 类型用 `.iter()` 而非 `.into_iter()`；
- 优先迭代器而不是手写循环；避免中间 `.collect()`；
- 先测量后优化（没有基线不优化）。

### Lint

- 常规跑：`cargo clippy --all-targets --all-features -- -D warnings`（天演 workspace 门禁为 `cargo clippy --workspace -- -D warnings`）；
- 关注：`clippy::perf`、`unwrap_used`、`expect_used`（天演为 warn 级）。

### 注释与文档

- 公开 API 用 `///` 文档注释（`//!` 模块级）；
- 注释写**为什么**（不写"是什么"——代码自己说）；
- 复杂不变量、契约、反直觉取舍写进文档注释。

### 测试

- 测试命名表达行为；一个测试一个断言组；
- 测试通过公共接口（seam），不测私有实现；
- 天演补充：**新回归测试必须先证明能红**（注入旧行为必红——"反向验证"）。

## 来源

- 外部标准化技能库移植（`~/.agents/skills\rust-best-practices`），2026-09-14 导入
- 原始出处：Apollo GraphQL Rust Best Practices Handbook（MIT），含天演仓库覆盖项

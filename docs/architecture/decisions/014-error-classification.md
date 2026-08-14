# ADR-014: 错误语义化 —— 构造器/谓词模式（4 变体约束下的结构化分类）

**日期**: 2026-08-13
**状态**: ✅ 已采纳
**影响范围**: `core/src/common/error.rs`（构造器 + 谓词）、executor 语义编辑错误发射（edit/patch/hashline）、server 错误分类消费点（`api/shared/error.rs`、`api/workspace/services.rs`、`api/insights/handlers.rs`）

---

## 背景

`TianyanError` 仅保留 4 个变体（Io / Json / Toml / Custom），模块内部错误通过 `Custom(String)` 传递（AGENTS.md 硬约束）。此前仅有 `not_found` 一个语义化入口（`NOT_FOUND_PREFIX` + `not_found()` + `is_not_found()`，error.rs:40-60）。

Server 层为了把 core 错误映射为 HTTP 语义（404/400/409/403/504），在 **5 个站点各自做字符串匹配**：

| 站点 | 匹配方式 | 脆弱性 |
|------|---------|--------|
| `api/shared/error.rs` From\<TianyanError\> | `msg.starts_with("操作不被允许：")` 等 8 个前缀 | 前缀字面量散落；与 core 发射端无契约 |
| `api/workspace/services.rs` executor_error_to_api | `msg.contains("锚点")/("旧内容不匹配")/("补丁")/("块")` | **子串匹配过宽**——任何含"块"字的错误都被判 409 |
| `api/workspace/services.rs` snapshot_error_to_api | `msg.contains("快照")` | **任何含"快照"的错误都被判 404**（解析失败也 404） |
| `api/insights/handlers.rs` | `msg.contains("审批请求不存在或已超时")` | 与 core 固定消息文本耦合 |
| `server/core_bridge.rs` | 消息文本匹配 | 连接测试专用，已随内联处理 |

错误消息是**显示契约**，不是**分类契约**——跨层分类依赖消息文本导致：新增错误类别必须同步改 5 处；消息措辞调整会静默改变 HTTP 状态码；`contains` 过宽造成误分类。

## 决策

在 4 变体约束下，扩展既有 `not_found` 先例为通用**构造器/谓词模式**：

### 1. 语义 KIND 常量 + 构造器 + 谓词（core 单一来源）

`core/src/common/error.rs` 增加：

| KIND 常量 | 构造器 | 谓词 | 语义 |
|-----------|--------|------|------|
| `KIND_CONFLICT = "冲突"` | `conflict(detail)` | `is_conflict()` | 语义编辑锚点/旧内容/补丁定位不匹配 |
| `KIND_INVALID_INPUT = "无效输入"` | `invalid_input(detail)` | `is_invalid_input()` | 参数缺失/格式错误/非法路径 |
| `KIND_PERMISSION = "操作不被允许"` | `permission(detail)` | `is_permission()` | 权限拒绝/安全策略（含 `Io(PermissionDenied)`） |
| `KIND_TIMEOUT = "操作超时"` | `timeout(detail)` | `is_timeout()` | 请求/调用超时 |

约定：消息格式统一为 `"{kind}：{detail}"`；谓词以 `starts_with(kind)` 判定（与 `is_not_found` 同构）。**所有 KIND 前缀互不为对方前缀**（前缀碰撞矩阵测试守卫，error.rs `test_kind_prefixes_do_not_collide`）。

### 2. 统一超时前缀（消除别名）

`From<reqwest::Error>` 的超时分支原发射 `"请求超时：…"`，与 `KIND_TIMEOUT = "操作超时"` 语义重叠——迁移为 `TianyanError::timeout(err)`，收敛到单一规范前缀。

### 3. 发射端迁移（executor 语义编辑）

| 原发射 | 迁移后 |
|--------|--------|
| `edit.rs` 锚点不匹配 / 旧内容不匹配（`Custom`） | `TianyanError::conflict(...)` |
| `patch.rs` 无法定位补丁块 / 补丁块重叠 | `TianyanError::conflict(...)` |
| `patch.rs` 非法路径 / 绝对路径 | `TianyanError::invalid_input(...)` |
| `patch.rs` 文件不存在 | `TianyanError::not_found(...)` |
| `actions.rs` 文件不存在 | `TianyanError::not_found(...)` |
| `approval/workflow.rs` 审批请求不存在或已超时 | `TianyanError::not_found(...)` |
| `snapshot/mod.rs` 快照不存在 | `TianyanError::not_found(...)` |

detail 保留原模块前缀（`executor: apply_edit: …`）——**消息显示契约不变**，仅前缀语义化。

### 4. 消费端迁移（server 谓词消费）

5 个分类站点全部改为谓词判定（`is_conflict` → 409、`is_invalid_input` → 400、`is_permission` → 403、`is_timeout` → 504、`is_not_found` → 404），**不再出现对错误消息文本的 starts_with/contains 分类**。`executor_error_to_api` 与 `snapshot_error_to_api` 的过宽 `contains` 判定随之消除（副作用修正：含"快照"的解析失败错误不再误判 404）。

## 后果

**正面**：
- 错误分类契约收敛到 core 单一来源——新增类别只改 `error.rs` 一处；发射端用构造器、消费端用谓词，跨层无文本耦合
- 消除了 `contains` 过宽误分类（"块"/"快照"子串）的潜在 bug
- 测试可直接断言谓词（`assert!(err.is_conflict())`），不再依赖消息文本

**负面/边界**：
- 谓词仍是 `starts_with` 字符串判定（4 变体约束的代价）；KIND 前缀碰撞由测试矩阵守卫
- 既有 `Custom("配置错误：")` / `"认证失败："` / `"安全错误："` / `"无效路径："` 前缀保留为局部约定（未纳入 KIND 体系——调用点少且无跨层分类需求，避免扩大改动面）
- **已知债务**：`vfs/backend/sqlite_db.rs` 的 `open`/`open_in_memory`/`init_all_schemas` 返回 `rusqlite::Error`（公共 API 泄漏外部错误类型，违反"仅 TianyanError"约束）。3 个调用点（vfs 后端、observability、server lib.rs），收益低、改动 3 处——**记录为债务，后续单独处理**

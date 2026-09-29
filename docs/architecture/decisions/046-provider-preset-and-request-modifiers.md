# ADR-046: Provider 预置表与请求修饰单点（头差异数据化）

**日期**: 2026-09-29
**状态**: ✅ 已采纳（已实施）
**影响范围**: 配置（`core/src/config/model.rs` 新增 `HeaderBinding` / `HeaderSource` +
`ProviderConfig` 扩展；新增 `core/src/config/presets.rs` 预置表）、客户端
（`core/src/model/provider/client.rs` UA / 动态头解析）、chat
（`core/src/model/provider/chat.rs` 发送单点注入 + 非流式路径统一手写）、
agent（`loop.rs` session_id 贯通）、server（预置 API + discovery UA）、
前端（`config-transform.ts` 透传修复 + 预置选择 UI）

---

## 背景

1. **opencode Go 网关的接入要求**（官方文档 "Where can I use it?"）：
   专属 User-Agent（如 `my-coding-agent/1.0`，**不得**用通用 SDK / HTTP 库名）；
   每段对话在 `x-opencode-session` 中发送**稳定会话 ID**（用于路由与提示词缓存）。
   官方将 "missing or incomplete session support" 的客户端列为 Known Problematic Clients。
2. **ADR-038 的覆盖缺口**：038 把 body 字段级差异收敛为方言数据，但 HTTP 头差异
   （UA / 会话头）没有结构——头类差异是下一个需要数据化的"声明式差异"面。
3. **现状缺陷（本次一并修）**：
   - 所有 provider 请求发 reqwest 默认 UA（`reqwest/x.y.z`，通用库名）——被 opencode
     明确拒绝，风控网关也会拦截；
   - **非流式路径**（`create_byot`）不经过发送单点，`ProviderConfig.headers` 静默不生效；
   - 前端 `config-transform.ts` 丢 `dialect` / `first_token_timeout`（UI 保存即重置）；
   - endpoint / 方言 / 头要求全靠用户手写，无预置机制。

## 决策

### 1. 预置表：provider 的"数据接入面"

`config/presets.rs` 新增 `ProviderPreset` 静态表。一条预置 = 一组默认值
（endpoint / dialect / UA / 动态头 / 是否需 key / 环境变量提示 / 描述），
**与 ProviderConfig 同构**——"内置预置＝出厂默认配置，用户配置＝覆盖"。

匹配：显式 `preset` 字段 > `name` 精确匹配（`eq_ignore_ascii_case`）。
首批条目：`ollama`（本地）/ `ollama-cloud` / `opencode` / `opencode-go` /
`opencode-zen` / `deepseek`。

### 2. 请求修饰三层单点（与 ADR-038 同构）

| 层 | 目标 | 落地 |
|---|---|---|
| L1 配置 schema | `ProviderConfig`（+ 前端 / 预置表共用） | 增补 `user_agent: Option<String>`、`dynamic_headers: Option<Vec<HeaderBinding>>`、`preset: Option<String>` |
| L2 解析 | 构造时解析一次、请求路径零判定 | `resolve_dynamic_headers()` / `resolve_user_agent()` / `resolve_endpoint()`（与 `resolve_dialect` 并列） |
| L3 应用 | 发送单点 | `send_chat_request` 内：静态 headers → 动态头（请求级覆盖 client 级） |

### 3. 动态头 = 受控绑定表（B 方案）

```rust
pub struct HeaderBinding { pub name: String, pub source: HeaderSource }
pub enum HeaderSource {
    #[serde(rename = "session_id")]
    SessionId,
}
```

- **受控枚举**：变量源是编译期枚举，**永不引入 `${...}` 模板字符串**
  （模板把声明式变半编程：转义 / 注入 / 未知变量 / 错误语义全是新失败模式）。
  新变量源 = 加一个变体 + 求值处一行。
- 预置中 opencode 系列携带一行绑定：`x-opencode-session → SessionId`。

### 4. 合并与失败语义

- **合并**：显式 > 预置 > 默认（整体替换，不逐条合并）；
  `None` = 用预置；`Some([])` = 显式清空（退出预置行为）。
- **校验（fail fast）**：header 名合法（`HeaderName` 解析）、无重名（大小写不敏感，
  静态 ∪ 动态合并视图）——配置校验（`validate()`）与 client 构造两处。
- **降级（请求时）**：变量无值（压缩 / judge 等非会话请求）→ 跳过该头，不失败；
  值非法 → warn + 跳过（头是附属信息，不得拖死请求本体，对齐 038"可见失败容错兜底"）。
- **UA 解析链**：`user_agent` 字段 > `headers` 手工项 > 预置 > `tianyan/{VERSION}`
  （应用到 reqwest client 级——流式 / 非流式 / embedding / vision 全路径生效）。

### 5. 边界（防止结构膨胀）

- 只吸收**声明式**头差异：静态常量头（长尾逃生舱 = `headers`）、身份头（UA）、
  动态头（绑定表）；**协议级**差异（Anthropic `x-api-key` / 版本头 + 消息结构）
  不吸收，按 038 既有约定拆子目录；语义推断类留启发式（038 三分法不变）。
- 会话头覆盖范围 = **对话流量**：主 agent 与子代理（流式 + 非流式，`session_id`
  来自 AgentLoop）；压缩 / judge / reminder 首期不带（非对话，偶发单次请求）。

### 6. endpoint 隐式补全（2026-09-29 追加）

`endpoint` 与其他修饰字段同链：**显式配置 > 预置 > 报错**。命中预置的 provider
endpoint 可留空（配置文件更简洁、端点跟随预置更新）；**未命中预置且留空 = 校验失败**
（不引入隐式默认端点——避免"配置指向未知服务"）。

可见性由 `GET /api/v1/config` 下发的 `resolved_endpoints`（key = provider name）保证：
UI 在端点输入框留空时只读展示"留空 = 使用预置端点：xxx"，扫描 / 测试连接均改用生效端点。

同一响应还下发 `provider_presets`（预置命中信息：预置显示名 / 实际生效的动态头 /
端点来源）——UI 在 provider 卡片渲染「⚡ 内置预置：xxx」徽章。**预置在运行时静默
生效，但在 UI 上绝不隐形**（回答"用户怎么知道这是内置的"的可见性问题）。

## 影响

- **非流式路径统一手写 JSON 层**（对齐流式 `create_raw_stream`）：
  修复 headers 不生效 + 动态头全覆盖 + 错误分类单点（`classify_http_status`）。
- **UA 全局变更**：所有 provider 请求从 `reqwest/x.y.z` 变为 `tianyan/{VERSION}`。
- **前端契约**：透传 `dialect` / `first_token_timeout`（修复既有丢失）+
  `user_agent` / `dynamic_headers`（新）。
- 存量配置零迁移：新字段全部 `default` + `skip_serializing_if`，旧配置行为不变
  （UA 除外——那是本次修复目标）。

## 实施记录（2026-09-29）

| 层 | 文件 | 内容 |
|---|---|---|
| 配置 | `core/src/config/model.rs` | `HeaderBinding` / `HeaderSource`（受控枚举 + `resolve` / `wire_name`）；`ProviderConfig` 增 `user_agent` / `dynamic_headers` / `preset`；`matched_preset` / `resolve_dynamic_headers` / `resolve_user_agent`（显式 > 预置 > 默认）；`validate_dynamic_headers`（名合法 + 重名，fail fast）；`Default`（对齐 serde 默认，供构造点 `..Default::default()`） |
| 配置 | `core/src/config/presets.rs`（新） | `ProviderPreset` 表（ollama / ollama-cloud / opencode / opencode-go / deepseek）+ `find_preset`（大小写不敏感）；opencode 系携带 `x-opencode-session → session_id` 绑定 |
| 请求 | `core/src/model/provider/client.rs` | UA 解析并设到 reqwest client（流式 / 非流式 / embedding / vision 全路径）；`dynamic_headers` 构造时解析一次（`HeaderName` 校验 fail fast） |
| 请求 | `core/src/model/provider/chat.rs` | `send_chat_request` 增 `session_id` 参数并在静态头之后注入动态头（无值跳过 / 值非法 warn 跳过）；**非流式改手写 JSON 层**（对齐流式、复用发送单点）；`classify_upstream_error` 删除（分类单点统一为 `classify_http_status`） |
| 请求 | `core/src/model/types/chat.rs` | `ChatCompletionRequest.session_id`（`#[serde(skip)]`，内部元数据、不上 wire） |
| 请求 | `core/src/model/provider/vision.rs` | 调用点适配（VLM 非会话流量 → `None`，跳过动态头） |
| Agent | `core/src/agent/loop.rs` | `prepare_request` 增 `session_id` 并链式 `.with_session_id(...)`（run / 流式两路径唯一填充点；子代理同路径自动受益） |
| Server | `server/src/api/config/discovery_handlers.rs` + `routes.rs` | `GET /api/v1/config/provider-presets`（静态数据下发，单一事实源）；test / scan 收敛 `build_http_client` + 专属 UA |
| 前端 | `types.ts` / `config-transform.ts` / `api-client.ts` / `ModelsTab.tsx` / `SettingsPanel.tsx` | 字段透传（**修复** `dialect` / `first_token_timeout` 保存丢失；新增 `user_agent` / `dynamic_headers`，空数组 = 显式清空不省略）；预置选择器（懒加载 + 一键填草稿 name/endpoint + 动态头提示） |
| 文档 | `AGENTS.md` / `config.example.toml` | ADR 索引 + 预置与请求修饰示例 |

**验证**：前端 `tsc --noEmit` / vitest（56 文件 435 测试，含新增预置选择交互与透传回归）/ eslint + prettier 全绿；
`cargo fmt --all --check` 干净；`cargo check -p tianyan-core -p tianyan-server --all-targets` 通过；
`cargo test -p tianyan-core --lib` **1453 passed / 0 failed**、`cargo test -p tianyan-server`
**198 passed / 0 failed**（含动态头注入 mock 断言、UA、预置解析与校验拒绝、旧配置兼容）；
`scripts/test.ps1 lint` 统一门禁（fmt + clippy --all-targets + 工具目录 freshness + 前端 lint/typecheck）通过。

## 验收

- 判别力：动态头注入（mock server 断言 `x-opencode-session` = 请求 `session_id`）、
  UA 断言、合并语义（None / Some([]) / Some([...])）、校验拒绝（非法名 / 重名）、
  非流式路径头生效断言。
- 全量：`cargo test -p tianyan-core --lib` / `cargo test -p tianyan-server` /
  `cargo fmt --check` / `cargo clippy --workspace` + 前端 `vitest` / `typecheck`。

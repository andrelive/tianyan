# AGENTS.md

**代码是唯一事实来源。** `docs/` 中的文档提供架构引导和设计意图，但可能过时。当文档与代码冲突时，以代码为准。实际结构请用 `grep` / `glob` 验证。

## 构建与检查命令

```powershell
cargo check --workspace                     # 快速编译检查
cargo fmt --all -- --check                  # 格式检查
cargo fmt --all                             # 自动格式化
cargo clippy --workspace                    # Clippy（不用 -D warnings，missing_docs 太多）
cargo test -p tianyan-core --lib            # 单元测试
cargo test -p tianyan-core vfs::backend::local -- --nocapture  # 指定测试模块
.\scripts\test.ps1 lint                     # fmt + clippy
.\scripts\test.ps1 unit                     # 单元测试
.\scripts\test.ps1 all                      # lint + unit + integration + e2e + bench
```

## 工作区结构

4 crate，单 `Cargo.toml` workspace (resolver = "2")：

| Crate | Package | 类型 |
|-------|---------|------|
| `core/` | `tianyan-core` (lib: `tianyan`) | 纯库 |
| `server/` | `tianyan-server` | Axum HTTP 服务 |
| `gui/` | `tianyan-gui` | Yew WASM 前端 |
| `tauri/` | `tianyan-tauri` | Tauri 桌面包装 |

共享依赖统一在 workspace `[workspace.dependencies]` 中定义，crate 引用即可。

## 代码规范

- `unsafe_code = "deny"` — 禁用 unsafe
- `missing_docs = "warn"` — pub 项需文档注释
- Clippy: `unwrap_used`、`expect_used`、`unwrap_in_result` 均为 `warn`
- **不要用 `//` 行注释**，公开 API 用 `///` / `//!`

---

## 核心架构决策

### 决策 1: VFS 双层摘要索引 —— 基础机制

VFS 是所有上下文（知识库、记忆、技能、规则）的统一存储与检索层，采用**三层内容 + 双向量 RRF 融合**：

| 层级 | 名称 | Token | 向量 | 用途 |
|------|------|-------|------|------|
| L0 | Abstract | ~100 | `abstract_vector` | 向量搜索、快速过滤 |
| L1 | Overview | ~2K | `overview_vector` | 内容导航、重排序 |
| L2 | Detail | 无限制 | — | 完整内容，按需加载 |

`SummaryEngine` 通过 LLM 为每条 VFS 条目生成 L0/L1 摘要。检索时：查询文本 → embed → Qdrant RRF 融合搜索 `abstract_vector` + `overview_vector` → `ContentLoadStrategy::from_score()` 按分数分层加载（>0.85→L2, >0.6→L1, 其他→L0）。

**此机制是项目底层基础，其他设计必须妥协于它：**

- **知识库不做 chunk**：文档完整解析后直接写入 VFS L2 Detail，SummaryEngine 生成 L0/L1 摘要，双层检索自然替代 chunk-based RAG。`Chunker` 已移除。（`core/src/knowledge/ingestor/`）
- **技能渐进式披露**：技能存于 `tianyan://skill/` 命名空间。L0 Abstract 用于快速发现（`SkillManager::list_available_skills()` 读取所有技能的 abstract），L2 Detail（`skill.md`）按需加载完整定义。`Skill::description_embedding` 向量支持语义检索。这不同于业界常见的文件系统全量加载模式。（`core/src/skills/`）
- **图像双通道**：VLM 生成文本描述 → 文本 embedding 用于 L0/L1 检索，同时 `embed_image()` 生成 `visual_vector` 用于视觉相似度搜索。无单独 `VisionEncoder` trait。
- 所有命名空间（User、Session、Memory、Knowledge、Agent、Skill）共享同一套机制。

关键文件：`core/src/vfs/traits.rs`（`VfsSearch` trait）、`core/src/vfs/vfs_impl.rs`（`search()` 实现）、`core/src/vfs/vector/qdrant.rs`（RRF 融合）、`core/src/context/retrieval/retriever.rs`（`DualLayerRetriever`）。

### 决策 2: StructuredMessage —— 核心数据结构

`StructuredMessage` 是贯穿持久化、会话组装、跟踪、统计的单一真相源（`core/src/common/types/structured_message.rs`）：

```rust
pub struct StructuredMessage {
    pub id: String, pub parent_id: Option<String>, pub role: MessageRole,
    pub parts: Vec<Part>,                  // Text | Reasoning | ToolCall | ToolResult
    pub tokens: DetailedTokenUsage,        // input/output/reasoning/cache
    pub cost: f64, pub model_id: Option<String>, pub time: MessageTime,
    pub session_id: String, pub finish: Option<String>,
    pub compression_marker: bool,          // ★ 会话压缩锚点
}
```

**四个核心职责：**

1. **持久化**：JSONL 格式写入 VFS。`AgentLoop` 每产生一条消息，实时调 `SessionManager::add_structured_message()` 落盘。工具调用消息不丢弃，全部持久化。

2. **会话组装**：存储与传输分离 —— `StructuredMessage`（存储层）↔ `Message`（传输层，仅 LLM 需要的字段）。`ContextAssembler::assemble()` 将持久化消息转为传输格式，顺序：soul → rules+memories → history → current input。

3. **会话跟踪**：`compression_marker` 标记压缩产生的摘要消息。加载会话时反向扫描到最近 marker，只加载 marker 及之后的消息（旧消息保留在磁盘）。压缩触发条件：marker 后 > 6 条消息。

4. **Token 统计**：LLM 响应 `TokenUsage` → `AgentLoop` 捕获 → `DetailedTokenUsage` 存入字段 → 持久化 → 聚合到 `AgentState.total_tokens`。每个消息精确记录 token 消耗和成本。

`Agent::process_message(session_id, msg)` 自闭环：加载 session → 构建 SessionState → 执行 loop → 持久化所有消息 → 压缩 → 返回响应。Server 不需要手动管理 session。

### 决策 3: 组件工具化

知识库查询、技能调用等能力封装为 OpenAI function calling 兼容的工具，由大模型通过 `tool_call` 自主调用，提升任务执行稳定性和可组合性。

当前已有基础：`ToolRegistry` 注册 **13 个工具**（`read_file`、`write_file`、`execute_command`、`search_code`、`search_knowledge`、`vfs_read`、`vfs_list`、`call_skill`、`run_tests`、`verify_build`、`ask_user`、`self_check`、`delegate_to_agent`），其中 `call_skill` 桥接到 `SkillExecutor`。

---

## 模块速览

- **Model**: `ModelServices` 是 `Arc<dyn Chat/Embedding/Vlm>Service` 的容器，不路由、不重试。`RetryService` 已移除，禁止重新引入。`model/provider/` 用 `pub(crate)`，外部通过 `ModelServices` 访问。
- **Session**: `SessionManager` 管理 VFS 中的会话文件。`load_session_from_vfs()` 用 `compression_marker` 截断。
- **Context Pipeline**: `ContextPipeline::load_injectable()` 加载 soul+rules+memories（soul 首次加载后缓存）；`compress_if_needed()` 独立执行压缩。`TokenBudget` 已删除。
- **Agent**: `Agent::process_message()` 自闭环管理 session。`AgentLoop` 持有 `SessionManager` 实时持久化。
- **Scheduler**: `RuleTask → RuleSuggester → RuleRecorder` 是定时 cron 任务。导入路径 `tianyan::scheduler::tasks::*`。
- **Config**: 两套 config 类型（已知重复，待统一）。无 `max_retries`。
- **Error**: 所有错误用 `TianyanError`，禁止引入新错误类型。

---

### 决策 4: 上下文组装前缀匹配原则

`ContextAssembler::assemble()` 将持久化消息拼装为 LLM 输入时，严格遵循**固定前缀 + 可变后缀**顺序：

```
soul → rules+memories → history(from compression_marker) → current input
```

**设计理由**：

- **固定前缀（soul + rules + memories）**：会话期间不会变化。LLM Provider（OpenAI、Anthropic 等）利用前缀匹配缓存，固定前缀只计算一次，后续请求仅处理可变后缀，显著降低延迟和成本。
- **可变后缀（history）**：对话历史随每轮交互增长。compression_marker 决定拼接起点——从最近 marker 之后的消息开始加载，marker 之前的旧消息不纳入本次输入（但保留在磁盘）。
- **绝对禁止**把 soul/rules/memories 放在 history 之后——这会破坏前缀缓存，让每次请求都重新计算全部 token。

实现位置：`core/src/context/assembler.rs` 的 `assemble()` 方法，顺序不可变更。

---

## 设计原则

- **天演的开发目标是实现一个部署在个人PC上的通用智能体。** 核心意图是为了最大化、全面化地发挥智能体的能力。不要将特定语言或工具链的假设硬编码到系统中——智能体通过 `execute_command` 使用用户环境中已有的任何工具进行质量验证、构建、测试等操作，而非通过硬编码的验证门控。Agent 的工具集面向通用任务设计，不预设用户的技术栈。
- **One adapter = hypothetical seam. Two adapters = real seam.** 不因"未来可能需要"而引入 trait。**测试 mock 算 adapter**——有生产实现 + 测试 mock 时，trait 是真实的 seam（依赖反转）。
  - `ChatService`（1 生产 + 5 测试 mock）、`EmbeddingService`（1 生产 + 1 测试 mock）→ 真实 seam
  - `VlmService`（1 生产 + 1 测试 mock）、`ServiceDiscovery`（1 生产 + 1 测试 mock）→ 真实 seam
  - 注：以上两个 trait 的 mock（`MockVlmService`、`MockServiceDiscovery`）定义在 `core/src/model/traits.rs`
- **Trait 实现直接在 `impl Trait for Struct` 中完成**，不定义中间 inherent 方法做委托。
- **Providers 应 fail fast**，不做重试/退避。
- **VFS 初始化分离**：基础设施 → `core/src/vfs/vfs_impl.rs::initialize()`；应用内容（soul.md、learned 目录）→ `server/src/lib.rs::bootstrap_app_vfs()`。
- **`DEFAULT_SOUL`** 在 `core/src/agent/mod.rs`，通过 `include_str!` 构建。不要直接读文件。
- **已有链路不叠加抽象**：当现有组件已形成功能闭环（如 MemoryTask → MemoryExtractor → ContextPipeline 的记忆链路），不需要额外封装 Manager/Coordinator 层。与 seam 原则一脉相承——不因"设计整洁"而引入不必要的抽象。
- **泛型有边界**：位于 crate 边界（被其他 crate 实例化）的类型，优先使用 `Arc<dyn Trait>` 而非泛型参数。泛型适用于模块内部封装，但对外暴露时应考虑 trait object 兼容性。`KnowledgeIngestor` 曾以 4 个泛型参数对外暴露导致 Server 层无法直接实例化，已重构为纯 `Arc<dyn ...>` 的具体 struct，消除了所有泛型。
- **层间职责分明，不传递不属于本层的决策**：每一层只对自己职责范围内的信息负责。持久化层只管存，执行层才决定用什么模型执行。若中间层无法从上游获取某个值、也无权限从配置读取，就应当传递 `Option::None` 让下游自行决断，而不是拍一个硬编码值塞进去。例如 `SessionService` 不应在创建会话时写死 `model: "gpt-4"`——模型选择是 Agent 运行时的配置决策，不属于会话管理层。

## 常见陷阱

- `tianyan` 和 `tianyan-core` 是同一个包（lib name: `tianyan`），导入用 `tianyan::...`。
- `ServiceDiscovery::is_available()` 是 `async` —— 别忘了 `await`。
- 配置文件查找顺序：`./tianyan.toml` → `~/.config/tianyan/tianyan.toml` → `~/.tianyan/tianyan.toml`。
- 测试不依赖外部服务，用 `MockVectorStorage` 和 temp dir 隔离。

# 天演设计原则

> 从 AGENTS.md 提取，作为架构设计的约束参考。变更需同时更新 [module-map.md](module-map.md) 和相关 ADR。

---

## 项目定位

天演的开发目标是实现一个部署在个人 PC 上的通用智能体。核心意图是最大化、全面化地发挥智能体能力。不将特定语言或工具链的假设硬编码到系统中——智能体通过 `execute_command` 使用用户环境中已有的任何工具进行质量验证、构建、测试等操作，而非通过硬编码的验证门控。Agent 工具集面向通用任务设计，不预设用户的技术栈。

---

## 架构原则

### Seam 原则
**One adapter = hypothetical seam. Two adapters = real seam.** 不因"未来可能需要"而引入 trait。测试 mock 算 adapter——有生产实现 + 测试 mock 时，trait 是真实的 seam（依赖反转）。

- `ChatService`（1 生产 + 5 测试 mock）、`EmbeddingService`（1 生产 + 1 测试 mock）→ 真实 seam
- `VlmService`（1 生产 + 1 测试 mock）、`ServiceDiscovery`（1 生产 + 1 测试 mock）→ 真实 seam

### Trait 实现
Trait 实现直接在 `impl Trait for Struct` 中完成，不定义中间 inherent 方法做委托。

### 泛型边界
位于 crate 边界（被其他 crate 实例化）的类型，优先使用 `Arc<dyn Trait>` 而非泛型参数。泛型适用于模块内部封装，但对外暴露时应考虑 trait object 兼容性。

### 已有链路不叠加抽象
当现有组件已形成功能闭环（如 MemoryTask → MemoryExtractor → ContextPipeline 的记忆链路），不需要额外封装 Manager/Coordinator 层。

---

## 职责原则

### 层间职责分明
每一层只对自己职责范围内的信息负责。持久化层只管存，执行层才决定用什么模型执行。若中间层无法从上游获取某个值、也无权限从配置读取，就应当传递 `Option::None` 让下游自行决断。

### VFS 自带容错，上层不重复检查
SQLite 的 WAL 模式和 `ON CONFLICT` 语义已处理并发和不存在的情况。上层调用方不需要在写之前先 `exists` 检查。

---

## 运行时原则

### Providers fail fast
不做重试/退避。`RetryService` 已移除，禁止重新引入。

### 工具不做自主多轮决策
工具（如 `delegate_to_agent`）采用多轮调用——LLM 每次响应要么给出最终答案，要么请求 tool_calls。决策权永远在 LLM 手中，工具只是执行者 + 信使。`delegate_to_agent` 采用有界循环（默认 5 轮）防止失控。

---

## 初始化原则

### VFS 初始化分离
- 基础设施 → `core/src/vfs/vfs_impl.rs::initialize()`
- 应用内容（soul.md、learned 目录）→ `server/src/lib.rs::bootstrap_app_vfs()`

### DEFAULT_SOUL
在 `core/src/agent/mod.rs`，通过 `include_str!` 构建。不要直接读文件。

---

## 常见陷阱

- `tianyan` 和 `tianyan-core` 是同一个包（lib name: `tianyan`），导入用 `tianyan::...`
- `ServiceDiscovery::is_available()` 是 `async` —— 别忘了 `await`
- 配置文件查找顺序：`./tianyan.toml` → `~/.config/tianyan/tianyan.toml` → `~/.tianyan/tianyan.toml`
- 测试不依赖外部服务，用 `MockVectorStorage` 和 temp dir 隔离
- 所有错误用 `TianyanError`，禁止引入新错误类型

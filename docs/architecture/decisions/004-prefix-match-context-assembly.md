# ADR-004: 上下文组装前缀匹配原则

**日期**: 2026-06  
**状态**: ✅ 已采纳  
**影响范围**: ContextAssembler — 顺序不可变更

---

## 背景

每次 LLM 请求需要拼装 soul + rules + memories + history + current input。若这些内容的顺序不当，LLM Provider 的前缀匹配缓存将无法生效，导致每次请求都重新计算全部 Token，显著增加延迟和成本。

## 决策

`ContextAssembler::assemble()` 严格遵循 **固定前缀 + 可变后缀** 顺序：

```
soul → rules+memories → history(from compression_marker) → current input
```

### 设计理由

- **固定前缀（soul + rules + memories）**：会话期间不会变化。LLM Provider（OpenAI、Anthropic 等）利用前缀匹配缓存，固定前缀只计算一次，后续请求仅处理可变后缀。
- **可变后缀（history）**：对话历史随每轮交互增长。`compression_marker` 决定拼接起点——从最近 marker 之后的消息开始加载，marker 之前的旧消息不纳入本次输入（但保留在磁盘）。
- **绝对禁止** 把 soul/rules/memories 放在 history 之后——这会破坏前缀缓存，让每次请求都重新计算全部 Token。

## 后果

- soul 首次加载后缓存（`ContextPipeline` 中的 `cached_soul`）
- `compression_marker` 由 `ContextCompressor` 管理
- 顺序不可变更——变更会破坏 LLM 前缀缓存命中率

## 关键文件

- `core/src/context/assembler.rs` — `assemble()` 方法，顺序不可变更
- `core/src/context/pipeline.rs` — `ContextPipeline`（soul 缓存 + 压缩触发）
- `core/src/context/compression/` — `ContextCompressor`（生成 compression_marker）

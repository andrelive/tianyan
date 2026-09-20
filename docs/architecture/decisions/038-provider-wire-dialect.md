# ADR-038: provider wire 方言单点（方言即数据）

**日期**: 2026-09-20
**状态**: ✅ 已采纳（已实施）
**影响范围**: 配置（`core/src/config/model.rs`，新增 `CacheField` / `ThinkingParam` /
`EmbeddingUsageShape` / `DialectPreset` / `ProviderDialect`）、客户端
（`core/src/model/provider/client.rs`，新增 `dialect` + `model_thinking_params` 缓存）、
chat（`core/src/model/provider/chat.rs`，缓存字段与思考参数改读方言）、
embedding（`core/src/model/provider/embedding.rs`，绕开库类型化 + 宽容解析）、
配置示例（`config.example.toml`）

---

## 背景

所谓"OpenAI 兼容"各家细节差异很大，且这些差异此前**散落在 5 处、以不同方式处理**：

| 差异项 | 原处理位置 | 原处理方式 |
|---|---|---|
| 历史思考字段名 | `config/model.rs` + `chat.rs` | `ThinkingField` 枚举 + 嗅探（已有，形态正确） |
| 缓存命中字段 | `chat.rs::extract_cache_tokens` | **三个字段挨个试** |
| 思考强度参数形态 | `chat.rs::apply_thinking_params` | **按模型名含 `qwen` 硬编码嗅探** |
| 嵌入 usage 形状 | `embedding.rs` | **无处理**（依赖 async-openai 类型） |
| usage 整体缺失 | `agent/loop.rs` | 估算兜底 |

**事故（本次直接触发）**：DashScope `text-embedding-v4` 的兼容端点只返回
`usage.total_tokens`，而 async-openai 0.34 的 `EmbeddingUsage` 把 `prompt_tokens`
定义为**必填**字段 → 类型化反序列化在**库内部**失败（`missing field 'prompt_tokens'`），
**向量本身完全正常却被整次丢弃** → `search_vfs` 语义检索、知识库导入、L0/L1 摘要
向量化全线不可用。JSON 合法、错误位置（column 4 万+）仅因 serde 在文档解析终点报错。

## 决策

### 1. 方言即数据：`ProviderDialect` + `DialectPreset`

差异收敛为**单一描述符**，由 `ProviderConfig::resolve_dialect()` 在**客户端构造时
解析一次**并缓存（与既有 `ThinkingField` 形态同构），请求 / 响应路径**零判定**。

```rust
pub struct ProviderDialect {
    pub thinking_field: ThinkingField,        // 历史思考字段名
    pub cache_field: CacheField,              // 缓存命中首选字段
    pub thinking_param: Option<ThinkingParam>,// 思考参数形态（None = 模型名嗅探）
    pub embedding_usage: EmbeddingUsageShape, // 嵌入 usage 形状
}
```

`DialectPreset`（`openai_compatible` / `deepseek` / `dashscope` / `ollama` / `custom`）
是**预设参数表**：新增服务商命中已知预设 = 表里加一行；长尾用显式 `dialect` 或
模型级覆盖。**新增服务商不改请求路径代码。**

### 2. 差异分类的判据（三分法）

| 差异性质 | 判据 | 归属 |
|---|---|---|
| **协议级**（Anthropic Messages / Gemini） | 端点、消息结构、鉴权全不同 | 拆 `provider/` 子目录独立实现（沿用 `mod.rs` 既有约定） |
| **字段级**（本文全部差异项） | 同一 HTTP/JSON 骨架，仅字段名 / 参数名 / 字段存在性不同 | 方言数据（本 ADR） |
| **语义推断类**（上游错误文本分类、流式空响应识别） | 需要"猜"，不是"声明" | **不入表**，留在代码启发式（数据化会退化为"让用户描述错误文本格式"） |

### 3. 按"失败可观测性"分层（决定各项是"必须显式"还是"允许兜底"）

- **静默失败**（发错后服务端忽略，请求仍成功、上下文/思考静默受损）→ **必须显式可配**：
  `thinking_field`、`thinking_param`。
- **可见失败 / 无歧义**（探不到即 0、缺字段直接报错）→ **声明首选 + 容错兜底**：
  `cache_field`（保留全字段探测链）、`embedding_usage`（宽容解析 + 缺失兜底）。

该分层刻意保留兜底，以**消除新失败模式**：若"声明唯一"，用户配错 dialect 就会
静默丢缓存统计——那是把"可见失败"改造成"静默失败"，方向相反。

### 4. 两级粒度：provider preset + 模型级覆盖

思考参数形态是**模型级**差异（同一 DashScope 下 Qwen 思考族用 `thinking_budget`、
其它模型用 `reasoning_effort`），provider 级表达不了。故：
`ProviderConfig.dialect`（preset）+ `ModelEntry.thinking_param`（模型级覆盖），
解析单点 `AsyncOpenAIClient::thinking_param_for`：**模型级显式 > provider 方言 > 模型名嗅探**。

### 5. 不新增请求封装层

embedding 改为手写 `POST /embeddings`（与 chat 因 `reasoning_content` 手写同因），
但**复用同一套请求层单点**：同一个 `reqwest::Client`、同一组 headers、
同一 `RetryPolicy`（`with_retry`）、同一 `classify_http_status` 语义分类
（提升为 `pub(super)` 共享，不复制第二份）。

## 影响

- **修事故**：嵌入响应宽容解析（`prompt_tokens` 缺失以 `total_tokens` 兜底），
  另附**成因锁定测试**（断言库类型化路径在该形状上必失败——上游修复后该测试转红，
  提示可回归类型化）。
- **OCP 验收**：`test_ocp_hypothetical_provider_zero_code_adoption` —— 假想 provider
  （嗅探不到的自建网关 + 非标准 usage + 模型级参数覆盖）**零 Rust 改动**跑通。
- **已知边界**：`CacheField` 仅 3 种已知形态（长尾自动降级为 0，可见、不影响功能；
  出现新形态 = 加一个枚举值，1 行）。

## 验收

- 判别力：方言基线（阶段 0 锁定现状）→ 重构后原样通过；嵌入成因锁定测试；
  OCP 零代码接入测试。
- 全量：`cargo test -p tianyan-core --lib` 1372 绿、`-p tianyan-server` 绿、
  `cargo check --workspace --all-targets` + `cargo fmt --check` + `cargo clippy --workspace` 干净。

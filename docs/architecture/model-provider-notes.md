# 模型 / Provider 侧事实手册（model-provider-notes）

> **定位**：模型与 Provider 侧的**事实手册**——把排查中积累的高价值知识固化：reasoning 回传契约、思考强度/语言实测、前缀缓存口径、上游异常。
> **最后更新**：2026-09-12（对应 HEAD `afba701`，版本 0.3.15）
> **取证纪律**（本文所有陈述遵守）：
> 1. 每条陈述给出**代码位置（`文件:行`）或数据来源（`.scratch/` 文件名）**；
> 2. 实测数字标注**来源文件 + 条件**，并注明"会话实测/非实验室基准"；
> 3. 无法在仓库内核实的历史数字，写"来源不明，待复测"，不编造。
>
> ⚠️ **`.scratch/` 被 `.gitignore` 忽略且从未提交**（见 `.scratch/audit-reports/bt_6893a4e4.md` §4）。本文引用的 `.scratch/*.py` / `*.json` / `*.txt` 是**本地复现材料**，不在版本控制内——本文把它们承载的结论固化下来，脚本仅作复现线索。

---

## 1. Provider / 协议矩阵

### 1.1 协议：只有一套客户端（OpenAI 兼容 API）

| 事实 | 位置 |
|---|---|
| 天演**只实现 OpenAI 兼容 API**；若需非兼容 provider，须拆 `anthropic/` 等子目录（当前不做） | `core/src/model/provider/mod.rs:1-10` |
| 客户端 `AsyncOpenAIClient` 绑定**一个 provider**，同一实例服务 chat / embedding / vision 三种能力 | `core/src/model/provider/client.rs:9-46` |
| 能力以三个 trait 表达：`ChatService` / `EmbeddingService` / `VlmService`（+ 轻量 `ServiceDiscovery`） | `core/src/model/traits.rs:21-31, 80-95, 100+` |
| 请求体统一**序列化为 JSON 后再注入**方言字段（async-openai 0.34 类型缺 `reasoning_content`） | `core/src/model/provider/chat.rs:58-67, 174-182` |

**含义**：Claude / Qwen / Kimi 等都不是原生协议，而是当作 OpenAI 兼容端点接入（内置规格表里 `provider_prefix` 只用于**规格匹配**，不改变协议）。"协议差异"在代码中实际表现为**思考字段方言差异 + 少量参数差异**（见 §1.2）。

### 1.2 思考字段方言 `ThinkingField`（传输层字段名）

| 方言 | wire 字段名 | 适用 | 位置 |
|---|---|---|---|
| `ReasoningContent`（默认） | `reasoning_content` | DeepSeek 官方 / OpenAI 生态 | `core/src/config/model.rs:72-78, 84-87` |
| `Ollama` | `reasoning` | ollama OpenAI 兼容层 | `core/src/config/model.rs:79-87` |

- **解析优先级**：显式配置 `thinking_field` > `sniff(endpoint, name)` > 默认 `ReasoningContent`（`core/src/config/model.rs:146-149`）。
- `sniff` **只认** ollama 域名 / `:11434` / 名称含 `ollama`，**刻意不猜自建域名/代理**（`core/src/config/model.rs:96-104`）——嗅探不到的网关用 `thinking_field` 显式逃生（`core/src/config/model.rs:137-144`，ProviderConfig 字段）。
- **发错字段名不会报错**：Go 的 `encoding/json` 静默忽略未知字段，请求仍成功，但**思考内容根本没进 prompt**——多轮上下文与思考一致性悄悄受损（`core/src/config/model.rs:66-71`）。
- 客户端**构造时解析一次并缓存**，请求路径不再判断（`core/src/model/provider/client.rs:29-34, 84`）。

### 1.3 特性矩阵（按协议/字段维度）

| 维度 | DeepSeek 官方 | ollama（cloud / 兼容层） | 代码依据 |
|---|---|---|---|
| 读思考增量（stream delta） | `reasoning_content` | `reasoning` | serde `alias` 统一：`core/src/model/types/streaming.rs:75-76` |
| 写历史思考（请求回传） | `reasoning_content` | `reasoning` | `ThinkingField` + `inject_reasoning_content`（`chat.rs:660`） |
| 非流式缓存命中 | 只有 `prompt_tokens_details.cached_tokens`（顶层 `prompt_cache_hit_tokens` 被类型化丢弃） | 同 | `chat.rs:128-135` |
| 流式缓存命中 | **顶层** `prompt_cache_hit_tokens` **或** `prompt_tokens_details.cached_tokens` | `prompt_tokens_details.cached_tokens` | `extract_cache_tokens`（`chat.rs:581-599`） |
| usage（流式） | 完整（含 `completion_tokens_details.reasoning_tokens`） | **默认不返回**，须请求 `stream_options.include_usage` | `chat.rs:163-170`；缺 usage 时 TokenEstimator 兜底（`core/src/agent/loop.rs:576-604`） |
| tools | 支持（天演 agent **总是**携带 tools） | 支持 | `chat.rs:47-49` |
| 强度参数 | `reasoning_effort` | `reasoning_effort` | `apply_thinking_params`（`chat.rs:205-222`） |

### 1.4 上下文窗口来源（`ModelSpec.context_length`）

优先级：**显式条目 > 内置规格表 > 全局默认**，返回前经 `clamp_spec` 钳制。

| 来源 | 说明 | 位置 |
|---|---|---|
| `ModelSpec.context_length` | 总窗口（输入+输出共享） | `core/src/model/spec.rs:26-38` |
| `ModelEntry.context_length`（显式） | 配置项，最高优先 | `core/src/config/model.rs:236` |
| 内置规格表 `BUILTIN_SPECS` | 按 `provider_prefix + model_prefix` 三级匹配（精确 → 前缀 → 仅模型名兜底） | `core/src/model/spec.rs:96-197, 206-236` |
| 默认 | `context_length=32_768 / output=8_192 / input=8_192` | `core/src/model/spec.rs:41-49` |
| 解析单点 | `resolve_spec()` → `clamp_spec()` | `core/src/model/spec.rs:275-282, 286-303` |

内置规格示例（`core/src/model/spec.rs:96-197`）：`deepseek-v4-flash` 1M/32K、`deepseek-r1`（条目 `spec.rs:110-121`）128K、`kimi-k3` 1M/128K、`gpt-4o` 128K、`claude` 200K、`qwen3`/`qwen` 131K、`llama` 128K。

> **第三级兜底**（`core/src/model/spec.rs:222-233`）：provider 前缀不匹配时**仅按模型名前缀**匹配——网关 provider（如 `opencode`）挂载知名模型（`deepseek-v4-flash`）也能命中规格（`spec.rs:520-528` 测试锁定）。

### 1.5 思考强度档位声明（advisory）

- 档位由**模型自己声明**：`ModelEntry.reasoning_efforts: Option<Vec<String>>`（`core/src/config/model.rs:245-247`）；`None` = 不支持思考（对话中不显示档位选择）；`"off"` 为内置语义（不附加参数）。
- 内置目录默认档位为 `low/high/max`（`deepseek-v4-flash`、`kimi-k3`；`core/src/model/spec.rs:107, 133`）；`deepseek-r1` 固定深度思考、不支持档位（`spec.rs:112-121`）。
- 档位解析、`builtin_catalog` 等均为 **advisory**（仅展示/预填，不改变运行时行为，`core/src/model/spec.rs:247-260`）。

---

## 2. reasoning 回传契约（重点）

### 2.1 规则（DeepSeek 思考模型）

> **只要请求带 `tools`，就必须全量回传历史 `reasoning_content`**（含**未实际调用工具**的轮次）；不带 tools 时可不回传。

- 依据：DeepSeek `thinking_mode` 官方文档 + 代码注释（`core/src/model/provider/chat.rs:649-660`、`core/src/context/assembler.rs:126-129`、`core/src/common/token_estimator.rs:40-43`）。
- **天演作为 agent 总是携带 tools** → 该契约**恒常适用**（`core/src/context/assembler.rs:127-128`、`core/src/common/token_estimator.rs:41-42`）。

### 2.2 契约实现链（`Part::Reasoning` 的组装与回传路径）

| 阶段 | 动作 | 位置 |
|---|---|---|
| ① 流式累积 | `delta.reasoning_content` 逐段累积进 `accumulated_reasoning`，同时 `send_thought` 推送前端 | `core/src/agent/loop.rs:470-478` |
| ② 组装 assistant | 累积结果写入 `Message.reasoning_content`（空则 `None`） | `core/src/agent/loop.rs:569-573` |
| ③ 持久化（Message→Structured） | `reasoning_content` 非空 → `Part::Reasoning`（排在 text 之前） | `core/src/common/types/structured_message.rs:278-285` |
| ④ 组装（Structured→Message） | `Part::Reasoning` → `reasoning_content`，**无条件保留**（不按 `has_tool_calls` 过滤） | `core/src/context/assembler.rs:115, 124-129, 165` |
| ⑤ 发送（JSON 注入） | `convert_messages` 类型化构造后，序列化体由 `inject_reasoning_content` 按索引回填 wire 字段（非流式 + 流式都调） | `core/src/model/provider/chat.rs:660-676`；调用点 `chat.rs:66, 182` |
| ⑥ 估算 | `TokenEstimator.estimate_message` 计入 `reasoning_content`（否则占用被低估） | `core/src/common/token_estimator.rs:44-48` |

**回填为什么必要**：`convert_messages` 用 async-openai 0.34 的 `ChatCompletionRequestAssistantMessage`（**无 `reasoning_content` 字段**，`..Default::default()` 构造），类型化序列化会**静默丢弃**思考内容；故必须序列化后按索引回填（`core/src/model/provider/chat.rs:649-660`）。`convert_messages` 对每条 `Message` 恰好生成一条请求消息 → 索引一一对应（`chat.rs:664-666` 注释）。

**回归测试锁定**（`core/src/model/provider/chat.rs`）：
- `test_inject_reasoning_content_roundtrip`（`chat.rs:876`）——assistant 的 reasoning 必须回传；
- `test_inject_reasoning_content_ollama_dialect`（`chat.rs:948`）——ollama 方言写 `reasoning`，且**不得双写** `reasoning_content`（`chat.rs:966-967`）；
- `test_inject_reasoning_content_dialects_are_exclusive`（`chat.rs:973`）；
- `test_structured_to_messages_reasoning_without_tool_call_is_kept`（`core/src/context/assembler.rs:374`）——无工具调用轮也保留。

### 2.3 历史结论（已否定方向，勿重复讨论）

| 方向 | 结论 | 依据 |
|---|---|---|
| "只对带 `tool_calls` 的消息回传思考内容" | **否定**。`assembler` 已**去掉 `has_tool_calls` 条件**，改为无条件保留 reasoning（回归测试锁定无工具调用轮也保留）。收益≈1%（**会话结论；仓库内无对应数据文件 → 来源不明，待复测**） | `core/src/context/assembler.rs:126-129`；`docs/release/RELEASE_NOTES.md`（0.2.x 条目"`assembler` 去掉 `has_tool_calls` 条件无条件保留 reasoning"） |
| "裁剪思考内容（省略历史 reasoning）" | **否定**。契约优先、不裁剪——带 tools 时省略违反官方规则；且字段名/轮次覆盖是"静默失效"风险（发错字段或漏轮次都不会报错，只静默丢上下文）。安全性由探针验证（见下） | `core/src/config/model.rs:66-71`；探针 `.scratch/probe-strip-safety.py`（K1~K5）、`.scratch/probe-strip-safety2.py`（C1/C2/C4） |

**裁剪安全性探针**（`.scratch/probe-strip-safety.py` / `probe-strip-safety2.py`，只读本地结果即可，**不要重跑**）：
- K1~K5 用例：含/省略 thinking × 有/无 tool_calls × 带/不带 tools（脚本 docstring，`probe-strip-safety.py:1-11`）；
- C1/C2/C4 用例（`probe-strip-safety2.py`）：C2 = "省略 thinking、有 tool_calls"（文档警告 400 的最可能触发形态）；C4 = "裁剪后真实历史形态"（旧工具轮含思考 + 末答省略思考 + 新 user）。
- 结论已被**代码注释固化**：`assembler.rs` 无条件回传（见 §2.2 ④）。

---

## 3. 思考强度 × 语言（会话实测）

> ⚠️ **本节数据为会话实测值，非实验室基准**。原始记录：`.scratch/lang-effort-results.json`（逐调用指标）、探针 `.scratch/probe-lang-effort.py`（Part1 tokenizer 前提 / Part2 强度矩阵 / Part3 语言约束 V1 软 vs V2 硬）、`.scratch/probe-anchor.py`（英文历史抗锚定对照）。模型：`deepseek-v4.1-flash`（ollama 网关）。

### 3.1 强度参数映射（代码事实）

`apply_thinking_params(body, effort, model)`（`core/src/model/provider/chat.rs:205-222`）：

| 模型族 | 附加参数 | 值 |
|---|---|---|
| `effort == "off"` | **不进入本函数**，不附加任何参数（模型默认行为） | — |
| 模型名含 `qwen` | `enable_thinking: true` + `thinking_budget` | `low=1024 / medium=4096 / high=16384 / 其他（含 max）=4096` |
| 其余 OpenAI 兼容族（DeepSeek 等） | `enable_thinking: true` + `reasoning_effort` | **原档位值原样透传** |

档位为模型自己声明的集合（`core/src/config/model.rs:245-247`），不做本地翻译；未知档位在 qwen 族用默认 `4096`。DeepSeek 等已实测容忍附加 `reasoning_effort`（`chat.rs:196-203` 注释）。

### 3.2 tokenizer 前提：中文比英文省 token

来源：`.scratch/lang-effort-results.json` 的 `part: "tokenizer"` 两条。

| 同义文本 | 字符数 | `prompt_tokens` | chars/token |
|---|---|---|---|
| 中文（CN） | 116 | 90 | ≈1.29 |
| 英文（EN） | 418 | 104 | ≈4.02 |

→ **同义内容中文比英文省 ~13% token**（90 vs 104）。与 `RELEASE_NOTES.md` 0.3.15 条目一致（"同义内容中文比英文省 ~13% token（思考占上下文 ~40% → 增速约 -5%）"）。

### 3.3 强度矩阵：思考量级（会话实测）

| 档位 | 思考消耗量级（tokens，会话实测折算） | JSON 内原始 `reasoning_chars` 样本 |
|---|---|---|
| `low` | ≈550 | T1/low/r1=1681、T1/low/r2=2640、T2/low/r1=2668、T2/low/r2=1838 |
| `high` | ≈940 | T1/high/r1=2952、T1/high/r2=2451、T2/high/r1=8387、T2/high/r2=1247 |
| `max` | ≈4200+ | T1/max/r1=**29434**、T1/max/r2=19198、T2/max/r1=13249、T2/max/r2=4965 |

**口径与可信度**：
- 上表"tokens 量级"为**本次会话记录的折算/估计值**（JSON 只存 `reasoning_chars` 与 `reasoning_cjk`，未存逐条 `completion_tokens`——见 `lang-effort-results.json` 中 `completion_tokens: null`）。**精确 token 待复测**。
- `max` 档常见 `finish: "length"`（思考吃满输出预算，`lang-effort-results.json` 中 T1/max 两条均为 `finish=length`、`content_chars=0`）——即"想完没说话"的主要形态。
- `reasoning_cjk` 在无约束时波动大（0.0 ~ 0.588），说明**未约束时思考语言不受控**（`lang-effort-results.json` 全量 `reasoning_cjk`）。

### 3.4 soul 中文约束（原文引用）

已写入产品默认 soul（`core/src/agent/default_soul.md:48-50`，末节"## 思考语言"）：

```text
## 思考语言

你的内部思考（reasoning）必须始终使用中文——即使对话历史、工具输出或代码是英文；仅代码片段、报错原文与专有名词可保留原文。
```

> 该段与 `probe-anchor.py` 中的 **V3 "覆盖条款"** 版本一致（含"即使对话历史、工具输出或代码是英文"的抗锚定条款）；`probe-lang-effort.py` 的 V1（软）/V2（硬）为对照变体。`DEFAULT_SOUL` 通过 `include_str!("default_soul.md")` 加载（`core/src/agent/mod.rs`）。

### 3.5 约束有效性（会话实测）

| 指标 | 值 | 来源 |
|---|---|---|
| 切中文成功率 | **12/12**（含英文历史 + 英文工具输出的抗锚定场景） | `RELEASE_NOTES.md` 0.3.15 条目；`.scratch/probe-anchor.py`（V3 覆盖条款下仍切中文，`.scratch/reason-anchor-V3-覆盖条款.txt`） |
| 生效时机 | **需重启应用生效**：新会话立即；已有会话下次压缩点刷新 | `RELEASE_NOTES.md` 0.3.15 |

---

## 4. 前缀缓存

### 4.1 语义与数据结构

| 项 | 定义 | 位置 |
|---|---|---|
| `CacheUsage.read` | 缓存**命中** token 数 | `core/src/common/types/structured_message.rs:31-38` |
| `CacheUsage.write` | 缓存**写入** token 数 | 同上 |
| 落库 | `tokens.cache.read` 随 `content_parts`（StructuredMessage JSON）持久化 | `docs/architecture/context-pipeline.md` §6.2 |

### 4.2 **关键口径（单点，务必记住）**

```rust
// core/src/common/types/structured_message.rs:239
pub fn prompt_side_tokens(&self) -> usize {
    self.tokens.input.max(self.tokens.cache.read)
}
```

- `tokens.input` 是**完整输入**（**含**缓存命中部分，`cache.read` 是其**子集**）→ **取 `input`**；`max(cache.read)` 仅兜底旧数据 `input=0` 的异常口径。
- **严禁 `input + cache.read`**：两者相加把缓存命中**重复计一遍**（判定值 ≈ 真实值 × 2）。
- 该口径**单点共用**于：压缩判定（`agent_core` 的 `maybe_compress_and_persist`）与实测输入恢复（`core/src/agent/loop.rs:620-651`）。

### 4.3 历史事故："上下文预算不足 1024" 误报

- **现象**：压缩后继续对话直接报错，且该会话此后每条消息必现（会话卡死）。来源：`docs/operations/troubleshooting.md` §2.c、`RELEASE_NOTES.md` 0.3.14。
- **根因（双叠）**：
  1. 实测输入恢复用 `tokens.input + cache.read` → double count。**事故实测**：`input=593_399 / cache_read=592_128` → 判定 `1_185_527` 直接越过 1M 窗口 → `dynamic_max_tokens` 返回 `None`（`core/src/model/spec.rs:344`，`MIN_MAX_TOKENS=1024`）→ 请求不发出；
  2. 恢复范围未按压缩点截断（压缩后组装视图实际仅 ~1.7 万）。
- **修复**（0.3.14）：统一改用 `prompt_side_tokens()` 单点 + 只取最后一个压缩点之后的实测。回归测试用事故真实数字锁定（`input=593_399`）。

### 4.4 展示口径（前端）

| 口径 | 公式 | 位置 |
|---|---|---|
| 上下文占用百分比 | `prompt_tokens / context_window` | `gui-vite/src/components/chat/ContextRing.tsx:39` |
| 缓存命中率 | `cache_read / prompt_tokens` | `gui-vite/src/components/chat/ContextRing.tsx:41-43` |

> **重要边界**：缓存命中部分（`cache_read`）**算窗口占用**（它仍占窗口），**不得**用 `input - cache_read`（"未命中"）当上下文占用——那是**成本口径**，不是**窗口占用口径**（`docs/architecture/context-pipeline.md` §6.1）。

### 4.5 前缀稳定性（命中的前提）

命中依赖前缀**逐字节稳定**：组装顺序固定（soul → project_instructions → rules+memories → history）**不可变更**（ADR-004；`docs/architecture/context-pipeline.md` §1）。会话内**不刷新**注入内容（ADR-012），刷新点 = 启动全量 / 新会话增量 / **压缩点**（会话内唯一免费刷新点）——会话内任何前缀抖动都会使缓存失效。

---

## 5. 嵌入调用（用量入账与查询缓存）

> 本节记录 0.3.16 的一次实测排查：**嵌入一直在被调用，但此前完全不入账**，
> 导致"账单里看不到嵌入费用 → 以为没在用"的误判。

### 5.1 嵌入在哪被调用（实证）

| 调用点 | 触发 | 位置 |
|---|---|---|
| 语义检索（query 侧） | 每次 `search_vfs` → `VfsSearch::search` | `core/src/vfs/vfs_impl.rs`（embed query 后查 LanceDB 两列 RRF） |
| 内容写入（摘要侧） | 摘要生成/更新 → `index_entry` | 同上（`abstract` / `overview` 各嵌一次） |
| 图像分析 / 角色路由 | 图像理解、`suggest_role` | `knowledge/image/analyzer.rs`、`agent/role_router.rs` |

日志证据（`%APPDATA%\com.tianyan.app\logs\`，时间戳为 **UTC**）：

```
INFO ... retrieve{top_k=10 query=...}: tianyan::model::provider::middleware:
  embed_single_with_dimensions called model=text-embedding-v4 dimensions=2048 text_len=132
INFO ... embed_single_with_dimensions succeeded model=text-embedding-v4
```

### 5.2 为什么"看不到费用"（三个原因，按可能性排序）

1. **账单口径**：若 `endpoint` 是百炼 **MaaS 专属实例**域名
   （`llm-xxxx.<region>.maas.aliyuncs.com`），费用通常按**实例/算力时长**计，
   不出现在"模型调用（token）"账单条目里；公共 `dashscope.aliyuncs.com` 才按 token 计。
2. **用量极小**：单次只是短 query（几十~百余字符）或摘要文本；数百条内容 +
   千余次检索，按公共价量级也在"元"以下。
3. **应用内不入账（0.3.16 已修）**：嵌入不走 AgentLoop，其 token 此前**不写
   `usage_logs`**——`usage_logs` 里只有 chat provider，统计面板自然看不到。

### 5.3 0.3.16 的两项改动

| 改动 | 内容 | 位置 |
|---|---|---|
| **用量入账** | `EmbeddingUsageSink` 契约（model 层定义，装配层注入实现）→ `UsageLogEmbeddingSink` 把 `provider/model/prompt_tokens` 异步写入 `usage_logs`（`session_id=""`） | `core/src/model/provider/middleware.rs`、`server/src/embedding_usage.rs` |
| **查询缓存 + 空串短路** | 键 `model\|dimensions\|text`，容量 256（FIFO）；命中不发请求、不重复上报。空/空白文本直接报错（日志曾出现 `text_len=0` 的调用）。VFS 侧空 query 直接返回空结果、双摘要皆空则跳过索引 | 同上、`core/src/vfs/vfs_impl.rs` |

- 装配点：VFS 实例（`server/src/lib.rs` 的 `initialize_vfs_for_app`）与 Agent 实例
  （`server/src/state.rs` 构造 + 配置热重载两处）**都注入**——两个实例各自持有缓存。
- 验证方式（升级后）：任意检索一次 → `usage_logs` 应出现
  `provider=<你的嵌入 provider>` / `model=text-embedding-v4` 的记录；
  同 query 立刻再检索一次 → **不再新增记录**（缓存命中）。

### 5.4 与账单核对的对应关系

| 你要看的东西 | 看哪里 |
|---|---|
| 应用内嵌入消耗 | `usage_logs`（按 provider/model 聚合；`db::UsageRepo::stats`） |
| 云侧账单 | 控制台「模型调用」条目（公共端点）；专属实例看部署/算力费用 |
| 是否真的在调用 | 文件日志的 `embed_* called/succeeded` 行（含 model 与 text_len） |

---

## 6. 上游异常

### 5.1 `finish_reason: "load"` + 空 assistant（0 token）

**现象**：会话里落一条**空 assistant 消息**（`parts=[]`、`tokens=0`、`finish="load"`）；前端看不到，需退出重进才可能察觉。来源：`docs/operations/troubleshooting.md` §2.a、`.scratch/wake-no-user-finding.md`（0.3.10 现场定位）。

**机制（四步）**：
1. 压缩点恰好落在通知前 → 组装视图 = "摘要(system) + 通知(system) + 唤醒指令(system)"，**没有任何 user 消息**（`core/src/context/assembler.rs` 从最后一个压缩点开始组装）；
2. **Ollama 云对"无 user 消息"的请求返回 `finish_reason="load"` + 空内容、0 token**（直连实测 4 组对照：**无 user = load；补 1 条 user 正常生成**）；
3. loop 对空响应重试 1 次（相同上下文 → 相同 load），`process_wake` 把空输出当"无需回复"静默完成；
4. 历史同款：9/10 的 `49ee` 会话 5 条 load 空消息；本会话 `seq=248`。

**取证思路**（`troubleshooting.md` §2.a / §3.3）：查 `session_messages` 中 `tokens` 全零、`finish='load'`、`bytes` 极小的 assistant 消息；对应用量行在 `usage_logs`（应为全 0）：

```sql
SELECT seq, role, tokens, json_extract(content_parts,'$.finish') AS finish,
       length(content_parts) AS bytes
FROM session_messages
WHERE session_id = '<会话 id>' ORDER BY seq DESC LIMIT 8;

SELECT * FROM usage_logs WHERE session_id = '<会话 id>' ORDER BY id DESC LIMIT 5;
```

**判定**：空输出消息恰好紧跟一个压缩点（`content_parts LIKE '%"compression_marker":true%'`），且前后组装视图缺 user 角色锚点 → 命中本故障。

**处置**：
- **主修（0.3.13 已落地）**：压缩摘要改为 **user 角色锚定**（组装层按 `compression_marker` 统一归一，旧数据 system→user）；
- **加固**：`finish=load` / 完全空响应**不再静默** → 异常化（走唤醒重试，耗尽后前端可见错误）；
- 临时规避：给该会话手动发一条 user 消息再触发唤醒轮。

### 5.2 ollama cloud 缓存命中实测（2026-09-11 会话）

来源：`.scratch/v41-run1.txt` / `v41-run2.txt`（模型 `deepseek-v4.1-flash`）、`.scratch/v4f-run1.txt` / `v4f-run2.txt`（模型 `deepseek-v4-flash:0731`）；请求体 `.scratch/verify-long-v41.json` / `verify-long-v4f.json`（固定长前缀 system + 一次短 user，`stream_options.include_usage`）。

| 模型 | 冷启动（run1） | 热请求（run2） | 命中率 |
|---|---|---|---|
| `deepseek-v4.1-flash` (v41) | `prompt_tokens=1315`，**无** `cached_tokens` 字段 | `prompt_tokens=1315`，`cached_tokens=1280` | **1280/1315 ≈ 97.3%** |
| `deepseek-v4-flash:0731` (v4f) | `prompt_tokens=1289`，`cached_tokens=0` | `prompt_tokens=1289`，`cached_tokens=1288` | **1288/1289 ≈ 99.9%** |

- **结论**：1315 token prompt 的**热请求命中 97%–99.9%**；**冷启动存在差异**（首次无缓存/部分命中）。
- **判定**：这是**上游/网关侧**的缓存行为（前缀命中由服务端决定），**非 agent 缺陷**。
- **旁证**：DeepSeek 官方响应用顶层 `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens`（`.scratch/verify-resp-deepseek.txt:33`）；ollama 用 `prompt_tokens_details.cached_tokens`（`.scratch/v41-run2.txt`）——**两种形态已由 `extract_cache_tokens` 统一**（`core/src/model/provider/chat.rs:581-599`）。

### 5.3 其他上游异常形态

| 形态 | 说明 | 位置 |
|---|---|---|
| reasoning-only（有思考无正文） | ollama 对"system 完成通知"只输出思考不输出正文是**系统性**行为（相同上下文重试结果相同）——已**移除整轮重试**，保留 loop 内部空响应单次重试 | `RELEASE_NOTES.md` 0.3.9 条目；`core/src/agent/agent_core.rs:221-224, 1093-1107` |
| 非标准 `finish_reason` | 网关 `network_error` 等非标准值在 provider 层**显式上抛为错误**而非静默丢弃；`finish_reason` 随 assistant **持久化**到 `StructuredMessage.finish` | `docs/architecture/context-pipeline.md` §7.3 |
| 流式中断 | 已有内容累积 → 保留为截断输出（`finish="interrupted"`）；完全无内容 → 按失败上报 | `core/src/agent/loop.rs:516-556` |
| 上游错误分类 | 语义谓词（ADR-014）：`invalid_api_key`/`unauthorized` → permission、`model_not_found` → not_found、超时 → timeout；HTTP 401/403/404/408/504 各自语义分类 | `core/src/model/provider/chat.rs:236-256, 263-275`；`docs/architecture/decisions/014-error-classification.md` |

---

## 7. 复现脚本与数据来源清单

> 全部位于 `.scratch/`（**本地、未提交**）。复现时**只读既有结果文件**；如需重跑探针，注意它们会**发起真实 LLM 请求**。

| 文件 | 用途 | 承载结论 | 可信度 |
|---|---|---|---|
| `probe-lang-effort.py` | 思考强度×语言实测（Part1 tokenizer / Part2 low·high·max 矩阵 / Part3 V1 软·V2 硬） | §3.2 / §3.3 | 会话实测，可复现 |
| `lang-effort-results.json` | 上述探针的逐调用结果（`reasoning_chars` / `reasoning_cjk` / `finish` / `elapsed`） | §3.2 / §3.3 | 原始记录（**未存 token**） |
| `probe-anchor.py` + `reason-anchor-V3-覆盖条款.txt` | 英文历史 + 英文工具输出下的抗锚定对照（V2 硬性 vs V3 覆盖条款） | §3.5（12/12 切中文） | 会话实测 |
| `probe-reason-text.py` | 抓思考原文（V2 强约束 vs 基线），判断语言是否真切换 | §3.4 / §3.5 旁证 | 会话实测 |
| `probe-strip-safety.py`（K1~K5）、`probe-strip-safety2.py`（C1/C2/C4） | 思考回传"裁剪安全性"——省略历史 reasoning 是否被 400 拒 | §2.3 | 探针脚本（结果片段已入代码注释） |
| `verify-body-deepseek.json` / `verify-body-ollama.json` | 最小请求体（`stream_options.include_usage`） | §1.3 usage 形态 | 请求体快照 |
| `verify-resp-deepseek.txt` / `verify-resp-ollama.txt` | 单轮响应原文（字段名差异） | §1.3 / §5.2 | 响应快照 |
| `verify-long-v41.json` / `verify-long-v4f.json` + `v41-run1/2.txt`、`v4f-run1/2.txt` | 前缀缓存命中实测（固定长前缀 + 冷/热两次） | §5.2 | 会话实测（2026-09-11） |
| `probe-ollama-reasoning-field*.py` | ollama `reasoning` vs `reasoning_content` 字段名探针（阳性对照：塞进 content 必涨 `prompt_tokens`） | §1.2 / §2.2 | 探针脚本 |
| `wake-no-user-finding.md` | 唤醒轮"无 user → `finish_reason:load` → 静默"四步故障链 | §5.1 | 现场定位记录 |

### 未能核实 / 待复测

| 项 | 状态 |
|---|---|
| "只对带 `tool_calls` 的消息回传思考内容"的**收益≈1%** | **来源不明，待复测**——仓库内无对应数据文件；可核实的只有"该方向已被否定 + 代码无条件回传"（§2.3） |
| `low≈550 / high≈940 / max≈4200+` **tokens** | 会话实测**折算值**；`lang-effort-results.json` 只存 `reasoning_chars`，未存逐条 `completion_tokens` → **精确 token 待复测**（§3.3） |

---

## 8. 相关文档

- `docs/architecture/context-pipeline.md` — 上下文组装顺序 + 压缩机制 + 构成量化方法 + 故障模式
- `docs/architecture/decisions/014-error-classification.md` — 错误分类语义谓词（KIND 前缀 + 构造器/谓词）
- `docs/release/RELEASE_NOTES.md` — 0.2.x（reasoning 回传 + 缓存）、0.3.9（ollama 思考字段回写）、0.3.14（预算误报）、0.3.15（思考语言中文约束）
- `docs/operations/troubleshooting.md` — 日志/DB 取证 + `finish_reason:"load"` 排查路径

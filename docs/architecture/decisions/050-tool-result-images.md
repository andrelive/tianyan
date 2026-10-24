# ADR-050: 工具结果携带图片（`view_image` 直通）

**日期**: 2026-10-24
**状态**: ✅ 已采纳（已实施）
**影响范围**: 传输层（`plugins/provider-llm/src/chat.rs` 的 `content_value` 与 Tool 分支）、
组装层（`core/src/context/tool_images.rs` 新增 + `assembler.rs` 的 Tool 分支）、
消息类型（`Message::tool_with_images`）、工具（`core/src/executor/image.rs` 冻结副本 +
`plugins/tools-fs/src/view_image.rs`）、装配（`server/src/agent_builder.rs` 注入 `data_dir`）、
展示（`ToolPresentation::Image` + 前端图标）

---

## 背景

1. **模型看不见磁盘上的图片。** `read_file` 只读文本（`sniff_binary` 拦截二进制），
   所以「文件夹里的截图 / 用户给的图片文件」在对话里是不存在的——模型只知道路径。
   MCP 截图（playwright）同样只落盘 `{data_dir}/mcp_images/`，模型拿不到内容，
   于是「改完 UI 自己截图自验证」这条闭环一直断着。
2. **两类图片必须分开看**（2026-10-24 结论，勿混）：

   | | 来源 | 存储 | 回放 |
   |---|---|---|---|
   | ① 用户从会话框贴图 | 用户输入 | `Part::Image { url: data:... }` 落 `session_messages` | ✅ 每轮组装重新注入 |
   | ② 工具读文件得到的图 | 模型调 `view_image` | tool 结果存**引用**，字节不进库 | ✅ 组装期按引用加载 |

   ① 是既有链路（base64 落库、每轮回放），本 ADR **一行不动**。
3. **协议事实（实测，非推断）**：OpenAI 兼容协议里 `role: "tool"` 的 `content`
   可以是**内容块数组**。2026-10-24 用同一张 `HELLO 42` 探针图对三家真实上游
   各发两次（图放 user / 图放 tool），**12 个组合全部成功**：

   | Provider | 模型 | 图放 user | 图放 tool |
   |---|---|---|---|
   | ollama（cloud 订阅） | `deepseek-v4.1-flash` / `:1017` / `glm-5.3-flash` | ✅ | ✅ |
   | opencode（go 订阅） | `deepseek-v4.1-flash` / `glm-5.3-flash` | ✅ | ✅ |
   | deepseek（官方） | `deepseek-flash` | ✅ | ✅ |

   tool 路径的回答带具体视觉细节（"白底上黑色粗体大写字母"），不是幻觉。
   这一条**推翻了两个预判**：①「tool 带图是非标准扩展、只有 DeepSeek 官方支持」
   ——三家全支持；②「reroute 到末尾 user 消息是唯一通用路径」——直通可用。
4. **此前挡路的不是传输层，是类型层**：天演的 HTTP/SSE/重试/双预算/UA/动态头
   早已自研，`async-openai` 只承担 JSON 构造 + 类型检查；它的
   `ChatCompletionRequestToolMessageContentPart` 只有 `Text` 一个变体，
   把 tool 带图**在编译期堵死**。ADR-049 期 2 序 6 之后请求体已是纯 `Value`
   构造，唯一阻塞项消失——本 ADR 是那笔投资的兑现。

## 决策

### 1. 传输层：tool 消息复用内容块数组（`content_value`）

`convert_messages` 的 Tool 分支不再写死字符串，而是与 User 分支共用同一份
`content_value(m)`：

- `content_parts` 为 `None`/空 → `content` 为**字符串**（与改造前逐字节一致，
  纯文本工具结果零变化）；
- `content_parts` 非空 → `content` 为 `[{type:"text"},{type:"image_url"},…]`。

`user_content` 因此改名为 `content_value`（它的语义从来不是"用户"，
而是"一条消息的 content 值"）；两个分支共用一份编码/档位过滤逻辑，不复制。

### 2. 工具：`view_image`（读文件 → 冻结副本 → 返回引用）

```
view_image(path)
  ├─ resolve_tool_path（会话工作目录语义，与 read_file 同一口径）
  ├─ security_policy.check_path（路径沙箱）
  ├─ metadata 大小守卫：> 5 MB → 报错（不进入下游）
  ├─ 读字节 → sniff_image_mime（magic bytes，png/jpeg/gif/bmp/webp）
  │    └ 非图片 → 报错（**不靠扩展名**，与 knowledge 的宽容推断刻意相反：
  │      那里未知回退 image/png，这里未知必须拒绝）
  ├─ sha256(字节) → {data_dir}/session_images/{sha256}.{ext}
  │    └ 已存在则跳过写入（幂等；同图多次读取不产生第二份副本）
  └─ 返回 {"image":"session_images/<sha>.png","sha256":…,"mime":…,"bytes":…,"path":<原路径>}
```

**为什么是冻结副本**（与 ADR-047 spill 同构：大对象落受管目录，会话只留引用）：

| 风险 | 冻结副本后 |
|---|---|
| 原图被移动/改名/删除 | ✅ 副本在，后续回放照常 |
| 原图被**覆盖成另一张** | ✅ 副本不变；sha256 不同 = 另一个条目，语义可复现 |
| 前缀缓存 | ✅ 注入内容逐字节恒定，命中到注入点为止 |
| 库膨胀 | ✅ 字节不进 SQLite（`session_images/` 是运行期产物，与快照/spill 同类例外） |
| 幂等 | ✅ 同一 sha256 只存一份，重复读同一图零成本 |

### 3. 组装层：引用 → data URL（`SessionImages`）

`ContextAssembler` 新增 `assemble_with(…, images: Option<&SessionImages>)`，
Tool 分支对每条 `Part::ToolResult` 调 `SessionImages::resolve(content)`：

- 解析 tool 结果 JSON 的 `image`（字符串）/ `images`（字符串数组）字段；
- 引用必须形如 `session_images/<文件名>`（**只认这一种前缀**，文件名里禁止
  `/`、`\`、`..`）——引用来自库（可能被注入污染），路径逃逸在这里**结构上**
  不可能，而不是靠事后校验；
- 文件缺失 / 超 5 MB / magic 不符 → **降级为纯文本**（best effort，同 ADR-047）；
- 存在 → `data:{mime};base64,…`，与 tool 原文一起构成
  `Message::tool_with_images`（`content_parts = [text(原文), image(…)]`）。

`assemble()`（无图版本）保留并委托给 `assemble_with(…, None)`——core 内 8 处
单测与不知道 `data_dir` 的调用点零改动。

### 4. 明确不做（每条都有理由，不是省事）

| 不做 | 理由 |
|---|---|
| **VLM 转述**（工具内部调视觉模型，返回文字描述） | 实测直通可用后它只剩缺点：多一次模型调用（延迟 + 费用）、描述有损（细节取决于转述模型）、且要求额外配置 `preferences.vision`。**保留为将来"主模型确无视觉能力"时的可选路径**，不是现在 |
| **reroute 到末尾 user 消息** | 直通不需要它。它的问题：历史出现断档（模型看到"我读过图"却看不到那条 user 消息）、打断 tool 块序列（严格上游 400）、图片块每轮重算 |
| **图片字节落 SQLite** | 库已 600 MB+；引用式足够（冻结副本保证语义），代价只是"数据目录被手动清空后历史图失效"（降级为纯文本，不报错） |
| **保活最近 K 张** | 图**就在** tool 结果里，历史每次都带——"保活"这个概念在直通方案里不存在 |
| **按 provider 能力探测降级** | 三家实测全通；探测需要额外的试探请求，成本高于收益。失败形态是**可见的**上游 400（ADR-038 可观测性分层），不引入静默降级 |
| **`session_images/` 的 GC** | 首版不做。目录只增不减，但增长速率 = 用户实际看图数（每张一份，同图去重）。后续可复用 tool_output 的 GC 策略（按会话/时间），届时单独决策 |

## 后果

### 正面

1. **任何支持视觉的模型都能"看"本机图片**：`view_image` 之后模型直接看到像素，
   不需要额外模型、不需要用户转录。
2. **MCP 截图闭环打通**（本次未接线，机制已就位）：模型对截图路径调
   `view_image` 即可自验证 UI，把"人肉验收"换成"模型自查"。
3. **历史自洽**：tool 结果与图片是**同一条消息**，不存在"结论在历史、依据不在"
   的断档；模型后续要细节可以再调一次工具（幂等，读同一份副本）。

### 代价（诚实记账）

1. **每轮重传**：带图的 tool 结果在历史里每轮都会被重新编码并上传。张数少时
   可忽略；长会话里图多则显著（图片 token 本来就贵）。缓解路径（不在本 ADR）：
   按 seq 只内联最近 N 条、或按会话图片预算剥离旧图——**但任何剥离都必须
   落在压缩点旁边**（改历史 = 前缀分叉，ADR-048 的教训）。
2. **前缀稳定性依赖"字节恒定"**：同一 sha256 副本 → 同一 base64 → 同一前缀。
   原图被覆盖不影响（副本冻结）；**不要**给注入块加时间戳/路径/随机量。
3. **视觉能力仍是用户手动声明**（`ModelCapability::Vision`）：本机制不校验
   "该模型是否真能看图"，发错模型的表现是上游 400。这是既有缺口，非本 ADR 引入。
4. **数据目录被清空** → 历史图片降级为纯文本（引用还在，字节没了）。可接受，
   但**不要**把 `session_images/` 当成可随意清理的缓存：它和 `tool_output/`
   同级（运行期产物，用户可删但会丢回放能力）。

## 实施期发现（两条，均推翻了一个隐含前提）

### 1. 「组装期加载」不够：工具结果必须在**本轮内**补图

设计时的隐含假设是「每轮请求都会重新组装历史」——错的。agent loop 在**一轮内**
直接复用消息 Vec（`ctx.messages.push(Message::tool_result(...))`，`loop.rs`），
工具结果 `push` 后立刻作为下一次请求，**不经过** `ContextAssembler`。

后果（实测可见）：模型拿到 tool 结果原文（引用 JSON），但**看不到图**，于是
开始用 `execute_command` + Python/PIL 逐像素"猜"图里写了什么（日志实录：
`python -c "from PIL import Image; ..."`、ASCII art 打印）。功能等于没做。

修法：`SessionImages::hydrate` 在**入 Vec 之前**就给 tool 消息附上图片片段
（`loop.rs::session_images()`）。形状由 `Message::attach_images` 单点保证——
本轮内 hydrate 与下一轮 `assemble` 的产物**逐字节一致**，否则同一段历史在
「本轮」与「下一轮」之间会产生前缀分叉（这正是 ADR-035/048 反复强调的那类分叉）。
单测 `test_hydrate_attaches_image_to_tool_message` 直接断言两者序列化相等。

**纪律**：任何"请求视图上的派生注入"都必须同时覆盖两条路径——组装层（跨轮）
与循环层（轮内）。只做前者，用户看到的是模型"装作看过图"。

### 2. ollama 在 HTTP/2 下对「大请求体 + SSE + tool 图」不返回任何响应头

实测（2026-10-24，隔离实例）：第二轮请求（32 个工具定义 + tool 图，49KB）
发出后 reqwest **等 400s 无响应头、无错误、无重试日志**；同一份请求体
（逐字节落盘后重放）用 h1.1 只花 1.5–2s 返回 200，模型读到 `HELLO 42`；
同进程内把 client 切成 h1.1 后立即恢复。属上游网关（ollama.com，前置 CDN）
的 h2 缺陷，客户端侧规避。

修法：`HttpClientSpec.http1_only`（默认 `false`），**仅模型 provider 的 client
置 `true`**（`plugins/provider-client` 构造处）。对话/嵌入/视觉流量是一问一答，
没有 h2 多路复用需求，h1.1 的连接池 keep-alive 照常复用；web_fetch 等其他用途
保持 h2 不受影响。

**这条与图片能力无关也可以独立成立**（大上下文 + 长工具集 + SSE 同样可能触发），
只是它被本功能的端到端验证撞出来了——首 token 预算（300s）都没等到响应头，
表现为"对话卡死且无任何日志"，是最难查的一类故障。

### 3. 与 DSH `read_image` 对账（补齐 4 项，1 项刻意不同）

首版实现没查 DSH 源码（照 ADR-047 的同构思路 + 协议实测推导）。事后对账
（`deepseek-harness/packages/fs/tool-fs/`）结论：

**结构同源**——DSH 的 `read` 是 `Model-facing UTF-8 read`（全文无 image 逻辑），
`read_image` 是**独立工具**；与本 ADR 的 `read_file` / `view_image` 一一对应。
「把 read 做成多模态」在 DSH 也不成立。

**补齐的 4 项**（都是「能识别 ≠ 上游能接收」的缺口）：

| 项 | 实现 |
|---|---|
| 尺寸/像素上限 + 降采样 | 长边 >2048 或像素 >2048² → `Lanczos3` 等比缩；**解码前**按 `MAX_DECODE_PIXELS`（6400 万）硬拒（100MP 图解码要数百 MB） |
| 16-bit PNG 规范化 | 读 IHDR 位深（偏移 24）→ 16-bit 解码后转 8-bit 重编码（少数 provider 直接 400） |
| 坐标乘数 | 结果回带 `original_dimensions` + `scale_hint`（`original/副本`，两轴差 <1% 只给一个数）——模型在原图坐标定位特征时换算 |
| 能力门控 | `ToolContext::chat_model_capabilities`（会话选择 > `preferences.chat`）→ 明确声明了能力但不含 `vision` 时**早失败**，省一次往返与一条误导性 400 |

**刻意不同（1 项）**：DSH 对 `inputModalities === undefined` 也拒绝（它的
undefined = 路由解析不到模型，属异常）；天演的 `capabilities` 是**可选**的用户
标签（`serde(default)` → 空数组，旧配置与新加模型都可能未标注），空数组只说明
"没标"而不是"不能"，因此放行——门控只在"标了能力但没标 vision"时拦。这是本次
对账里唯一一处不能照抄的地方：**能力标签的语义取决于它怎么产生、以及缺失意味着
什么**（DSH 来自 provider 目录，缺失 = 解析失败；天演来自用户勾选，缺失 = 未标注）。

> **实施期教训（记录在案）**：判断"某个模型有没有标 vision"时，我用 PowerShell
> 手搓了一个 TOML 解析，把 `capabilities`（**字母序在 `name` 之前**）归到了上一条
> 模型名下，于是 `deepseek-v4.1-flash:1017` 被误读成"空数组"，还据此写进了代码
> 注释与本文档。已修正：解析逻辑抽成纯函数
> `lookup_model_capabilities`（core `agent::tool_context`）+ 6 个单测（含"未标
> 能力 ≠ 查不到"与 provider 作用域），配置形态用 TOML 字面量**逐字节模拟落盘
> 顺序**。**纪律：判定配置事实一律走代码序列化/反序列化，不手搓解析**。

**不做（有证据再上）**：`compaction-image-offload`（上游返回
`IMAGE_OFFLOAD_REQUIRED` → 追加持久事件 `image/offload` → 最早输入图投影成
占位文本 → 重试）。它是**事件溯源式**的（不改历史内容，故不产生 ADR-048 那类
前缀分叉），形态是对的；但需要 provider 侧错误码识别 + 请求错误瀑布 seam，
而天演目前没有任何"上游要求卸载"的实测证据——按「不为假设的需求加机制」搁置。

## 与既有决策对账

- **ADR-001 / VFS**：图片字节是**运行期产物**（与快照 ADR-006、spill ADR-047、
  MCP 图片 ADR-010 同类例外），不经 VFS——它不是知识/记忆/技能内容。tool 结果
  的**引用文本**照常落 `session_messages`（ADR-018）。
- **ADR-004 / 前缀缓存**：注入点是**历史中已有的 tool 消息**，不新增末尾合成
  消息；内容逐字节恒定，前缀行为与普通 tool 结果一致。
- **ADR-043 / 工具对连续性**：`normalize_tool_pairs` 只重排消息顺序，不看
  content 形态；带图的 tool 消息仍是 `role: "tool"` + `tool_call_id`，配对语义不变。
- **ADR-047 / spill**：两者是**同一族**（大对象出库、会话留引用、best-effort
  降级），但方向相反——spill 是"结果太大→落盘"，本 ADR 是"结果里有大对象→
  落盘并把引用变成内容"。共同纪律：落盘失败/副本缺失**保持可用**，不报错。
- **ADR-049 / 插件化**：工具定义与门控在 `plugins/tools-fs`，底层实现
  （magic bytes / sha256 / 冻结副本）在 core `executor::image`（与 read_file 的
  "定义在插件、算法在 core"同形）。

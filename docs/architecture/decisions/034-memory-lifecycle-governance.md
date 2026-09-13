# ADR-034: 记忆生命周期治理（巩固通道 + 消费面分离 + 压缩契约）

**日期**: 2026-09-13
**状态**: ✅ 已采纳
**影响范围**: `core/src/common/types/uri.rs`（memory_paths 运维判定）、`core/src/vfs/summary/`（概览压缩契约）、`core/src/scheduler/tasks/`（summary_task / gc_task / evolution_task）、`server/src/api/insights/`（面板过滤）、`core/examples/memory_cleanup.rs`（存量清洗工具）

---

## 背景（2026-09-13 侦查实证）

记忆/规则长期"只增不减、越用越碎"，数据实证：

| 问题 | 证据 |
|------|------|
| **运维数据混入记忆** | 80 条"记忆"中 31 条（39%）是运行状态/日志——`extraction_state` 17 条（旧提取管道遗物，代码零引用）、`evolution_reports` 12 条（每日 +1 永不清理）、`task_states` 2 条 |
| **同主题无限碎片** | 发布流水 ×N、事件架构 ×5、环境事实 ×4、审查流程 ×8；L1 中位 1026 > L2 中位 540 的倒挂 43/80 条 |
| **删除通道从未生效** | `find_entry` 仅匹配**目录**条目（`if !entry.is_directory() { continue; }`），而规则/技能/记忆条目在 VFS 中均为**文件**——演化删除"目标不存在，跳过"被静默吞掉；`memory/archive`、`skill/_archive`、`agent/learned/archive` 三个归档目录恒为空；报告声称的"清理 83/93 条规则"从未发生（391 条规则含 119 条已被实测证伪的 shell 元字符禁令） |
| **记忆治理缺失** | 记忆写入 add-only（update/merge 被静默丢弃）；`auto_consolidation` / `consolidation_interval` / `decay_rate` 配置**只有定义零实现**；GCD `scan_memory` 仅 `list` 一层，嵌套条目从未被扫到（90 天 TTL 从未生效） |
| **概览（L1）扩写污染** | 短条目（几百 token）被"详细概览（2000 token 以内）"prompt 扩写为多节文章并**引入原文外信息**（如为"装 ripgrep"一条记忆补出 apt/brew/choco 细节）；L1 参与检索注入（0.6~0.85 相关度加载）与向量索引——脑补内容进上下文 |
| **综述清单不完整** | inventory 对 memory 仅浅列目录名（综述看不到条目明细，无法巩固）；skill root 指向空目录 `skill/learned`（技能实际在 `skill/{id}` 根下） |

## 决策

### 1. 运维子域消费面分离（`memory_paths::is_operational_path` 单一判定）

`memory/events/{extraction_state,evolution_reports,task_states}` 与 `memory/archive/` 是**运维数据**（状态/日志/归档），不是记忆内容。判定函数单点定义于 `common::types::memory_paths`，三处消费面统一过滤：

- **摘要生成**（SummaryTask）：运维子域不生成 L0/L1 与向量（不参与检索）；
- **记忆面板**（insights handler）：运维子域不作为「记忆」展示；
- **GC**：白名单之外豁免（见 §3）。

运维数据**物理保留**在 memory 命名空间（任务状态水位线、审计报告有真实用途），仅从"记忆消费面"移除——不做命名空间级迁移（避免架构级动静）。

### 2. 概览压缩契约：L1 ≤ L2（`SummaryEngine::generate_overview` 单点）

- **短内容直用**：内容本身 ≤ `OVERVIEW_TOKEN_LIMIT`（2000 token）时，L1 直接复用原文（概览无语义压缩空间，LLM 生成只会引入扩写风险）；
- **长度守卫**：超容量时 LLM 生成，生成结果**不得超过原文长度**（超过即告警回退原文）；
- **prompt 忠实原则**："只包含原文中出现的信息，不得引入原文之外的内容；原文已简洁处不得扩写"。

L0（摘要）保持原语义（<100 token 短路径 + LLM 简洁摘要）。本契约同时覆盖知识导入（ingestor 经 `SummaryService` 共享）。

### 3. GC 白名单化 TTL（递归扫描 + 按域分层）

- `scan_memory` 改**递归**遍历全树（修复"只扫一层"缺陷）；
- TTL **白名单化**（未列入的域默认豁免，防误删）：`cases/` 与 `clipboard/` 90 天（`memory_ttl_days`）；`events/evolution_reports/` 30 天；`facts/`、`events/decisions/`、`events/task_states/`、`archive/` 等**豁免**（删除权归演化治理或任务自身）；
- 只删叶子条目（目录下钻）。

### 4. 记忆巩固通道（演化综述）+ 写入收口

- **`find_entry` 修复**：按名字匹配**任意条目**（文件/目录），目录继续递归——恢复删除通道（修复后归档目录首次产生内容）；
- **`MemoryPlan` 支持 `merge`**：`{"action":"merge","id":"目标条目","content":"归纳内容","merge_from":["被合并 id"]}`——目标原地重写（未命中降级为新建），`merge_from` 经软删除通道归档（摘要标注"已被合并到 X"）；
- **`auto_consolidation` 配置兑现**：`true`（默认）时综述 prompt 含巩固职责（查重/合并/归档引导 + merge 输出格式）；`false` 时不出现在 prompt 且 apply 层跳过 merge（保守模式）；
- **写入收口**（prompt 写前原则）：过程记录（版本发布、功能完成、例行里程碑）**不写记忆**（在演化报告与项目文档留痕）；只写跨会话可复用的偏好/事实/教训/决策；过时/被证伪/重复条目（含记忆）主动列入 deletions；
- **综述清单完整化**：memory 递归列出条目明细（运维子域跳过、条目上限约束）；skill root 修正为 `skill/`（根下直挂）；清单只列文件条目（跳过归档子目录）。

### 5. 存量清洗（一次性，`core/examples/memory_cleanup.rs`）

幂等、可 dry-run 的清洗工具（备份后执行）。执行结果（2026-09-13）：

| 项 | 前 | 后 |
|----|----|----|
| memory 活跃条目 | 80 | **22**（+ 归档 19） |
| user 条目 | 12 | **3**（+ 归档 10） |
| agent/patterns | 14 | **4**（+ 归档 3） |
| agent/learned 规则 | 391 | **31**（+ 归档 360） |
| extraction_state | 17 | 0 |
| L1>L2（活跃条目） | 43+ | **0** |

## 后续候选（未实施，按需触发）

- `decay_rate` / 访问追踪（access_count 从未维护）——衰减机制需要访问计数基础设施，暂缓；
- 记忆面板覆盖 user/agent 命名空间（`Preference → user/preferences`、`Pattern → agent/patterns` 当前不在面板显示）——面板口径议题；
- 同 id 多条记忆（`evo-{ts}` 时间戳一次运行多条共享 id）下 merge/delete 按 id 查找的歧义——当前按"首个命中"处理；
- 规则自动提炼通道（rule_recorder）的产出质量复审。

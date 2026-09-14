# 天演 Tianyan 0.4 发布说明

> 0.4.1 发布日期：2026-09-14（grep 工具搜索根修复）· 0.4.0 发布日期：2026-09-13（记忆生命周期治理 ADR-034 + 演化治理链路修复）· 0.3.18 发布日期：2026-09-13（待办整表替换 + 命令输出落盘回读）· 0.3.17 发布日期：2026-09-13（工具表稳定性 + 嵌入用量入账 + 协议工具豁免 + 终止/停止竞态修复）· 0.3.16 发布日期：2026-09-12（文档固化 8 份 + 结构重构与技术债清理）· 0.3.15 发布日期：2026-09-12（安全策略收敛默认自主 ADR-033 + 思考语言中文约束 + 文件日志修复）· 0.3.14 发布日期：2026-09-12（预算误报修复 + 压缩阈值 60% + 读通道治理 + 思考块收起）· 0.3.13 发布日期：2026-09-12（压缩点 user 锚定：防压缩后全 system 空输出）· 0.3.12 发布日期：2026-09-12（压缩点体验：判定口径修复 + 实时推送 + 可视化标识）· 0.3.11 发布日期：2026-09-11（记忆面板体验：分类树展示 + L0/L1/L2 层级切换）· 0.3.10 发布日期：2026-09-11（记忆浏览修复：递归展开 + 时间戳解析 + 父链创建）· 0.3.9 发布日期：2026-09-11（唤醒轮实时性：转发死锁修复 + 流式转发统一重构 + ollama 思考字段回传 + 前端配置往返保真）· 0.3.8 发布日期：2026-09-10（会话列表最后对话时间 + 设置页输入丢焦点修复 + 待办面板样式）· 0.3.7 发布日期：2026-09-10（后台通知实时显示 + 滚动跟随重构）· 0.3.6 发布日期：2026-09-09（对话流滚动体验 + 技能系统回归 VFS 方法论文档 + License 改回 MIT）· 0.3.5 发布日期：2026-09-08（唤醒轮可见性：EventSource 常驻 + 事件纯函数化）· 0.3.4 发布日期：2026-09-05（压缩会话反馈与占用统计）· 0.3.1 发布日期：2026-09-04（统一事件推送，ADR-028）· 0.3.0 发布日期：2026-09-04（会话时序链模型重构，ADR-027）· 0.2.0 发布日期：2026-08-30 · 0.2.2/0.2.3 修复日期：2026-08-30 · 0.2.4 修复日期：2026-08-30 · 0.2.5 修复日期：2026-08-31 · 0.2.6 修复日期：2026-09-01 · 0.2.7 修复日期：2026-09-03 · 0.2.8 修复日期：2026-09-03 · 0.2.9 修复日期：2026-09-03 · 0.2.10 修复日期：2026-09-03 · 0.2.11 修复日期：2026-09-03 · 0.2.12 修复日期：2026-09-04

天演（Tianyan）是一个本地优先的通用智能助手：桌面应用（Tauri）+ 本地服务（Axum）+ 自进化智能体（VFS 统一知识/记忆/技能/规则）。

## 0.4.1 grep 工具搜索根修复

- **修复（工具链）**：`grep` 的搜索根解析缺失工作区归属——相对 `path` 与缺省 `path` 均以**进程 cwd**（桌面端为 exe 安装目录）为基准，目标遍历不到时静默返回 0 条假阴性（脚手架排查误判「文档断链」的根因）。修复后与 `glob` / `read_file` 同一套归属规则解析到**会话工作目录**（无会话保持旧回退语义），搜索输出回显解析后的搜索根
- **回归保护**：新增两条回归测试（相对路径 / 缺省 path）；全量门禁通过（fmt / clippy -D warnings / 1211 项单测）

## 0.4.0 记忆生命周期治理（ADR-034）+ 演化治理链路修复

- **删除通道修复（重大）**：`find_entry` 仅匹配目录条目的缺陷使演化删除**自上线起从未命中任何目标**——三个软删归档目录恒为空，报告声称的「清理 83/93 条规则」从未发生（391 条规则含 119 条已被实测证伪）。修复 = 按名字匹配任意条目（文件/目录），删除/归档通道首次真正可用（含**真实后端**集成验证）
- **概览压缩契约（L1 ≤ L2）**：短记忆此前被「生成详细概览」prompt 扩写为多节文章并引入原文外信息（实测 80 条记忆中 43 条 L1 > L2 倒挂，且 L1 参与检索注入与向量索引）。修复 = `generate_overview` 对 ≤2000 token 内容直接复用原文（无压缩空间时不调 LLM）；超容量时 LLM 生成 + **长度守卫**（超原文回退）+ prompt 忠实原则（不得引入原文之外的信息）
- **记忆巩固通道 + 写入收口**：演化综述支持 `merge`——同主题碎片归纳为一条主题条目（`merge_from` 旧条经软删除归档）；`auto_consolidation` 配置兑现（综述 prompt 巩固职责 + apply 层开关）；过程记录（版本发布/功能完成/例行里程碑）**不写记忆**（在演化报告与项目文档留痕），过时/被证伪/重复条目主动列入删除
- **运维子域消费面分离**：`extraction_state`（旧提取管道遗物）/`evolution_reports`/`task_states`/`archive` 统一判定（`memory_paths::is_operational_path`）——记忆面板、摘要生成、检索三处过滤；运维数据物理保留（任务水位线/审计），仅退出「记忆消费面」
- **GC 递归 + TTL 白名单分层**：`scan_memory` 改递归全树（此前只扫一层，90 天 TTL 从未生效）；白名单化——`cases`/`clipboard` 90 天、`evolution_reports` 30 天、语义记忆与状态类豁免（防误删）
- **综述清单完整化**：memory 递归列条目明细（巩固的前提；此前只有目录名）；skill root 修正（此前指向空目录，综述看不到技能）；清单跳过归档子目录
- **存量清洗（工具随包，`core/examples/memory_cleanup.rs`；幂等 + dry-run + 执行前备份）**：本机已执行——记忆 80 → 22 条（同主题合并 7 组、过时/重复/流水账归档、死数据清理）、user 12 → 3、patterns 14 → 4、规则 391 → 31（早期爆炸存量归档 360 条）；全部软删除可恢复（`*/archive/`）。**升级后记忆面板即为此主题化状态**
- **测试**：core **1209** / server 152（+1 ignored）/ mcp 16 / tauri 9 全绿；**9 项反向验证**（注入旧行为必红；含真实后端集成验证——SqliteBackend 形态锁定删除/合并链）；fmt · clippy `-D warnings` 0

## 0.3.18 待办整表替换（对齐 DSH）+ 命令输出截断落盘回读

- **待办 `create` 语义改为整表替换（对齐 DSH last-write-wins）**：旧语义“全批完成才替换、否则永远追加”——模型重新规划后**废弃项永久残留**（实测 4 个会话累积 15 项废项，只增不减）。修复 = `create` 无条件**整表替换**（`TodoStore::replace_many` 原子写入、失败不触碰旧批），重规划即天然清理废项；单条快捷形态删除（全量语义下“单条 = 替换为一条”反直觉易误用）；响应新增 `replaced` 计数；父子挂靠改经 `update`。配套：存量 15 项废项已清理；ADR-022 增修订节
- **命令输出截断落盘 + read_file 回读闭环**：`execute_command` 输出此前经尾部截断（50KB / 2000 行）后**完整输出被丢弃、无法找回**（前台路径无任何回读通道）。修复 = **截断前**完整输出落盘 `{data_dir}/command_logs/exec-{uuid8}.log`（header + stdout + `[stderr]` 分节，best effort），返回新增 `stdout_total_bytes` / `stderr_total_bytes` / `log_file`；截断标记升级为“输出已截断，共 N 行 M 字节，完整输出已保存至 `<path>`，可用 `read_file` 的 offset/limit 分页读取”——**截断不再等于丢失**
- **测试**：core 1194 / server 151（+1 ignored）全绿；新增/补强测试均经**反向验证**（注入旧行为必红：追加语义“left 4 vs right 2”、跳过落盘 panic）；fmt · clippy `-D warnings` 0 · tool-catalog freshness ✓

## 0.3.17 工具表稳定性（前缀缓存友好）+ 嵌入用量入账 + 协议工具与竞态修复

- **工具表稳定性三项（ADR-030 后续演进，前缀缓存友好）**：① **动态工具有序化**——注册表由 `HashMap` 改为注册序 `Vec`（追加末尾、按名去重），`definitions()` 声明稳定性契约（连续两次取定义**逐字节一致**），消除工具定义块漂移静默打掉前缀缓存；② **角色清单与工具描述解耦**——`delegate_to_agent` 描述不再注入运行时角色 L0 摘要（保留内置角色名 + 指向 `suggest_role` 按需查询），角色增删不再影响描述稳定性，未知角色报错附全量可调用清单（一轮内自愈）；③ **会话级工具表变化检测**——每个**真用户轮**请求前比对工具表指纹（有序名 + 描述 + params schema），变化时**强制压缩一次**（压缩点 = 天然 tools 刷新点；首轮只记基线；唤醒轮不检查）
- **嵌入用量入账 + 查询缓存**：嵌入调用此前完全不入账（`usage_logs` 只有 chat provider——“账单看不到嵌入费用”的误判来源之一）；新增 `EmbeddingUsageSink` 契约 + `UsageLogEmbeddingSink`（VFS 与 Agent 实例均注入，含热重载）。同一 query 一次检索被嵌 2 次的浪费经**查询缓存**（键 `model|dimensions|text`，256 FIFO）消除；空文本/空查询短路。**验证**：升级后任意检索一次 → `usage_logs` 出现嵌入 provider/model 记录；同 query 再检索一遍不新增（缓存命中）
- **终止轮不再显示上一轮结论**：取消/失败轮后，服务层把“会话最后一条 assistant”（= 上一轮）当作本轮边界事件下发，前端合并进本轮占位气泡——修复 = 轮前记水位 `prev_assistant_id`，只下发**本轮新产生**消息（本轮无新消息则静默不下发）
- **子代理 `submit_result` 恢复正常调用**：请求侧恒注入协议工具、执行侧角色白名单未豁免 → 调用被误拒（结果只能靠最后输出兜底）；修复 = 协议工具豁免单点（`PROTOCOL_TOOLS` / `is_protocol_tool`，请求注入与执行豁免同源；白名单外普通工具仍被拒）
- **停止按钮竞态**：新会话首个请求未返回时（会话 id 未就绪）点停止 → 取消请求发不出、后端继续跑完而前端显示已中止；修复 = `cancelWhenSessionIdReady`（等待 id 就绪后补发取消）
- **测试**：core 1191 / server 152 / mcp 16 / tauri 9 / 前端 395 全绿；本轮新增测试均经**反向验证**（注入旧行为必红）；fmt · clippy `-D warnings` 0 · eslint · tsc · prettier 全绿

## 0.3.16 文档固化（机制/运维 8 份）+ 结构重构（技术债 12 项处置）

- **机制文档 4 份（新增）**：`docs/architecture/context-pipeline.md`——组装顺序（含最易漏的 `project_instructions` 层）+ 压缩机制（触发线 0.6 / 临界 0.8 / 保留 10 / 最小 6；**只在整轮收尾检查一次**，这是"体感 80% 才压缩"的机制原因；`prompt_side_tokens` 单点口径；压缩点 user 锚定）+ 上下文构成量化 SQL + 三类故障模式；`event-protocol.md`——4 类事件字段表 + `StreamChunkType` 7 成员 + 订阅/快照恢复 + 可靠性分层（并核实出与 ADR 的 10 处描述不一致）；`model-provider-notes.md`——reasoning 回传契约（带 tools 必须全量回传）+ 思考强度×语言实测 + 前缀缓存口径 + 上游异常；`task-runtime.md`——三类任务、并发排队（20/40/8）、3 天 TTL、唤醒语义、取消与回退、子会话不索引 FTS 边界、面板数据流
- **运维手册 3 份（新增）**：`docs/operations/troubleshooting.md`（日志位置 + 三类高频故障路径 + DB 只读取证 + QA 纪律 + PowerShell/cargo 陷阱）、`data-health-check.md`（10 表 + 18 条巡检 SQL，**全部经 EXPLAIN 实测可执行**）、`release-msi.md`（版本落点 + 打包流程 + CI 链 + 已知坑 + 装前/装后验收清单）
- **结构重构（4 个超长函数拆分，行为零变化）**：`spawn_background` 308 → 65 行（+8 私有 helper）、`run_stream` 226 → 46、`run_turns` 222 → 105、子代理委托入口 235 → 95；字符串字面量集合比对确认日志/错误消息逐字保留
- **技术债清理**：VFS 目录列举与删除的前缀匹配（`LIKE` → `substr + length`，修 `_` 被当通配符误匹配、Rust 字节长度与 SQL 字符长度混用）、usage 统计 WHERE 子句单点构造、截断实现收敛到 `common::truncate` 单点、**事件总线有界化**（4096 + `try_send` 丢弃计数与告警——杜绝订阅者积压导致的内存无限增长）、命令输出事件 100ms 时间窗合并（对齐 ADR-028 声明）+ 收尾 flush、统计落盘语句复用、`SqliteDb` 错误边界收敛（rusqlite 类型不再出现在公开签名）、`agent/role_store` 兼容层删除
- **脚本**：`build.ps1` 产物路径修正（此前查 `tauri/target`，实际在 workspace `<root>/target`）、补 `protoc` 检查（CI 有、本地缺）、重复前端构建提示
- **测试**：core 1182 / server 146 / mcp 16 / tauri 9 / 前端 392 全绿；新增 5 个回归测试（VFS 前缀 ×2、usage 口径 ×1、事件总线背压 ×1、输出节流 ×1），**均经反向验证**（注入旧缺陷必红）；fmt 干净、clippy `-D warnings` **0 警告**、tool-catalog freshness 通过

## 0.3.15 安全策略收敛（默认自主，ADR-033）+ 思考语言中文约束 + 文件日志修复

- **安全策略收敛：默认全自主（ADR-033）**：审批行为收敛为单枚举 `approval_mode`（`autonomous` 默认 / `confirm` / `interactive`）——默认黑名单外全放行（含子代理，主/子一致、零打扰）；评估链改为「黑名单 Deny 最优先 → 模式短路单点」。命令检查层收敛为四态 `safety_mode`（新增 `relaxed` 默认：跳过元字符/解释器检查、**黑名单仍强制**；原 strict/transform/permissive 保留）。四个旧开关下线（`allow_all_operations` / `wait_for_approval` / `confirm_commands`（死开关）/ `unattended_mode`（死开关））：旧配置读取兼容 + 启动自动归一，写回时自然消失——`allow_all_operations=true` 的老配置行为完全不变（迁移为 `safety_mode=relaxed`）；想收紧可用 `approval_mode="confirm"` / `safety_mode="strict"`
- **思考语言中文约束（soul）**：默认 soul 增加「思考语言」节——内部思考必须中文（代码/报错/专有名词除外）。实测：无约束时思考 CJK 占比仅 3.5%~15.4%；约束后 12/12 探测切中文（含「英文历史+英文工具输出」抗锚定场景）；同义内容中文比英文省 ~13% token（思考占上下文 ~40% → 增速约 -5%）。**需重启应用生效**（新会话立即；已有会话下次压缩点刷新）
- **文件日志静默归零修复（file_filter 接线）**：`tauri` 的 `file_filter` 建了未挂载（unused）——设置 `RUST_LOG` 后会连带过滤文件层，GUI 实例文件日志可能静默为空（流式中断无取证）。修复 = 按层过滤器（控制台层 RUST_LOG 优先 / 文件层恒配置级别），并抽 `layer_filters` 纯函数 + 回归测试
- **测试补强（三处回归缺口 + 1 处小缺口，均带判别力验证——旧实现必红已实证）**：续读偏移无跳行（字节截断场景）/ `read_file` 超大 limit 钳制与 `total_bytes` / `vfs_read` 会话导出截断 / 压缩点自身 usage 不计入压缩判定
- **前端与文档**：审批面板与设置页对齐新模式（审批模式 / 命令检查选择器）；eslint 解构剔除误报修复；`development.md`（vitest 392）/ AGENTS.md / CHANGELOG 口径同步
- **测试**：core 1174 / server 165 / tauri 9 / 前端 392 全过；fmt 干净、clippy 0 error

## 0.3.14 预算误报修复 + 压缩阈值 60% + 读通道体量治理 + 思考块收起

- **压缩后“预算不足 1024”误报（会话卡死根因）**：压缩后继续对话直接报错、且该会话此后每条消息必现。根因双叠：① `run_turns` 实测输入恢复为 `tokens.input + cache.read` double count（`input` 已含缓存命中、`cache.read` 为其子集，判定值≈真实×2——事故实测：压缩前 input=593399/cache_read=592128，判定 1185527 直接越过 1M 窗口 → `dynamic_max_tokens` 返回 None，请求不发出）；② 恢复范围未按压缩点截断——压缩后组装视图已从压缩点重新开始（实际仅 ~1.7 万），预算仍按压缩前的旧实测计算。修复 = 取数口径与压缩判定共用 `StructuredMessage::prompt_side_tokens()` 单点（input，max 兜底）+ 只取最后一个压缩点之后的实测（压缩后首轮退回估算）；压缩判定同步排除压缩点自身（其 tokens 是摘要请求用量，非会话占用）。新增 2 个回归测试（用事故真实数字复现）
- **默认压缩阈值 50% → 60%**：上下文仍有余量时尽量保留原文，减少压缩频次与摘要信息损耗
- **读通道体量治理（大文件/大输出直灌上下文）**：`read_file` 改“按需读取”引导（默认前 2000 行/约 50KB，先 grep/symbol_outline 定位再按行范围精读；结果新增 `total_bytes`；续读偏移与 showing.limit 按实际返回行数计算）；`execute_command` stdout/stderr 尾部截断（50KB/2000 行 + `*_truncated` 标志——此前单条输出可达 45.5 万字符直灌上下文）；`vfs_read` 会话导出头部截断 + 标注总数 + 指引 `session_recall`
- **思考块默认收起 + header 横幅**：思考过程默认折叠；收起时 header 单行横幅显示最新思考（流式期间实时滚动、truncate 截断、hover 看全文）；点击展开全文，子代理面板复用同一组件
- **测试**：core 1166 / server 145 / 前端 392 全过；新增预算误报回归 ×2、命令截断回归、思考横幅回归

## 0.3.13 压缩点 user 锚定（防压缩后全 system 空输出）

- **压缩摘要改 user 角色锚定（对齐 DSH checkpoint 设计）**：压缩摘要此前为 `system` 角色——压缩后的组装视图（摘要 + system 通知 + 唤醒指令）可能全为 system，云 API 判为"无用户输入"返回**空输出**（唤醒轮无汇总事故的根因之一；实测本地复现 `finish_reason:"load"` 空响应、补一条 user 即恢复生成）。修复 = 摘要消息落库为 **user 角色**（压缩后视图恒有 ≥1 条 user 锚点）；组装层按 `compression_marker` 统一归一（历史旧数据 system → user，两代数据同一转换路径）——旧会话（含既有压缩点）立即受益，无需等新压缩
- **推送门控同步（体验不回退）**：`event_push` 由「role == System」改为「role == System || compression_marker」——压缩点 user 锚定后仍落库即推送（实时可见保持 0.3.12 行为）
- **前端适配（展示与 0.3.12 完全一致）**：压缩点保持「徽章 + 分割线」无框节点渲染（不套用户气泡、隐藏回退按钮——回退锚点只属于真实用户输入）；`applyServerMessage` 压缩点走独立幂等路径（不受乐观合并「未确认 user_message_id 跳过」逻辑影响）；手动压缩响应改幂等 upsert（推送 + 响应双路径收敛不重复）
- **测试**：core +1（两代数据归一 + 普通 system 不受影响）；server 更新（user 锚定推送断言）；前端 +3（旧数据兼容渲染 / 乐观消息在场仍追加 / 响应与推送收敛）

## 0.3.12 压缩点体验（判定口径修复 + 实时推送 + 可视化标识）

- **压缩判定 double count 修复（提前压缩根因）**：压缩触发时用「最近一次请求的真实占用」对比窗口阈值（50%），但计算为 `tokens.input + tokens.cache.read`——`tokens.input` 是**完整输入**（含缓存命中部分，`cache.read` 为其子集），相加使判定值≈真实值×2（实测 302K 真实占用被算成 594K，越过 1M 窗口的 500K 触发线）——压缩在真实占用 27%～30% 时就提前发生。修复 = 取 `input`（`max` 兜底异常口径），恢复「窗口 50% 才压缩」的设计语义；新增正反两个回归测试
- **压缩摘要实时推送（与普通消息同构，不再需要重载才发现）**：此前 `compression_marker` 消息被有意排除在推送之外（"历史加载已含"）——自动压缩静默落库，页面完全无感知（用户无从知道压缩发生过、上下文占用为何没降）。修复 = 移除特例，压缩摘要与后台通知走同一条 System 消息推送路径（落库后补发 chat_stream 边界事件 → 前端 `applyServerMessage` 按 id 查重追加）——压缩点即会话时序链上的普通节点（统一结构，不做区分）；圆环在压缩后的空窗期即时更新（摘要消息带压缩请求 usage，`lastMessageUsage` 压缩点分支生效），不必等下一条请求
- **压缩点可视化标识**：摘要消息渲染「压缩点」徽章 + 分割线（时间线顶部），正文仍走统一时间线（SegmentBlocks），不另做渲染分支——压缩点在页面上可辨识
- **测试**：core 新增 double count 正反回归（真实占用 4800<阈值 不压 / 5100>阈值 压）；server 推送测试改为断言压缩摘要 `chat_stream` 事件（含 `compression_marker` 透传）；前端新增压缩点徽章渲染 ×2 + 推送追加 ×1 用例

## 0.3.11 记忆面板体验（分类树展示 + L0/L1/L2 层级切换）

- **左列表改分类树**：按 `relative_path` 分段建树（`memory-tree.ts` 纯函数）——目录节点字母序、**类目内部按时间倒排**（后端时间序的稳定子序列），目录可折叠/展开并显示叶子计数；叶子显示条目名（路径信息由树结构表达），hover 显示完整 URI
- **详情面板加 L0/L1/L2 层级切换**：此前是"回退链"（有 detail 只看得到 L2），用户无法查看 L0 摘要 / L1 概览。改为一组层级按钮（三层均显式可见）——默认选中有内容的最高层，无内容的层级禁用；点击任意层级查看对应内容
- **测试**：新增 `memory-tree.test.ts`（建树/排序/嵌套/回退 5 例），MemoryPanel 测试更新为树渲染 + 层级切换 + 目录折叠（前端全量 385 通过）

## 0.3.10 记忆浏览修复（递归展开 + 时间戳解析 + 父链创建）

- **记忆面板只显示 3 个空目录、记忆条目全部不可见**：根因 = `GET /api/v1/memory` 只列 `tianyan://memory/` 一层（`vfs.list` 单层语义）——返回的 events/facts/cases 三个目录条目无内容（abstract/overview/detail 全 null）、importance 为默认 0.5（前端渲染成 ⭐50%），且前端无目录导航，73 条记忆条目全部不可达。修复 = handler 递归展开子目录、仅返回叶子条目，按 updated_at 倒序（最近记忆在前），新增 `relative_path` 字段区分跨目录同名条目（如 cases 下 failed/successful 同名）；前端列表改显相对路径（hover 显示完整 URI）
- **VFS 条目时间戳解析静默失败**：根因 = SQLite `datetime('now')` 产出 `YYYY-MM-DD HH:MM:SS`（非 RFC3339），`parse_from_rfc3339` 解析永远失败——条目 created_at/updated_at 退回 `EntryMetadata::new` 的构造时刻（`Utc::now()`），时间信息失真、排序失效。修复 = 新增 `parse_stored_timestamp` 兼容双格式（SQLite datetime + RFC3339）
- **VFS 深层路径写入中间目录缺失（递归遍历断链）**：根因 = `ensure_entry_exists` 只创建直接父级——写 `cases/failed_tasks/x` 时创建 `cases/failed_tasks` 但无 `cases`，基于 list 的递归遍历从根找不到该分支（条目"隐身"）。修复 = `mkdir -p` 语义逐级补齐完整父目录链（`ensure_directory_chain`）

## 0.3.9 唤醒轮实时性（转发死锁修复 + 流式转发统一重构）+ ollama 思考字段回传

- **唤醒轮输出不实时显示（需退出重进才看到）**：根因 = 唤醒轮转发器先 `await` 整个 `process_wake` 完成才去取事件——期间事件持续写入容量 100 的通道，输出超过缓冲即阻塞，形成死锁（轮等转发器收、转发器等轮完）。修复 = `process_wake` 改收 `sender` 参数（与 `process_message_stream` 同构），转发器在轮开始前就建通道并立即消费；新增防死锁回归测试
- **流式事件转发三处接线收敛为单一实现（ADR-032）**："chunk → 事件 JSON → 统一通道"的接线原在用户轮 / 唤醒轮 / 子代理三处手写，已出现漂移（容量 100/100/64 不统一、字段注入各写一遍）——上述死锁 bug 正是该结构的产物。core 新增 `agent/stream_forward.rs`：`spawn_stream_forwarder` 原子地建通道并启动消费任务（反向顺序从结构上写不出来），`StreamEventMapper` / `StreamEventDeliver` 保留为可插拔 seam（映射器与送达目标），`inject_stream_event_fields` 单点注入路由字段。三处调用点复用该函数
- **ollama 思考字段回传（传输层方言）**：0.2.7 只修了"读"（`DeltaContent` 加 serde alias `reasoning`）没修"写"——`inject_reasoning_content` 把历史思考无条件写到 `reasoning_content`，而 ollama 的 OpenAI 兼容层（`openai/openai.go` 的 `Message.Reasoning`）**只解析 `reasoning`**。Go 的 `encoding/json` 静默忽略未知字段，所以请求照样成功，但思考内容根本没进 prompt——多轮上下文与思考一致性悄悄受损（实测：ollama cloud 上 `reasoning_content` 的 `prompt_tokens` 零增长，`reasoning` 则 +2300）。修复 = 新增 `ThinkingField` 方言（`ReasoningContent`/`Ollama`），在客户端构造时按「显式配置 > endpoint/名称嗅探 > 默认」解析一次并缓存，`inject_reasoning_content` 按方言写字段名（互斥不双写）；新增 `thinking_field` 配置项作为嗅探不到的网关的逃生门（嗅探只认 ollama 域名与 `:11434`，刻意不猜自建域名）
- **前端配置往返丢弃 provider 新字段**：根因 = 前端 `config-transform.ts` 对 provider 是白名单映射，而后端 `PUT /api/v1/config` 是全量替换——后端新增的 provider 级字段（`thinking_field`）不在白名单里，用户按文档手写进 `tianyan.toml` 后，只要打开设置页点一次"保存"就被静默抹掉。修复 = 双向透传 + 设置页新增「思考字段名」下拉（自动 / `reasoning_content` / `reasoning`）；契约测试新增通用防线（逐字段断言 provider 每个键都能往返），防止下一个新增字段重蹈覆辙

## 0.3.8 会话列表最后对话时间 + 设置页输入丢焦点修复 + 待办面板样式

- **会话列表显示最后对话时间并按此排序**：根因 = 后端 `updated_at` 映射为 `ended_at.unwrap_or(created_at)`——ADR-027 时序链模型下会话没有"结束"概念，`ended_at` 恒为 None，列表显示的一直是创建时间。修复 = `list_meta` SQL 增加 `MAX(ts)` 最后消息时间，排序改为 `COALESCE(MAX(ts), created_at) DESC`；`updated_at` 优先取 `last_message_at`（无消息回退创建时间）
- **设置页 provider/模型名称输入丢焦点**：根因 = React key 用了可变值（name）——输入名字时 key 变化导致组件卸载重建、焦点丢失。修复 = key 改用稳定索引（ProviderCard/ModelRow 三处）
- **待办面板灰底横贯会话框**：外层全宽 div 透明无边框，灰底/边框/圆角移到居中 max-w-4xl 容器（只有待办内容居中的那部分有灰底圆角矩形）
- **唤醒轮移除 reasoning-only 整轮重试**：实测确认 ollama 网关 deepseek-v4-flash:0731 对 system 完成通知只输出思考不输出正文是系统性行为（相同上下文重试结果相同）——整轮重试只浪费 LLM 调用，已移除；保留 loop 内部空响应单次重试（偶发完全空响应）与加载/执行失败重试

- **后台任务/命令完成通知实时显示**：根因 = ADR-031 移除"落库即广播"后，后台任务完成通知（System 消息）只落库不推送——前端实时流永远看不到通知本身（只有刷新/重开会话靠历史加载可见），用户只看到唤醒轮的 assistant 汇总输出（"等通知即可"）。修复 = `BroadcastingSessionManager::add_structured_message` 对 System 角色（非压缩摘要）消息落库后，复用 `map_chunk_to_event` 构造 chat_stream 边界事件（chunk_type=message）推入统一事件通道——与用户消息 `send_message_boundary` 同构，前端 `applyServerMessage` 按 id 查重追加（多条通知独立显示，与 LLM 上下文/历史加载三份一致）
- **滚动跟随重构（DSH observed-top ledger + followSig 模式）**：根因 = 旧实现用 scrollTop 方向判断用户滚动——scrollToBottom 程序化设置 scrollTop 后，浏览器布局/图片加载导致 scrollHeight 变化，下一次 scroll 事件把程序化滚动误判为"用户向上滚动"→ 误脱离跟随 → 内容继续增长时不再跟随 → 视觉抖动；且 follow 用 useEffect 依赖 [messages, stickToBottom]，滚动阈值翻转（setState → effect → scrollToBottom → scroll）形成循环。修复 = ① observedTopRef 账本：程序化写 scrollTop 同步记录，scroll 事件用 |scrollTop - observedTop| > 0.5 判定"用户滚动"（wheel/touch/scrollbar/键盘统一覆盖）；② stickToBottomRef 双轨：scroll 热路径用 ref 避免高频 setState；③ followSig 内容签名：消息数/流状态/会话切换真正变化才 follow；④ ResizeObserver 替代 setTimeout 兜底（异步内容加载导致列高度变化时若仍跟随则滚动到底）；⑤ FOLLOW_THRESHOLD 150 → 24（滚轮一次 100-300px 立即脱离，不再被拉回）

## 0.3.6 对话流滚动体验 + 技能系统回归 VFS 方法论文档

- **对话流滚动体验**：① 打开长会话不再停在中间——滚动逻辑单次 rAF 设置 scrollTop，长会话/重内容（markdown、代码高亮、图片异步加载）渲染跨帧时 scrollHeight 未达最终值；改为多帧兜底（rAF + setTimeout 150ms 再滚一次）。② 底部向上滚动不再受限/来回跳动——旧逻辑"流式中距底部 150px 内就强制拉回"，滚轮一次滚动 100-300px 仍在阈值内，下一条流式增量到达即被拉回；改为 stickToBottom 状态机（聊天经典模式）：用户向上滚动立即脱离跟随（即使 1px 也不再被拉回），滚回底部自动恢复，发送消息/切换会话重置为跟随
- **技能系统回归 VFS 方法论文档**：技能 = VFS `skill/` 命名空间下的方法论文档（无执行语义），读写全走 VFS——删除 6 个桥接技能（file_read/file_write/file_delete/file_list/system_command/http_request）与 `SkillExecutor`/`SkillHandler`/`SkillRegistry`/`SkillRefresher`/`SkillSync` 等全部执行型基础设施（-3849 行）；`call_skill` 改为读 VFS 技能文档（L0 摘要 + L2 详情）返回，由 LLM 参考后自行用工具执行；planning 预置进 VFS（bootstrap 写入，与 GEPA 学习技能同构）；`/api/v1/skills` 列表/详情/执行统一走 `SkillManager`；`security.skill_*` 配置字段与 `allow_dangerous_skills` 删除
- **License 改回 MIT**：个人项目不设防换皮限制（README badge / LICENSE 全文 / Cargo.toml / CHANGELOG 同步）
- **README 全面同步**：支持的模型（Provider→Model 二级结构 + OpenAI 兼容任意接入 + Ollama）、配置路径（固定 `~/.tianyan/tianyan.toml`，ADR-023）、API 端点表（删已移除端点、补 events/todos/goals/tools/roles/scheduled-tasks 等）、架构树（补 db/roles/goals/todos/lsp 等模块）、工具数 25→30、参与方式（不征集外部贡献，鼓励提 Issue 或 Fork 独立演进）

## 0.3.5 唤醒轮可见性（EventSource 应用级常驻 + chat_stream 归约器纯函数化）

- **现象**：后台命令完成后主 agent 的唤醒轮输出（汇总/汇报）前端实时看不到——切走会话再切回（触发订阅快照）才出现；"思考过程 @if" 碎片垃圾消息入库
- **根因 1（前端）**：`routeChatStreamEvent` 依赖活跃归约器注册表（发消息时注册、流结束注销）——主循环空闲时唤醒事件到达无归约器可路由 → 静默丢弃；`useUnifiedEvents` 挂在 ChatPanel/AgentTasksPanel 上，最后一个消费者卸载即 `stopUnifiedEvents()` 关闭 EventSource——切到设置等面板期间订阅连接断开，切回靠 onopen 重放（看到的其实是重连快照）
- **根因 2（core）**：唤醒轮带 `allow_empty_answer` 豁免（空响应直接放过不重试）——失败场景（cmd 退出码 1 + "必须汇报"指令）下模型返回仅含碎片思考的"@if"空输出，框架直接放行并持久化
- **修复**：EventSource 应用级常驻（模块级启动、进程存活期间不关闭，组件卸载只移除回调）；chat_stream 归约器纯函数化（删除注册表/实例/`routeChatStreamEvent`，`handleChatStreamEvent` 无状态常驻处理，事件自带 session_id 直接映射 store——唤醒轮/主对话/子代理同一入口）；唤醒轮移除空输出豁免（与用户轮同构，空响应统一重试一次，失败场景"必须汇报"指令 + 重试 = 模型第二次机会正确输出）

## 0.3.4 压缩会话反馈与占用统计

- **压缩中反馈**：点击压缩后消息流底部显示「压缩中...」（spinner + aria-live），textarea 与发送按钮禁用（`compressing` 纳入 `canSend`/`disabled`/Enter 守卫）——避免用户在压缩完成前输入，与服务端基于分叉状态压缩并发；压缩完成/失败后恢复
- **压缩后圆环占用立即更新**（上一版下一轮才变）：根因 = 摘要消息 `tokens` 全零 → 前端 `usage:null` → `lastMessageUsage` 跳过摘要、命中压缩前最后一条 assistant 的占用。摘要消息现携带压缩请求真实 token 用量（`summarize`/`incremental_summarize` 改 `chat_completion()`），圆环对摘要消息用「第一条 assistant 输入（≈系统前缀）+ 摘要输出」估算压缩后上下文，新请求完成后新 assistant 消息优先命中
- **会话消耗计入压缩请求**：压缩不走 AgentLoop（无 assistant 消息），其消耗此前从未入账；摘要消息自带压缩请求真实 input/output/cache，`sumSessionUsage` 自动累计（压缩请求 = 一次完整 LLM 请求，输入/输出/缓存全量计入）
- **API 契约**：`ChatMessage` 新增 `compression_marker`（历史加载 + 流式边界事件从 `StructuredMessage.compression_marker` 映射），前端识别摘要消息（历史刷新后占用统计一致）

## 0.3.1 统一事件推送（后台通知/任务状态/命令输出实时可见，ADR-028）

- **根因**：后台任务/命令完成通知只落库（`SessionStore.append_message`），无推送通道——前端 store 仅在流式期间或挂载时能看到消息，会话 idle 时对"库里多了新消息"一无所知（重启后才显示）
- **模型**（用户确认）：SSE 是水管（常驻），循环是抽水机（请求驱动）；发消息/取消/任务通知都是开关；数据库是权威，通道是尽力而为的通知
- **落库即推送**：`BroadcastingSessionManager`（SessionManager wrapper）落库成功后广播 `{type: message, session_id, seq, message}`——一处插桩覆盖所有消息来源（用户消息/工具结果/后台通知/唤醒轮输出/压缩摘要），core 不感知
- **统一事件流**：`GET /api/v1/events` 全局单连接 SSE（事件带 type 区分：message / task_status / command_output），前端按 session_id / task_id 路由
- **任务状态事件**：`BackgroundTaskManager` / `CommandManager` 状态转移点 emit（pending/running/终态）——面板实时更新，不再 3s 轮询
- **命令输出实时视图**：`drain_output` 每读一块 emit 输出增量（尽力而为，日志文件为权威）——终端面板实时刷屏，完整输出经 `GET /tasks/{id}/log` 分页查看
- **任务生命周期守护**：`spawn_background_delegate` 改 watcher 模式（JoinHandle.await 捕获 panic 转 fail + 通知）——任务不再悬死，无需 watchdog
- **前端收敛**：删除唤醒轮询/任务终态感知轮询，改事件驱动；恢复 `mergeServerMessages`（按 id 去重追加合并）
- **回归测试**：e2e 统一事件推送链路（订阅 /events → 落库 → SSE 收到 message 事件）+ mergeServerMessages 追加语义

## 0.3.0 重构（会话时序链模型：存储层完整链 + 组装层压缩点视图，ADR-027）

- **根因**：`load_session_from_store` 在返回链之前做两道截断（compression_marker 截断 + MAX_SESSION_MESSAGES 截断），把完整链变成"工作集"。`delete_message` / `redo_message` 复用 `get_session` 拿到截断后的列表，truncate + rewrite 写回时**压缩点前的历史被永久抹掉**（数据库证据：压缩摘要从 seq=556 变成 seq=0，压缩点前 556 条历史 = 0 条）
- **模型**（用户确认）：会话 = 无分支时序链，节点 = user / assistant / tool / system 四种角色平等；回退 = 锚点（用户输入）之后全部截断，四种节点同等处置；压缩点与异步通知都是 system 节点，回退时一并截断丢弃（失败信息是给大模型看的，不是给用户看的）
- **存储层完整链**：`load_session_from_store` 删除 compression_marker 截断与 MAX_SESSION_MESSAGES 截断；删除 `MAX_SESSION_MESSAGES` / `KEEP_RECENT_MESSAGES` 常量（无硬上限）；`SessionState` 内存态同样不裁剪
- **组装层压缩点视图**：`ContextAssembler::assemble` 从最后一个 compression_marker 开始组装（无压缩点从头）——压缩点截断只发生在组装视图，不污染存储
- **任务时序锚点**：`BackgroundTask` / `CommandTask` 新增 `anchor_seq`（任务启动时链上消息数）；回退时锚点 > 回退点的任务一并取消（委托 cancel + 命令 kill）
- **回退编排**（四种状态统一）：① 停止 LLM 输出（置位 stream_cancel）→ ② 取消时序锚点之后的任务（等待终态，取消通知入库后截断时一并丢弃）→ ③ 回退文件到快照（索引 = 完整链 seq，不再错位）→ ④ 完整链截断 + rewrite
- **回归测试**：组装从压缩点开始 + 无压缩点全量 + 存储层完整链断言更新 + 内存态完整链保留

## 0.2.12 修复（手动压缩会话：无进行中反馈 + 可重复点击 + 摘要不显示）

- **根因**：① 压缩按钮点击后立即关闭详情面板，`compressing` 状态无处展示——整个压缩过程（LLM 摘要生成，通常 10-30 秒）界面无任何反馈；② `compressing` 状态未传给按钮做 disabled，期间可反复点击（服务端虽有 `turn_guard` 串行化 + 消息数守卫兜底，第二次返回"无需压缩"，但体验差）；③ 压缩成功后只弹 toast，**摘要消息不追加到会话流**——摘要已持久化到服务端，但前端 store 不更新，用户看不到压缩点
- **修复**：`ContextRing` 新增 `compressing` prop——压缩期间面板保持打开，按钮就地变为「压缩中...」（spinner + disabled，防重复点击）；`compress_session` 返回摘要消息（`StructuredMessage` → API `ChatMessage`），前端 `addMessage` **追加**到消息流末尾（展示始终只追加、保留完整库历史，不做清空/刷新/截断——服务端上下文组装从压缩点开始与此无关）

- **根因**：通知/注入/唤醒链路正常（System 通知入库 + 唤醒轮触发），但 `prepare_wake_context` 的唤醒指令允许"空输出结束"，模型在后台任务失败场景下选择沉默——主 agent 无任何反馈（构建失败后 5 分钟无反应）
- **修复**：唤醒指令区分失败/完成场景——感知任务状态（`background_tasks.snapshot()` + `command_tasks.list()` 任一 Failed），失败时指令明确"必须向用户汇报失败情况，禁止输出空文本"（ADR-013 shouldReply = allComplete || isTaskFailure 的语义落地）；全部成功且无需输出才允许空输出；空输出日志从 debug 升级为 info（取证可见）

## 0.2.10 修复（会话消耗汇总漏计工具轮输入：流式 usage 事件只下发最终轮）

- **根因**：AgentLoop 每轮 LLM 调用都有真实 usage（完整上下文重发，O(n²) 量级），且服务端持久化完整（每轮 assistant 消息都带 tokens）——但流式事件只有最终轮经 `send_complete` 下发 usage，**中间工具轮的 usage 事件从未下发**。前端本地消息因此只有每轮用户对话的最后一条 assistant 消息带 usage，`sumSessionUsage` 只累加"每轮最后一次 LLM 调用的输入"（如 84,447 + 99,252 = 183,699），而非所有 LLM 调用的输入总量。刷新历史后统计正确（历史加载路径逐消息映射持久化 usage），流式过程中本地累积错误
- **修复**：新增 `StreamEventSender::send_turn_usage`（delta 空、is_complete=false，前端归约器只消费 usage 字段附加到最后一条 assistant 消息，无正文/边界副作用）；`handle_llm_response` 工具轮分支在 persist 后逐轮下发该轮 usage——前端本地消息每轮都带 usage，会话消耗汇总恢复为真实总量（回归测试锁定工具轮事件下发）

## 0.2.9 修复（托盘退出卡死：常驻 SSE 流阻塞优雅关停）

- **根因**：`GET /tasks/stream`（后台任务聚合 SSE，ADR-026）的 forwarder 死等 broadcast 消息，无任何退出条件；前端 `AgentTasksPanel` 用 EventSource 常驻订阅该流（会话存在期间不关闭）。托盘「退出」→ `app.exit(0)` → ExitRequested 钩子 `prevent_exit` 后 `rt.block_on` 等待内嵌 server 优雅关停 → axum `with_graceful_shutdown` 等待所有活跃连接结束 → 常驻 SSE 连接永不结束 → 进程卡死，只能杀进程
- **修复**：`stream_tasks` forwarder 加 shutdown 感知（与 `/chat/stream` 的 `spawn_sse_forwarder` 同模式）——`tokio::select!` 轮询 `shutdown_flag`（1s 间隔），置位即结束流，优雅关停链得以完成（约 1-2 秒内退出）
- **约束**：任何新增常驻 SSE 端点必须带 shutdown 感知，否则阻塞应用退出（已写入 ADR-026）

## 0.2.8 修复（流式 usage 丢失根因：请求未显式要求 + 尾块被丢弃）

- **根因**：0.2.7 的 usage 兜底只是"症状缓解"——真正的根因是两条叠加的链路缺陷：① 流式请求从未发送 `stream_options: {include_usage: true}`（OpenAI 规范：流式响应默认不返回 usage，除非请求显式要求）——ollama 等严格遵守规范的网关因此永远不返回 usage，0.2.7 之前实测"ollama 流式不返回 usage"其实是没向网关要；② SSE 解析把 `{"choices":[],"usage":{...}}` 尾块（include_usage=true 的正常流尾）当作"正常流尾"静默丢弃——即使网关返回了 usage 也会丢掉
- **流式请求显式要求 usage**：`chat_completion_stream` 构造请求时设置 `stream_options: {include_usage: true}`（对齐 DSH 同名参数，回归测试锁定请求体含该字段）
- **usage-only 尾块保留下发**：SSE 解析识别 `{"choices":[],"usage":{...}}` 块后构造带 usage 的空 chunk 下发（loop 收到后更新 `last_input_usage` 校准值），不再静默丢弃；纯 cost 块（兼容层 `{"choices":[],"cost":"..."}`）、无 choices 无 usage 的空块仍正常跳过（回归测试锁定两种形态）
- **思考内容正确回传**（DeepSeek 官方要求）：携带 tools 的请求必须完整回传 `reasoning_content`（即使该轮未实际进行工具调用，官方 thinking_mode 文档）——`convert_messages` 此前用 `..Default::default()` 构造 assistant 消息（async-openai 0.34 无此字段），序列化后被静默丢弃；改为序列化后按索引回填（`inject_reasoning_content`，非流式与流式统一走 JSON 层）；`assembler` 去掉 `has_tool_calls` 条件无条件保留 reasoning（回归测试锁定无工具调用轮也保留）
- **估算计入思考与工具参数**：`TokenEstimator.estimate_message` 此前只算 `message.content`，漏掉 reasoning_content 与 tool_calls 参数——思考十几万字的会话被低估（5% 显示的根因之一）；估算纳入思考内容与工具调用参数（回归测试锁定），无 usage 兜底时占用显示包含思考
- **换模型旧实测值失效**：`last_input_usage` 从裸数字升级为 `(model, tokens)` 配对——同一会话切换 provider/model 后旧实测值不再适用（不同模型分词/计费口径不同），退回全量估算（对齐 DSH token-meter：header 不匹配时全量重估）（回归测试锁定）

## 0.2.7 修复（ollama 网关兼容：思考过程不显示 + 上下文圆环无数据）

- **根因**：聊天模型切换到 ollama 提供商（deepseek-v4-flash:0731）后，两个功能同时失效——ollama 兼容层流式响应与 opencode 网关存在两处协议差异：思考字段名不同（`delta.reasoning` vs `delta.reasoning_content`）、流式 chunk 不携带 usage（非流式才返回）
- **思考过程兼容**：`DeltaContent.reasoning_content` 加 serde alias `reasoning`，统一解析 ollama 与 DeepSeek 两种字段名（回归测试锁定）
- **流式 usage 兜底**（对齐 DSH token-meter 启发式）：网关不返回 usage 时用 TokenEstimator 估算（prompt 优先用上次实测值，completion 按正文+思考估算）——上下文占用/缓存命中展示与压缩判定不因网关差异而失效；真实 usage 存在时不受影响

## 0.2.6 修复（后台任务与子智能体统一面板，ADR-026）

- **委托只支持异步**：delegate_to_agent 删除同步分支——所有委托一律注册后台任务 + 立即返回 task_id（同步预算/超时提升逻辑删除），数据源统一、与 ADR-013 唤醒语义完全对齐
- **右侧任务面板（统一展示）**：后台任务与子智能体委托同一套展示机制——活跃在上、完成沉底、可展开、可取消、可折叠；委托任务展开 = 子智能体消息流（历史 + SSE 实时增量，渲染复用主对话流，视觉完全一致）；终端任务展开 = 输出尾部
- **子智能体 = 带父会话引用的会话**：子智能体消息流走会话存储（session_meta 加 parent_session_id，幂等迁移），逐轮落库（不进 FTS——子会话无"人"提供的信息，回忆检索天然排除，索引不膨胀）；面板展开复用 GET /sessions/{id}/messages 加载历史
- **子智能体过程实时可见**：GET /tasks/stream 聚合 SSE 端点（复用 ChatStreamEvent 协议 + task_id 归集），面板实时滚动显示子智能体思考/工具调用/中间输出
- **并发可配置**：[agent] 新增 max_background_concurrency（委托同时运行，默认 20）、max_background_queue（委托排队上限，默认 40，双信号量排队模型——排队满才拒绝）、max_command_concurrency（终端命令同时运行，默认 16）
- **数据有界**：后台任务注册表改 SQL 权威（内存只留活跃/排队任务，终态落 SQLite）+ 3 天 TTL 逐出；主会话删除级联删子会话；子会话总数超 300 惰性清理最不活跃主会话的子会话
- **修复后台任务并发控制失效**：原实现并发许可在 spawn 后立即释放（信号量形同虚设）——许可随任务闭包持有，任务结束释放

详见 [ADR-026](../architecture/decisions/026-background-tasks-unified-panel.md)。

## 0.2.6 修复（自演化综述可靠性 + 任务失败可见性 + 默认工作区常显）

- **修复自演化综述从未产出过任何产物的根因**（8/19、8/31 两次运行均无演化报告/规则/记忆落盘，任务以"综述输出解析失败：综述输出中未找到 JSON"告终）：综述智能体偶尔以散文回复（如 24 token 的"本周期无需变更"）或把 JSON 写入剪贴板工具而非回复文本，严格 JSON 解析即整体失败。三层修复：
  - `parse_llm_json` 增加平衡花括号提取回退——前言/后记/围栏夹杂的混排文本中正确提取首个 JSON 对象（字符串内花括号与转义引号正确跳过），旧契约"前言 + JSON 返回 None"正是本次失败形态之一
  - 解析失败降级为空计划（warn 日志 + 综述原文首行截断进摘要）：演化报告与水位线照常落盘、任务不再整体失败，失败可回溯、下周期可续跑（回归测试锁定）
  - 综述提示词明确禁止用工具输出结果，最终回复必须直接是 JSON
- **任务失败可见性**：调度器 `run_count` 此前无条件 +1 且失败信息只进日志——洞察页显示"自演化综述 1 次"实际可能是失败。`TaskStatus` 新增 `last_error`（持久化，跨重启可见），洞察页任务行显示失败图标 + 错误摘要（hover 看全文）；新增 `executing_task_id`（当前正在执行的任务），页面显示"执行中"徽标——消除"启动后看到摘要生成执行了 1 次而自演化综述没动"的错位感（实为串行执行中，综述尚未完成计数）
- **洞察页表头纠错**："Cron 表达式" → "执行间隔"（ADR-024 改间隔制后的遗留错误标签）
- **移除检索轨迹功能（ADR-025 / REJECTED #20）**：只覆盖上下文组装路径（同 query 的规则/记忆两条轨迹近乎相同，呈现为"重复记录"）、不覆盖 agent 主动的 `search_vfs` 路径、整个生命周期零有效使用。移除 `RetrievalTrace` 类型/构建器/表/`GET /retrieval/traces`/GUI 面板与侧边栏入口；保留 UsageStats（搜索热度/文档命中）与 tracing 检索日志
- **默认工作区始终显示**：会话列表默认分组（未绑定工作目录）此前在无会话时整体消失，作为工作区归属锚点应常显（空组可 hover「＋」直接新建会话）

详见 [ADR-025](../architecture/decisions/025-remove-retrieval-traces.md) 与 [REJECTED #20](../architecture/decisions/REJECTED.md)。

## 0.2.5 修复（todo 工具批量语义 + 完成条目保留展示）

- **批量操作**：todo 工具 create 接受 todos 数组一次写整份清单（不再逐条调用）；update 接受 updates 数组一次合并多条状态变更（开始/完成多条一次调用）；delete 接受 ids 数组；单条形态保留兼容。存储层 create_many/update_many/delete_many 单次落盘，update 原子（任一 id 不存在整体不上盘）
- **子待办**：parent_id 挂靠父待办（校验父存在且属于本会话），面板缩进展示——发现新工作/拆子任务随时追加
- **修复「todo 不存在或不属于当前会话」误报根因**：存储懒加载竞态——create/update/delete 不触发磁盘加载，新进程首个 create 会把既有条目全部覆盖；读盘移入写锁内双重检查（ensure_loaded）
- **修复后台终端命令对面板不可见**：execute_command(background) 启动的后台命令（cmd_*）只存在于 agent 内部注册表（task_status 可查），「后台任务」面板读的是委托任务注册表——实测"明明有后台实验在跑，面板却空空如也"。合并两套注册表为统一视图（BackgroundTask::from_command_task），面板显示终端/委托类型徽标，取消按钮对终端命令走进程树终止
- **修复整点定时任务触发的整进程闪退**（8/29 20:00、8/30 20:00、8/31 14:00 三次，WER BEX64 fail-fast）：根因链 = 摘要任务（cron 0 0 */6 * * *，UTC 语义 = 本地 8/14/20/2 点）发现待摘要条目 → 嵌入请求未携带 dimensions 参数（API 按模型默认 1024 维返回）→ 写入 LanceDB 时与配置 vector_dimension(3072) 的建表维度不一致 → arrow FixedSizeList 内部 panic → release panic=abort 整进程即刻终止且日志无任何输出。修复：嵌入路径按向量库建表维度显式请求 dimensions（index_entry 与查询路径，VectorStorage 新增 embedding_dim 声明）；upsert/查询前置维度校验，不匹配转为带行动指引的干净错误（回归测试锁定），杜绝此类 panic 再次发生
- **调度模型从 cron 改为间隔 + 补跑（ADR-024）**：个人 PC 服务不常驻，cron「某时刻执行」宕机即永不执行且 UTC 语义造成本地触发时刻漂移。改为间隔制——任务声明执行间隔，单一扫描循环每 60 秒顺序检查，「距上次执行 ≥ 间隔」立即执行；last_run 持久化（scheduler_state.json），宕机超期任务重启后自动补跑一次；全任务串行消除资源争抢；顺带修复边界双跑 bug（每任务在整点 ±1s 各执行一次）。用户自建定时任务同步迁移（旧 cron 自动换算），schedule_task 工具与「定时任务」创建表单改为间隔语义
- **完成条目保留展示**：面板不再过滤已完成条目——划线 + 对勾保留展示（可回看），无任何条目后面板才消失
- **面板宽度对齐输入框**（max-w-4xl 居中，与 composer 一致）

详见 [ADR-022 修订](../architecture/decisions/022-session-bound-task-ux.md)。

## 0.2.4 修复（流式静默中断，断流根因排查）

- **根因确认**：断流为 opencode 网关→GLM 上游连接中断（网关以非标准 finish_reason=network_error 结束流），DSH 同模型同供应商同样复现——非天演独有；但天演此前叠加了两个自身问题使断流"静默化"
- **客户端 120s 总超时（天演独有断流源）**：provider timeout 曾作为 reqwest 总请求时长限制，长思维链轮次超 120s 即被客户端掐断 → 改为读空闲超时（字节间静默才计时），活跃流总时长不受限
- **network_error 结尾块被静默丢弃**：非标准 finish_reason 反序列化失败即跳过 → 显式上抛并携带原始原因（与 DSH 报错可见性对齐）
- **提示只闪 3 秒**：中断标记补全，消息下方持久的红字"流式中断，已保留部分输出"提示
- **文件日志 0 字节**：文件层与 RUST_LOG 解耦，GUI 实例日志不再静默归零
- **输出上限链路确认**：设置页模型 output 限额（max_output_tokens）经 T6 动态预算正确生效（请求 max_tokens = min(配置值, 上下文预算)），前端 2048 参数不参与

## 0.2.3 修复（数据目录搬迁重做，ADR-023）

- **配置目录与数据目录分离**：配置文件固定为 `~/.tianyan/tianyan.toml`（不再多路径搜索，`TIANYAN_CONFIG` 环境变量仍可覆盖）；数据目录从配置 `storage.data_dir` 读取，两者彻底解耦
- **搬迁语义修正（真"搬"）**：搬迁忽略配置文件（历史安装中配置与数据同目录时，配置留在原地）；复制后逐文件校验（路径集合 + 字节大小）；**强制删除源数据条目**——被占用文件作为无害残留报告，绝不静默留下两份全量数据
- **顺序修正**：先更新配置再删除源（旧实现的"先删源再改配置"曾导致配置文件被自己搬走、更新失败、整体回滚——正是 0.2.2 中"搬迁后目录显示不变、两份数据并存"的根因）
- **失败可见**：搬迁后前端比对重启后配置的 data_dir 与所选新目录，回滚时明确报错而非误报成功

## 0.2.2 修复（0.2.0 验证反馈；0.2.1 因打包嵌入旧前端资源作废）

1. **数据目录搬迁交互**：数据目录改只读展示；「数据搬迁」按钮 → 对话框选择新目录（原生目录选择器/手动输入回退）；确认后系统校验目标目录为空（非空就地报错，不触发关停）再开始搬迁
2. **移除「计划」全局栏目**：todo/goal 是会话内推理辅助工具——数据按会话绑定，会话页输入框上方停靠展示活跃条目，完成即隐（对齐 DSH）；删除会话级联清理
3. **移除「任务」混合栏目**：内置任务并入「洞察」；定时任务独立一级栏目；后台任务移入会话页停靠条（运行中可取消，完成即隐，结果由主 agent 汇总进会话流）

详见 [ADR-022](../architecture/decisions/022-session-bound-task-ux.md)。

## 0.2 新增功能

### 会话待办/目标（todo/goal，会话绑定）
- **待办清单**：创建/完成/删除/优先级/关联目标，持久化 `todos.json`
- **目标**：长期目标，进度按关联待办完成比例自动计算（与 todolist 联动）
- **会话绑定 + 临时语义**（对齐 DSH）：todo/goal 是智能体的会话内推理辅助工具——数据按归属会话存储（`session_id`），会话页输入框上方停靠展示当前会话的活跃条目，全部完成后面板消失；**无全局「计划」栏目**；删除会话级联清理其 todo/goal

### 数据目录搬迁
- 设置页「数据存储」：数据目录**只读展示** + 「数据搬迁」按钮 → 对话框选择新目录（桌面端原生目录选择器 / 浏览器回退手动输入）→ 确认后系统校验目标目录为空（非空 400 就地报错，不触发关停）→ 自动完成（关 DB → 复制数据 → 更新配置 → 重启）
- 修复优雅关停循环引用（agent → 工具 → 调度器 → 统计 → SQLite），文件锁得以解除
- 失败自动回滚，数据不丢失

### Web 搜索配置 UI
- 设置页新增「Web 搜索」tab：后端选择（DuckDuckGo/Bing/SearXNG）+ SearXNG 端点 + 超时/缓存/抓取上限
- 配置向导新增「Web 搜索」步骤

### 待办/目标工具化
- 新增 `todo`/`goal` 动态工具：agent 可自主创建/更新/跟踪/修复待办与目标（会话绑定：创建归属当前会话，list/update/delete 只作用于本会话条目）

### 任务语义分层（0.2 修复后）
- **内置任务**（调度器 cron：摘要/GC 等）→ 在「洞察」栏目展示
- **定时任务**（到点调用智能体的周期 AI 工作）→ 独立一级栏目「定时任务」
- **后台任务**（会话发起的委托/终端任务）→ 会话绑定，在会话页停靠条展示（运行中可取消；完成即隐，结果由主 agent 汇总进会话流，ADR-013）

## 架构重构（0.2 之后，2026-08-30）

### 循环引用彻底修复（分层 + 解耦）
- 定时任务链：handler 经 `TaskResultSink` 接口回写（Weak）、`ScheduleTaskTool` 经 channel 解耦（不依赖 manager 类型）、manager 经 `TaskRegistrar` 接口注册——依赖单向向下，**不再依赖优雅关停特殊处理**（迁移验证旧目录完全清空）

### 分层重构（阶段 1-3）
- **基础类型层**：`roles`（角色纯类型）、`common`（含 RetrievalTrace）——config/agent/scheduler 共用，打破 `config↔agent`、`scheduler↔agent` 环
- **统一写入门面**（ADR-020）：`db::Database` 门面（单连接 + schema 集中）+ 业务域 Repository（stats/trace/execution/usage）；8 个组件不再各自持 SqliteDb
- **SQL 收敛**：Session/Stats/Trace/Execution/Usage 五域数据访问收口到 Repository/本模块（会话专属存储归 ADR-018）
- **db 纯底层**：SqliteDb 移入 db、SessionRepo 归位 session、RetrievalTrace 引用 common——**生产代码零模块环**（文件级 SCC = 0）
- **依赖清理**：cargo-machete 移除 7 个未使用依赖（core: anyhow/clap/diffy；server: tracing-subscriber；tauri: tower/tower-http/uuid）

## 0.1 主要功能（回顾）

### 智能体
- **对话与工具调用**：流式响应、思考过程、工具调用（文件/代码/搜索/网络/委托/定时任务）
- **编辑链路**：`apply_patch`（主力，unified diff + 上下文锚定，对齐 omo/Codex）+ `apply_edit`（内容匹配，极小改动）
- **子代理委托**：并行/嵌套委托（深度上限 3）、后台任务、完成通知
- **定时任务**：真实 cron 语义，可安排周期性工作

### 知识体系（VFS）
- **统一存储**：文档/记忆/规则/技能全部经 VFS（L0/L1/L2 双层摘要 + 向量检索）
- **自动注入**：soul/rules/memories 前缀按查询检索注入（会话首轮冻结，命中前缀缓存）
- **按需检索**：`search_vfs` 语义搜索整个 VFS（文档/记忆/规则/技能）
- **自进化**：技能/规则随使用自动学习（GEPA）

### 桌面应用
- **托盘常驻**：关窗最小化到托盘，后台任务继续运行；`Ctrl+Alt+T` 唤起
- **系统通知**：后台任务完成/审批挂起/主动提醒 → 桌面通知
- **剪贴板**：监听捕获 + agent 写出
- **自动更新**：启动检查 GitHub Releases，用户确认后安装

### 工作区
- 文件树/查看器/编辑、diff 面板、LSP 诊断、测试发现、构建验证

## 已知问题

见 [known-issues.md](known-issues.md)。重点：
- 模型生成截断工具调用（已护栏）
- apply_edit 精确匹配脆弱（引导用 apply_patch）
- 会话内规则/记忆不刷新（ADR-012 缓存优先，可用 search_vfs 检索）

## 安装与使用

### 桌面应用（Tauri）
1. 从 GitHub Releases 下载安装包（`Tianyan_<版本>_x64_zh-CN.msi` / `Tianyan_<版本>_x64_en-US.msi`）
2. 首次运行 SmartScreen 警告时点"仍要运行"（安装包**未做代码签名**；updater 的 `.sig` 是 Tauri 更新签名，与 Windows 代码签名无关）
3. 启动后自动检查更新

### 本地服务（开发模式）
```powershell
cargo run -p tianyan-server   # 后端 http://127.0.0.1:3000
cd gui-vite && npm run dev    # 前端 http://localhost:5100
```

### 配置
- 配置文件：`tianyan.toml`（模型 Provider/Model、存储、安全、日志、Web 搜索）
- 数据目录：`~/.tianyan`（SQLite 会话 + VFS 内容 + 快照）

## 发布流程（维护者）

1. 更新版本号（workspace Cargo.toml / tauri.conf.json / gui-vite package.json）
2. 推送 `v0.2.0` 标签触发 GitHub Actions 发布流水线
3. 流水线产出 `.msi` + `.sig` + `latest.json` 上传到 GitHub Release
4. 用户启动应用时自动检查更新

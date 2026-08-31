# 天演 Tianyan 0.2 发布说明

> 0.2.0 发布日期：2026-08-30 · 0.2.2/0.2.3 修复日期：2026-08-30 · 0.2.4 修复日期：2026-08-30 · 0.2.5 修复日期：2026-08-31

天演（Tianyan）是一个本地优先的通用智能助手：桌面应用（Tauri）+ 本地服务（Axum）+ 自进化智能体（VFS 统一知识/记忆/技能/规则）。

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
1. 从 GitHub Releases 下载安装包（`tianyan_0.2.0_x64.msi`）
2. 首次运行 SmartScreen 警告时点"仍要运行"（自签名）
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

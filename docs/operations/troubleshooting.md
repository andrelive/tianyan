# 天演运维排障手册（troubleshooting）

> 适用版本：1.1.0（撰写起点 0.5.2 / HEAD `732e44c`；2026-11 按 native 客户端重校准日志与端口两节）
> 面向：本机/内网部署天演的维护者与 QA。所有路径、命令、查询均取自当前代码，未核实项显式标注 **待核实**。
> CI / GitHub Actions 流水线自身的问题（挂死、日志丢失、编译指纹）见 [`ci-pipeline.md`](ci-pipeline.md)。

---

## 0. 取证入口一览

| 取证对象 | 位置 | 说明 |
| --- | --- | --- |
| 运行时日志（桌面端） | **默认没有文件**：`[logging].file` 未设时只走控制台，而 release 包无控制台 | 要取证就先给配置加 `[logging].file`（见 §1.1），再按 `max_file_size` / `max_files` 轮转 |
| panic 日志（桌面端） | `%APPDATA%\com.tianyan.app\logs\panic.log` | `panic_hook::install` **恒落盘**（`dirs::data_dir()` = Roaming，不在数据目录下）；崩溃/闪退先看它 |
| SQLite 权威数据 | `<data_dir>\tianyan.db`（+ `-wal`/`-shm`） | 默认 `data_dir = %LOCALAPPDATA%\tianyan` |
| 向量库 | `<data_dir>\lancedb\` | LanceDB，表名 = `storage.vector.collection_name` |
| 统一事件流 | `GET /api/v1/events`（SSE 单连接） | 前端 EventSource 常驻，`http://127.0.0.1:3000` 起 |
| 事件订阅（快照恢复） | `POST /api/v1/events/subscribe` | 打开/重连时推历史快照 + cursor |
| 任务日志（命令输出权威） | `GET /api/v1/tasks/{id}/log?offset=&limit=` | 分页读取，`limit` 默认 64KB、上限 1MB |
| 配置文件 | `~/.tianyan/tianyan.toml`（可用 `TIANYAN_CONFIG` 覆盖） | 固定位置，**不随数据目录搬迁** |

---

## 1. 日志在哪（三条通道，别找错地方）

### 1.1 桌面端运行时日志：默认不落文件

`gui-native/src/main.rs` 在起 server 之前就调用 `tianyan::common::logging::init_logging(&config.logging)`，与后端**同一套**实现：

- 控制台 layer **恒装配**；文件 layer **只在 `[logging].file` 有值时装配**（`core/src/common/logging.rs::init_logging`）。
- `LoggingConfig::default().file = None`，`config.example.toml` 也没有这个字段 ⇒ **默认没有运行时日志文件**。
- release 包是 GUI subsystem（`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`）⇒ **没有控制台**，stdout 无处可去；debug 版从终端启动才能看到 stdout。

所以要拿运行日志，给 `~/.tianyan/tianyan.toml` 加一段：

```toml
[logging]
level = "debug"
file = "D:\\tianyan\\logs\\tianyan.log"
```

之后按 `[logging].max_file_size`（默认 10 MB）轮转出 `tianyan.1.log`…，按 `max_files`（默认 5）保留最新 N 个；文件族前缀 = `file` 的文件名主体。

### 1.2 panic 日志：唯一默认落盘的东西

`core/src/common/panic_hook.rs::install` 把 panic 报告 append 到 `%APPDATA%\com.tianyan.app\logs\panic.log`（`dirs::data_dir()` = Roaming，**不在数据目录下**），目录不存在时尽力创建、写入失败静默。

桌面端崩溃、闪退、窗口没出来 —— **先看这个文件**，再看 §3 的 DB。

### 1.3 独立后端（`cargo run -p tianyan-server`）

`app/src/main.rs` 同样调 `init_logging`：

- 前台运行，控制台输出可见；
- 配了 `[logging].file` 时同时落文件（轮转规则同 §1.1）；
- 仓库根偶尔出现的 `server.log` / `server.err.log` 是**手动重定向**产物（`> server.log 2>&1`），不是应用自己写的。

### 1.4 日志级别

- `RUST_LOG` 优先于 `[logging].level`（`EnvFilter::try_from_default_env()`），非法值回退配置级别。
- 两个 layer 共用同一个全局过滤器 ⇒ 把 `RUST_LOG` 设成 `error` **文件里也只剩 error**（旧版 tauri 壳曾让文件层忽略 `RUST_LOG`，该实现已随壳删除）。

### 1.3 独立后端的日志（与桌面端不同）

独立启动（`cargo run -p tianyan-server`）走 `server/src/main.rs` → `tianyan::common::logging::init_logging`：

- 该函数装配控制台 subscriber；配置了 `[logging].file` 时**同时装配文件 layer**（0.5.x 接线，
  此前该字段未被使用）——文件按同一套 `max_file_size`/`max_files` 轮转，文件族前缀 =
  文件名主体（如 `tianyan.log` 与轮转出的 `tianyan.1.log` 同族）。
- 未配置 `[logging].file` 时独立后端**没有文件日志**；仓库根目录的 `server.log` /
  `server.err.log` 是**手动重定向**的产物，不是应用自身写的。

---

## 2. 高频故障排查路径（四类）

每条按「现象 → 查什么 → 判定 → 处置」给出。

### 2.a 唤醒轮无输出（历史事故：全 system 请求 → 云 API 空输出）

**现象**
后台任务/命令完成后，主 agent 没有任何汇总输出；会话里落了一条**空 assistant 消息**
（`parts=[]`、`tokens=0`、`finish="load"`）。前端看不到、需要退出重进才可能察觉。

**机制（四步，来自 `.scratch/wake-no-user-finding.md`，0.3.10 现场定位）**

1. **压缩点恰好落在通知前** → 组装视图 = “摘要(system) + 通知(system) + 唤醒指令(system)”，
   没有任何 user 消息（`context/assembler.rs` 从最后一个压缩点开始组装）。
2. Ollama 云对“无 user 消息”的请求返回 `finish_reason="load"` + 空内容、0 token
   （直连实测 4 组对照：**无 user = load；补 1 条 user 正常生成**）。
3. loop 对空响应重试 1 次（相同上下文 → 相同 load），`process_wake` 把空输出当“无需回复”静默完成。
4. 历史同款：9/10 的 `49ee` 会话 5 条 load 空消息；本会话 `seq=248`。

**查什么**

- 已配 `[logging].file` 时，搜关键词 `LLM 返回空响应，重试一次`、`唤醒轮空输出`
  （该事故的原始证据是 tauri 时代的文件日志 `tianyan_20260912_124231.log` 05:07:18–05:07:22；未配 `file` 时只能靠下面的 DB 查询）。
- DB 查最近消息的 `finish` / tokens：

  ```sql
  SELECT seq, role, tokens, json_extract(content_parts,'$.finish') AS finish,
         length(content_parts) AS bytes
  FROM session_messages
  WHERE session_id = '<会话 id>'
  ORDER BY seq DESC LIMIT 8;
  ```

  出现 `finish='load'` 且 `tokens=0`、`bytes` 极小的 assistant 消息即为现场。
- 对应用量行（应为全 0）：

  ```sql
  SELECT * FROM usage_logs WHERE session_id='<会话 id>' ORDER BY id DESC LIMIT 5;
  ```

**判定**
空输出消息恰好紧跟一个压缩点（`content_parts LIKE '%"compression_marker":true%'`），
且前后组装视图缺少 user 角色锚点 → 命中本故障。

**处置**

- **主修（0.3.13 已落地）**：压缩摘要改为 **user 角色锚定**（组装层按 `compression_marker`
  统一归一，旧数据 system→user）。确认版本 ≥ 0.3.13；旧会话（含既有压缩点）立即受益，无需等新压缩。
- **加固**：`finish=load` / 完全空响应不再静默 → 异常化（走唤醒重试，耗尽后前端可见错误）。
- 临时规避：手动给该会话发一条 user 消息再触发唤醒轮。

---

### 2.b 前端面板不实时（事件推送尽力而为 vs 落库权威）

**现象**
后台任务/命令**已完成**，但面板要“退出重进”“切换会话再切回”才显示通知或唤醒轮输出。

**模型（ADR-028/029/031）**

- **数据库是权威**，SSE 只是“水管”（尽力而为的通知）；落库成功后**补发边界事件**
  （`BroadcastingSessionManager::add_structured_message` 对 System 角色消息构造 `chat_stream`
  边界事件，推入统一通道）。
- 前端 EventSource 是**应用级常驻**（进程存活期不关闭），唤醒轮/主对话/子代理共用同一入口。

**查什么**

- 确认唯一事件流端点可达：

  ```powershell
  # 统一事件流（SSE，常驻；Ctrl+C 退出）
  curl.exe -N http://127.0.0.1:3000/api/v1/events
  ```

  正常会持续收到带 `type`（`message` / `task_status` / `command_output` / 子代理消息）的事件。
- 打开/重连时走 **快照**（权威兜底）：

  ```powershell
  curl.exe -X POST http://127.0.0.1:3000/api/v1/events/subscribe `
    -H "Content-Type: application/json" -d "{`"session_id`":`"<会话 id>`"}"
  ```

  返回 `{"status":"ok"}`，并把 `snapshot`（完整历史 + `cursor`）推入同一条流。
- **任务日志文件**是命令输出的权威，面板看不到时用它核对：

  ```powershell
  curl.exe "http://127.0.0.1:3000/api/v1/tasks/<task_id>/log?offset=0&limit=65536"
  ```

**判定**

- 若 `GET /events` 有事件、但前端面板不动 → 前端订阅/归约器问题（关注是否 EventSource 被意外关闭，
  如切到设置页时旧实现在组件卸载即 `stopUnifiedEvents()`；0.3.5 起已改为常驻）。
- 若 `GET /events` 无事件、但快照 `subscribe` 能拿到消息 → 该消息未触发边界事件推送
  （检查是否为 System 角色 / 压缩点，是否走了 `add_structured_message` 推送路径）。
- 若 `events` 与快照都无、但 `/tasks/{id}/log` 有内容 → 只是**终端输出增量**丢失（尽力而为）；
  完整输出以日志文件为准，可放心以文件为准。

**处置**

- 确认版本 ≥ 0.3.5（事件常驻）/ ≥ 0.3.7（后台通知实时）/ ≥ 0.3.9（唤醒轮实时性、转发死锁修复）。
- 前端面板不动而库里已更新：以 DB / `/tasks/{id}/log` 为权威判断“任务到底有没有做完”，
  不要把“界面没刷”误判成“任务没跑”。

---

### 2.c 压缩后报错 / “预算不足 1024”

**现象**
压缩（手动或自动）之后继续对话直接报错：

```
上下文预算不足（不足 1024 tokens）
```

且该会话此后**每条消息必现**（会话卡死）。

**根因（0.3.14 修复，双叠）**

1. **double count**：`run_turns` 实测输入恢复为 `tokens.input + tokens.cache.read`
   —— `input` **已含缓存命中**，`cache.read` 是其**子集**，相加使判定值 ≈ 真实 × 2。
   事故实测：压缩前 `input=593399` / `cache.read=592128` → 判定 `1185527` 直接越过 1M 窗口 →
   `dynamic_max_tokens` 返回 `None` → 请求根本不发出。
2. **恢复范围未按压缩点截断**：压缩后组装视图已从压缩点重新开始（实际仅 ~1.7 万），
   预算却仍按压缩前的旧实测计算。

**取数口径（单点，务必记住）**

```rust
// core/src/common/types/structured_message.rs
pub fn prompt_side_tokens(&self) -> usize {
    self.tokens.input.max(self.tokens.cache.read)   // 取 input，max 兜底旧数据
}
```

- **不得**写成 `input + cache.read`。
- 压缩判定与实测输入恢复**共用**该函数（`agent_core::maybe_compress_and_persist` 与
  `agent::loop` 同源）。
- **压缩点自身不参与**占用统计——它的 tokens 是摘要请求（压缩前上下文重发）的用量。

**压缩阈值与作用域**

- 自动压缩默认阈值 **窗口 60%**（0.3.14 从 50% 提到 60%）。
- 压缩点判定只看**最后一个 `compression_marker` 之后**的消息（组装视图同样从压缩点开始）。
- `dynamic_max_tokens`：无实测时退化为估算；**剩余预算不足 1024** 时提前报错。

**查什么**

```sql
-- 找最后一条带实测 usage 的消息（取 prompt_side_tokens 的来源）
SELECT seq, role,
       json_extract(content_parts,'$.tokens.input')     AS input,
       json_extract(content_parts,'$.tokens.cache.read') AS cache_read
FROM session_messages
WHERE session_id='<会话 id>'
ORDER BY seq DESC LIMIT 10;
```

**判定**
若存在 `input` 与 `cache_read` 同时很大、且二者之和超过该模型 `context_length` 的消息，
说明命中 double-count 口径。修复后剩余预算应按 `input`（max `cache.read`）计算。

**处置**

- 确认版本 ≥ 0.3.14（口径修复 + 阈值 60%）。
- 已卡死的旧会话：升级后新请求按修复后口径重算；若仍异常，检查模型 `context_length`
  （与 `storage.vector` 无关——向量维度是另一回事，见数据体检文档）。
- 回归证据：`core/src/agent/loop_tests.rs` 的“预算不足 1024”事故复现测试（`input=593_399`）。

### 2.d 终止后前端显示「上一轮的回答」

**现象**：在输入框打字（内容有误）后点击「终止/停止」，前端把**上一轮的 assistant
结论**渲染到本轮位置——内容明显不属于本轮。

**根因（0.3.17 修复）**：服务层在流式轮结束后**无条件**补发 assistant 消息边界事件，
而该事件取的是「会话中**最后一条** assistant 消息」。取消/失败轮根本不落库 assistant
消息，于是取到的是**上一轮**的消息；前端把它合并进本轮的占位气泡（覆盖
segments/thinking/usage），于是陈旧内容显示在当前轮。

**修复位置**：`server/src/api/chat/services.rs`

- 轮前记录水位：`last_assistant_message(session.messages).id` → `prev_assistant_id`；
- 边界事件只下发 `select_turn_assistant(messages, prev_assistant_id)`——即**本轮新产生**
  的 assistant 消息；本轮没有新消息则**不下发**（取消/失败轮静默）；
- 回归测试：`api::chat::services::tests::test_select_turn_assistant_*`
  （取消场景断言返回 `None`；反向验证：恢复旧行为必红）。

**排查入口**（旧版本再遇到时）：

1. 查该轮是否落库 assistant 消息——`session_messages` 按 seq 倒序看最近两条
   （取消轮只会看到用户消息）；
2. 比对前端展示的消息 id 与上一轮 assistant id 是否相同（相同即命中此 bug）。

**前端加固（2026-10-09 已实施）**：`applyServerMessage` 合并边界事件时**校验消息
归属本轮** —— 实现用 `turnState[sessionId].auto`（用户轮 `false` / 唤醒轮 `true`）：

- **auto 轮（唤醒轮 / 后台轮）的边界事件不得替换"用户轮的流式占位"**：两者是
  不同轮次、不同消息，按位置反查"最后一个无 id 的 assistant"必然跨轮错配 ——
  唤醒轮消息（有 id、且自带 `segments`）覆盖用户轮占位后，会把占位已累积的
  **`segments`（thinking 段 / tool 段）** 抹掉；三者全空时渲染层 `MessageBubble`
  直接 `return null`，表现为**该会话的思考块与工具卡片整条消失**。
- **触发场景**（用户报告）：**任务通知与"正在运行"的会话重叠**时。只有运行中的
  会话末尾存在"无 id 的流式占位"，故**只有它受影响**；覆盖发生在轮结束（或切回
  重建）时，故**输出过程中看得见、切回后不见**。
- auto 轮改走下方"按 id 幂等追加/更新"路径（唤醒轮输出是**独立新消息**）。
- 修复位置：`gui-vite/src/lib/store/chat-slice.ts`（`applyServerMessage` 的
  assistant 分支）；回归测试：`gui-vite/src/lib/__tests__/chat-stream.test.ts`
  （`唤醒轮边界事件不得覆盖用户轮的流式占位` + `用户轮（auto=false）…` 对照用例；
  **判别力已验证**：临时停用修复则该用例必红）。
  > 注：本节的 0.3.17 修复在**服务端**（只下发本轮新产生的 assistant）；唤醒轮的
  > 边界事件在服务端看来是"合法的新消息"，服务端侧无法拦住 —— 故这一层加固必须
  > 落在前端，两者互补。

---

## 3. DB 只读取证入口

### 3.1 路径

- 默认：`<data_dir>\tianyan.db`，其中 `data_dir` 默认 `%LOCALAPPDATA%\tianyan`。
- 可被 `storage.sqlite_path` 覆盖；`data_dir` 可被 `TIANYAN_DATA_DIR` 环境变量或配置覆盖。
- 打开参数：`PRAGMA journal_mode=WAL; synchronous=NORMAL; foreign_keys=ON;`
  → 运行期存在 `tianyan.db-wal` / `tianyan.db-shm`，**只读打开不会影响正在运行的服务**。

### 3.2 只读打开方式

```powershell
# 方式一：sqlite3 CLI（只读）
sqlite3 -readonly "$env:LOCALAPPDATA\tianyan\tianyan.db"

# 方式二：Python（推荐，便于脚本化）
python -c "import sqlite3; c=sqlite3.connect(r'file:%LOCALAPPDATA%\tianyan\tianyan.db?mode=ro',uri=True); print(c.execute('select count(*) from session_messages').fetchone())"
```

### 3.3 常用查询示例

**列全部表**

```sql
SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;
```

**某会话最近消息（含 finish / 角色）**

```sql
SELECT seq, role, message_id, tokens,
       json_extract(content_parts,'$.finish') AS finish,
       length(content_parts) AS bytes
FROM session_messages
WHERE session_id='<会话 id>'
ORDER BY seq DESC LIMIT 20;
```

**用量（按 provider/model 聚合）**

```sql
SELECT provider, model,
       SUM(uncached_input) AS uncached, SUM(cached_input) AS cached,
       SUM(completion_tokens) AS completion, SUM(total_tokens) AS total, COUNT(*) AS calls
FROM usage_logs
GROUP BY provider, model ORDER BY total DESC;
```

**任务表（后台任务/命令）**

```sql
SELECT id, kind, status, parent_session_id,
       datetime(created_at/1000,'unixepoch','localtime')   AS created_local,
       datetime(completed_at/1000,'unixepoch','localtime') AS completed_local
FROM background_tasks ORDER BY seq DESC LIMIT 20;
```

> 时间列口径：`session_messages.ts`、`background_tasks.created_at/completed_at`、`usage_logs.ts`
> 均为**毫秒 epoch**；文本时间列（`recorded_at` / `updated_at`）由 SQLite `datetime('now')` 产出
> （UTC，`YYYY-MM-DD HH:MM:SS`）。

---

## 4. QA 纪律

- **测试命令**：统一走 `.\scripts\test.ps1 [lint|unit|integration|e2e|gui-build|gui-e2e|bench|all]`；
  纯 Rust 单测可直接 `cargo test -p tianyan-core --lib`（见 `AGENTS.md`）。
- **端口约定**：`server/src/main.rs` **不解析 `--host/--port`**——独立启动**默认监听
  `127.0.0.1:3000`**（`ServerConfig::default()` = `host 127.0.0.1`, `port 3000`；`TIANYAN_PORT`
  可覆盖，走 `ServerConfig::from_env`）；QA 时**直接测 3000**。
  桌面端不自己抢端口：先经 `tianyan_desktop::ensure_server` **发现同一数据目录的已有实例并复用**，
  未命中才 `find_available_port` 自起（扫 `3000..=3100`，ADR-051）。**GUI e2e 后端不用 3000**（默认 3099，
  `gui-vite/playwright.config.ts`）——避免与运行中的桌面应用互斥。
- **配置热更新会落盘**：`PUT /api/v1/config`（`config/services.rs::persist_and_reload`）
  = 校验 → `save_to_file` 写回 `~/.tianyan/tianyan.toml` → 热重载。
  **QA/测试改过配置后必须恢复**，否则污染本机配置。
- **前端门禁**：`lint` 含 eslint + prettier + typecheck；`unit` 含前端 vitest；`all` 含前端生产构建。

---

## 5. PowerShell 陷阱（本机 5.1，务必读）

**症状**：`cargo ... 2>&1 | Select-Object ...` 或管道到 `Out-File` 时，cargo 写到 **stderr**
的**正常输出**（warning 列表、`Blocking waiting for file lock` 等）被 PowerShell 当成
`NativeCommandError` **抛出** → 脚本**误判“编译失败”**。

**实锤证据**（tauri 时代 `build-msi.log` 的首行，教训与今天同一份）：

```
cargo :         Info Looking up installed packages to check mismatched versions...
At line:1 char:1
+ cargo build --release 2>&1 | Out-File -FilePath build-msi.log ...
+ CategoryInfo : NotSpecified: (...) [], RemoteException
```

—— 那只是 cargo 的一行 Info，却被报成 `RemoteException`。tauri 已删除，这个陷阱不随壳消失。

**当前脚本的写法（已核实，如实描述）**

| 脚本 | 处理方式 |
| --- | --- |
| `scripts/test.ps1` | 顶部 `$ErrorActionPreference = "Continue"`；关键步骤一律显式检查 `$LASTEXITCODE`（失败才 `throw`） |
| `scripts/gen-tool-catalog.ps1` | 顶部 `$ErrorActionPreference = "Continue"`；`cargo run ...` 以 `$LASTEXITCODE` 判定 |

**排障建议**

- 在交互/自写脚本里显式 `$ErrorActionPreference = "Continue"`，或把命令包进 `cmd /c`：

  ```powershell
  $ErrorActionPreference = "Continue"
  cmd /c "cargo check 2>&1"
  if ($LASTEXITCODE -ne 0) { throw "cargo check 失败" }
  ```

- 判断编译成败**只看 `$LASTEXITCODE`**，不要看是否出现红字/异常。
- 看到 `Blocking waiting for file lock on build directory`：说明**另一个 cargo 进程正在编译**
  （本地并行构建或残留），不是错误；应等其结束，别同时抢 build 锁。
- 用 `Out-File` 记日志时注意 PowerShell 5.1 会写 **BOM**（`build-msi.log` 尾部可见 `\ufeffBUILD_EXIT=0`）。

---

## 附：本手册核实来源

- `gui-native/src/main.rs`（`init_logging` 调用点、`panic_hook::install`、`windows_subsystem`、`ensure_server`）
- `core/src/common/logging.rs`（`LoggingConfig` / `init_logging` / 轮转）、`core/src/common/panic_hook.rs`（`default_log_dir`）
- `desktop/src/`（`discover_existing` / `ensure_server` / `find_available_port`）
- `server/src/main.rs`、`server/src/lib.rs`（`ServerConfig::default`）
- `server/src/api/tasks/{routes,handlers}.rs`（`/events`、`/events/subscribe`、`/tasks/{id}/log`）
- `server/src/api/config/services.rs`（`persist_and_reload`）
- `core/src/common/types/structured_message.rs`（`prompt_side_tokens`、`compression_marker`）
- `core/src/agent/{agent_core,loop}.rs`、`core/src/model/spec.rs`（`dynamic_max_tokens`）
- `core/src/config/storage.rs`、`core/src/config/mod.rs`（数据目录/配置路径）
- `.scratch/wake-no-user-finding.md`、`build-msi.log`、`AGENTS.md`、`docs/archive/release-notes.md`

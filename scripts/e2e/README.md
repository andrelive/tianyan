# Tianyan E2E 测试基础设施

真实前后端集成测试的零依赖基础设施：mock LLM 服务器 + 专用配置 + 工作区夹具。
Playwright 测试将按本文档的「启动顺序」拉起整个栈，然后以真实 HTTP 流量驱动
后端与前端（不 mock 前端，不 mock 后端 HTTP 层）。

```
scripts/e2e/
├── mock-llm.mjs            # 零依赖 Node mock LLM 服务器（OpenAI 兼容协议）
├── tianyan.e2e.toml        # E2E 专用后端配置
├── fixtures/workspace/     # 工作区夹具（Agent 的文件操作目标）
│   └── hello.txt
└── README.md               # 本文档
```

## 端口地图

| 端口 | 进程 | 说明 |
|------|------|------|
| 8765 | mock-llm.mjs | OpenAI 兼容 mock（`MOCK_LLM_PORT` 可改） |
| 3099 | tianyan-server | Axum 后端（**e2e 专用端口，刻意避开 3000**；`TIANYAN_E2E_PORT` 可改） |
| 5100 | vite dev | 前端开发服务器（`npm run dev`，代理 `/api` → e2e 后端端口） |

## mock-llm.mjs：提供什么

零依赖（仅 Node 内置 `http`），启动：

```bash
node scripts/e2e/mock-llm.mjs          # 端口取 MOCK_LLM_PORT，默认 8765
```

| 端点 | 行为 |
|------|------|
| `POST /v1/chat/completions` | `stream:false` → 固定 JSON 完成；`stream:true` → SSE 流（3 个内容块 + 终止块 + `data: [DONE]`） |
| `POST /v1/embeddings` | 固定归一化向量 `[1,0,...,0]`（默认 768 维，请求带 `dimensions` 时优先），任意文本余弦相似度恒为 1.0 → 检索结果确定 |
| `GET /v1/models` | `e2e-chat` + `e2e-embed` |

协议要点：

- 聊天回复恒定：`你好，我是天演 E2E 模拟助手，这条回复来自 mock-llm。`，
  流式时按三等分拆到 3 个 chunk。**永不返回 tool_calls** —— agent 循环
  保持单轮，测试可预测。
- SSE 只使用 `\n`（绝无 `\r\n`），每行后立即 flush；结束标记 `data: [DONE]`。
- 接受任意 `Authorization` 头；未知路径 → 404 JSON 错误；畸形 JSON 体 → 400。
- 仅绑定 127.0.0.1；客户端断开 / 畸形 HTTP 均不崩溃。

模块可被导入复用常量（`REPLY`、`EMBED_DIM`），服务器只在作为主模块运行时启动。

## 单独运行 smoke 示例

```bash
node scripts/e2e/mock-llm.mjs &
```

```bash
# 非流式聊天（JSON）
curl -s http://127.0.0.1:8765/v1/chat/completions \
  -H "Authorization: Bearer e2e-key" -H "Content-Type: application/json" \
  -d '{"model":"e2e-chat","messages":[{"role":"user","content":"你好"}],"stream":false}'

# 流式聊天（SSE，用 curl -N 看逐行输出）
curl -sN http://127.0.0.1:8765/v1/chat/completions \
  -H "Authorization: Bearer e2e-key" -H "Content-Type: application/json" \
  -d '{"model":"e2e-chat","messages":[{"role":"user","content":"你好"}],"stream":true}'

# 嵌入
curl -s http://127.0.0.1:8765/v1/embeddings \
  -H "Authorization: Bearer e2e-key" -H "Content-Type: application/json" \
  -d '{"model":"e2e-embed","input":"你好"}'

# 模型列表
curl -s http://127.0.0.1:8765/v1/models
```

> Windows PowerShell 下不要用 `Invoke-WebRequest` 测 SSE（会破坏流），用 `curl.exe`
> 或 Node 脚本。

## E2E 栈如何启动

顺序：**mock → 后端 → vite**。

```powershell
# 1. mock LLM
$mock = Start-Process node -ArgumentList "scripts/e2e/mock-llm.mjs" -PassThru

# 2. 后端（工作目录必须是 gui-vite/，见下文 working_directory 说明）
$env:TIANYAN_CONFIG  = "<仓库根>\scripts\e2e\tianyan.e2e.toml"
$env:TIANYAN_DATA_DIR = "$env:TEMP\tianyan-e2e-data"
$env:TIANYAN_PORT     = "3099"   # e2e 专用端口（刻意避开 3000，见端口地图）
$backend = Start-Process cargo -ArgumentList "run","-p","tianyan-server" `
  -WorkingDirectory "<仓库根>\gui-vite" -PassThru
# 等待健康检查通过（最多 ~8 分钟含首次编译）：
do { Start-Sleep 2 } until (curl.exe -s http://127.0.0.1:3099/health)

# 3. 前端
# cd gui-vite; npm run dev   # http://localhost:5100
```

**关键约束（2026-08-13 起）**：

- `TIANYAN_CONFIG` / `TIANYAN_DATA_DIR` 环境变量已由后端实现（core/src/config：
  `find_config_file()` 优先读取 `TIANYAN_CONFIG`，`default_data_dir()` 读取
  `TIANYAN_DATA_DIR`），**不再需要复制配置**。直接设置环境变量即可。
- `tianyan.e2e.toml` 中的 `working_directory = "../scripts/e2e/fixtures/workspace"`
  是**相对后端进程启动目录**解析的。后端固定从 `gui-vite/` 启动，因此该相对路径
  恰好指向仓库 `scripts/e2e/fixtures/workspace`。不要从其他目录启动后端。
- 配置热更新 API（`PUT /api/v1/config`）会持久化写回配置：E2E 结束后无需恢复
  （e2e 配置与本地 `tianyan.toml` 完全隔离，通过 `TIANYAN_CONFIG` 注入）。

## 工作区夹具

`fixtures/workspace/hello.txt` 是 Agent 文件操作的沙箱：`working_directory`
指向它，`/api/v1/workspace/tree` 应列出 `hello.txt`。测试可安全地读写/修改
该目录下的文件，不影响仓库其他部分。

**会话级工作区（ADR-015）**：工作区是会话的父级分组。除全局配置兜底外，
`POST /api/v1/chat` 可携带 `working_directory`（仅新会话生效，固化到会话头部）；
`PUT /api/v1/sessions/{id}/workspace` 可修改/清除绑定（空串）。`/workspace/*`
端点接受可选 `session_id` 解析会话绑定目录（缺省 = 全局配置）。

## 排障

| 症状 | 处理 |
|------|------|
| mock 端口被占（8765） | `Get-NetTCPConnection -LocalPort 8765` 找到 PID 并 `Stop-Process`，或换 `MOCK_LLM_PORT` 并同步改 `tianyan.e2e.toml` 的 endpoint |
| 后端端口被占（3000，残留旧进程） | `Get-NetTCPConnection -LocalPort 3000 -State Listen` 找到 PID 后 `Stop-Process -Id <pid> -Force`（旧服务器可能是上一次测试的孤儿进程） |
| `/api/v1/config/status` 返回 `configured:false` | 后端没读到 E2E 配置：确认启动时设置了 `TIANYAN_CONFIG`（指向本目录 `tianyan.e2e.toml` 的绝对路径）且 TOML 合法；不要依赖任何 `gui-vite/tianyan.toml` 复制件（该机制已废弃，且会被 `TIANYAN_CONFIG` 完全忽略） |
| `/api/v1/workspace/tree` 400「未配置 working_directory」 | 后端启动目录不是 `gui-vite/`，相对路径解析失败 |
| SSE 响应乱码 / 首行不出 | 用了 `Invoke-WebRequest`；改用 `curl.exe -N` 或 Node fetch |
| mock 崩溃 / 无日志 | mock 日志打到启动它的终端；检查 `MOCK_LLM_PORT` 是否被占用导致 listen 失败 |
| 后端行为"改了没生效" | 后端是 `cargo run` 的，确认没有残留旧进程占用 3000（见上） |

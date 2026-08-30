# 天演 0.2 路线图（Roadmap）

> 计划版本：0.2.0 · 状态：**已完成（2026-08-30）**

0.1 已发布并稳定使用（轻量任务可用）。0.2 全部功能已完成并通过端到端测试。

## 1. Web 搜索后端 UI 配置入口 ✅

- **现状**：`[web] search_backend`（duckduckgo/bing/searxng）只能改配置文件，UI 无入口。
- **实现**：设置页新增「Web 搜索」tab（后端选择 + SearXNG 端点 + 超时/缓存/抓取上限）；
  配置向导新增「Web 搜索」步骤；config-transform 透传 web 节。
- **背景**：国内默认 duckduckgo 不可达，需手动改配置为 bing。

## 2. 数据目录搬迁功能 ✅

- **现状**：数据默认存 C 盘（AppData\Local\tianyan），只能手动移动 + 改配置。
- **实现**：设置页「数据存储」tab 新增「数据目录搬迁」区——选新目录 → 确认 →
  程序自动完成（关 DB → 复制数据 → 更新配置 → 重启）。
- **技术点**：
  - 迁移请求文件 `{data_dir}/.tianyan-migrate.json` + 内部关停信号（watch channel）；
  - 服务器关停后由监督循环（Tauri）/主循环（独立 server）执行搬迁并重启；
  - **copy 而非 rename**：Windows 上 SQLite 文件可能被锁（无 FILE_SHARE_DELETE），
    copy 只需要读权限；copy 成功后尽力删除源文件；
  - **循环引用修复**：优雅关停时释放 agent（替换为向导 agent），打破
    agent → ToolRegistry → ScheduleTaskTool → manager → scheduler → UsageStats
    → SqliteDb 引用链，SQLite 文件锁得以解除（旧目录完全清空）；
  - 失败回滚（删除已 copy 目标 + 配置未动）。

## 3. Todolist 功能 ✅

- **实现**：计划面板「待办清单」tab——创建/完成/删除/优先级/关联目标/筛选；
  持久化 `{data_dir}/todos.json`（与定时智能体任务同类的运行期结构化产物，不经 VFS）。
- **API**：GET/POST /todos、PATCH/DELETE /todos/{id}。

## 4. Goal 目标功能 ✅

- **实现**：计划面板「目标」tab——创建/状态/进度条/删除；
  进度按关联待办完成比例自动计算（与 todolist 联动）。
- **API**：GET/POST /goals、PATCH/DELETE /goals/{id}。

## 5. 任务面板区分内置/后台 ✅

- **实现**：任务面板三 tab——内置任务（调度器 cron 任务）/ 定时任务（用户创建）/
  后台任务（委托子智能体），对齐 DSH 界面。

## 其他候选

- 自动更新发布流水线恢复（GitHub Actions 免费额度）
- 会话内规则/记忆刷新（ADR-012 缓存优先的实时性权衡）
- 多智能体角色扩展（PPT/办公等专业子智能体）

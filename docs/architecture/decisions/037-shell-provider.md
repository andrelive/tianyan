# ADR-037: 命令执行底层——shell provider（单一事实源）

**日期**: 2026-09-19
**状态**: ✅ 已采纳（已实施）
**影响范围**: 执行器（新增 `core/src/executor/shell.rs`）、命令执行
（`core/src/executor/command.rs`）、工具描述（`core/src/agent/tool_registry/builtin_tools.rs`）、
配置（新增 `core/src/config/executor.rs`）、server 装配（`server/src/lib.rs`、
`server/src/api/config/services.rs`）、工具目录（`core/examples/tool_catalog.rs`）

---

## 背景

`execute_command` 的执行底层长期硬编码：Windows = `powershell`（Windows
PowerShell 5.1）、Unix = `sh -c`（Ubuntu = dash）；工具描述注入**编译期静态**
的平台提示（`cfg!(target_os)` 二选一）。实测痛点：

- **PS 5.1**：不支持 `&&` / `||`（模型高频踩坑）；传参引号序列化怪癖
  （PowerShell → native 程序多层转义，实测多起翻车）；native 输出编码走系统
  locale（中文 = GBK）→ git/rg/cargo 等 UTF-8 工具**中文输出乱码**；启动开销
  200–800ms；**机器上已装的 pwsh 7 从不被使用**（硬编码 `powershell`）。
- **Unix sh（dash）**：bashism（`[[ ]]`、数组、进程替换）不支持；有 bash 却无回退。
- 二者共同：**提示词与实现是两份事实**（静态常量 vs 硬编码分支）——换任何
  一侧都可能漂移（提示说 PS、实际跑 cmd 这类事故的先验概率不为零）。

## 决策

### 1. 单一事实源：`ShellSpec`

```
ShellKind（身份） + executable: PathBuf（执行路径） + args + hint（提示词）
```

执行侧（`build_command`）与提示词侧（`current_hint`，工具描述注入）**读同一个
spec**——"执行什么"与"告诉模型什么"同源产出，从结构上杜绝漂移。

### 2. 解析契约（`resolve`）

- `auto`（默认）：Windows 探测 `pwsh` → `powershell`；Unix 探测 `bash` → `sh`
  （PATH 搜索，**命中记绝对路径**——探测/执行漂移免疫）；
- 显式 provider：验证可执行存在——**找不到明确报错**（不静默回退：静默回退
  会让用户以为在用 A、实际在用 B）；`shell_program` 可覆盖默认程序（名或路径，
  如 pwsh 不在 PATH 时给绝对路径）；
- `custom`：`shell_program` 必填（名 → 探测；路径 → 验证）；
- **失败即启动报错**（含修复指引）；配置热更新**先验证后保存**（失败拒绝）。

### 3. 提示词注入与稳定性纪律

- hint 是 spec 的组成部分；工具描述**每轮现取**全局 spec → 配置变更在下一轮
  生效（无需新机制）；
- **稳定性纪律**（同 `ToolRegistry::definitions` 的前缀缓存契约）：install 后
  字节稳定，**绝不每轮动态生成**（先例：角色清单因自动演化漂移描述字节而被
  移出工具描述，改 `suggest_role` 按需查询）；
- **切换代价明示**（`[executor]` 配置注释 + 本 ADR）：切换使工具表指纹变化 →
  下一轮**重建请求前缀**（一次缓存未命中）+ 一次**主动压缩**（消息足够时）——
  低频操作可接受；由既有 1.5 检测（指纹 → 合并压缩到同一轮）自动吸收，**不
  免费但可控且已在机制内**；
- freshness 归一化（`tool_catalog --check`）升级为遍历
  `executor::shell::HINT_TEMPLATES` 全量替换（跨平台/跨配置不误报漂移）——
  **须同时替换 `\|` 转义形态**：产物表格把描述里的 `|` 转义（PS 系 hint 含
  `&& / ||`），只替换原文会在跨平台比较时**假红**（Windows 同平台两侧同文本、
  侥幸通过；CI Linux 必红）→ 判别力测试覆盖两种形态。

### 4. 内置 provider 与编码修补

| provider | 解析 | hint 要点 |
|---|---|---|
| `auto` | pwsh→powershell / bash→sh | 按实际探测 |
| `pwsh` | PowerShell 7+ | `&&`/`\|\|` 可用 |
| `powershell` | PS 5.1 | **不支持 `&&`/`\|\|`（用 `;`）** |
| `cmd` | cmd.exe | cmd 语义（如实描述） |
| `bash` / `sh` | 相应 | 完整 bash 语法 / POSIX 边界（附切换指路） |
| `custom` | 用户程序 | 用户 hint 或自动生成 |

- **UTF-8 输出编码注入**（对命令语义透明，退出码不受影响）：PS 系前置
  `[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;`；cmd `chcp 65001`——
  修复 native 工具中文输出乱码。

### 5. 装配与热更新（单点）

- **启动**：`server/lib.rs` 装配段 resolve + install（失败阻止启动，日志含
  kind/executable）；
- **热更新**：`persist_and_reload`（配置更新唯一序列）内先 resolve 验证 →
  保存 → install；验证失败返回配置类错误（分类保真，含修复指引）；
- 未装配场景（example / 测试）：lazy 按默认配置 auto 探测（极端环境兜底不 panic）。

## 非目标

- 细粒度 env 注入（`[executor.env]`）——等真实需求（登录环境可用 custom
  `bash -lc` 表达）；
- GUI 配置控件（TOML 直接配置为本期形态；配置页如自动渲染新节即随之可见）。

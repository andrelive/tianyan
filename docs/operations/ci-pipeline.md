# CI 流水线排障手册（ci-pipeline）

> 适用版本：0.6.3（撰写基线 HEAD `87aa123`）
> 覆盖：GitHub Actions 上 `quality.yml` / `release.yml` 的**挂死、日志丢失、假失败**类问题。
> 结论全部来自 0.6.3 发布周期的真机实测（附 run id / commit 便于复核）。
> 相关文档：[`troubleshooting.md`](troubleshooting.md)（应用运行时）· [`release-msi.md`](release-msi.md)（打包发布）

---

## 0. 症状 → 根因 速查表

| 症状 | 根因 | 处置 |
| --- | --- | --- |
| 步骤挂死 15–44 分钟；job 结束后**该 job 的日志 blob 不存在**（UI 点开是空的） | **runner 失联**（CPU/内存饥饿 → GitHub 判定 lost communication，job 异常收尾，日志未 finalize） | §2：诊断产物**就近上传 artifact**，别指望"挂住之后"的步骤还能跑 |
| `Install protoc` 步骤 **00:00 瞬时失败**、后续全 skipped | `arduino/setup-protoc@v3` 需从 GitHub Releases 下载，偶发抽风（0.6.3 期间命中 2 次，每次整轮白跑） | §7：已改 apt + 重试 |
| 单测步骤耗时 15 分钟却**全程在 `Compiling`**（一个测试都没跑到就被 timeout 杀掉） | **包集合不一致** → cargo 编译指纹不同 → 前一步的编译产物整棵作废 | §1 |
| `Starting N tests across M binaries` 之后**再无任何输出**（连一条 nextest SLOW 都没有），随后失联 | nextest 的 **process-per-test 进程累积**把 runner 拖死 | §3（**已定位 + 已修复**） |
| 本地 `cargo check` 全绿、CI `Rust clippy (deny warnings)` 红 | 本地用了比 CI 弱的检查（如漏跑 clippy） | §5：以 `.\scripts\test.ps1 lint` 为验收入口 |
| 每轮 CI 都冷编译 ~30 分钟 | rust-cache 失败轮次不保存（`CACHE_ON_FAILURE: false`）；tag 推送默认不保存 | §6 |
| `kill -9 -<pid>` 之后 runner 失联 / WSL 服务崩溃（`E_UNEXPECTED`） | 负 PID 信号**误杀无关进程组**（pid 已被系统复用） | §4 |
| 本机脚本轮询 GitHub API 突然报错退出 | 匿名请求 **60 次/小时**被打满 | §7：取证通道一律带 token |

---

## 1. 包集合决定编译指纹（最容易踩，也最贵）

### 现象（0.6.3 tag `040ec2e` 实测）

```
Build tests (compile only)      success  29:07   ← 已编译完成并报 Finished
Rust unit tests (core 1/2)      15 分钟后被 timeout 杀掉
```
把该步日志拉下来看，`core 1/2` 这 15 分钟**全在编译**（`serde_json → arrow-* → datafusion-* → lance-* → pdf-extract`），
**一个测试都没跑到**。而它的上一环节 2 分钟前才刚报"编译完成"。

### 原理

**cargo 的 feature 统一（feature unification）范围取决于「本次命令行选中的包集合」**：

- `--workspace --exclude tianyan-tauri` 编出的产物
- 与 `-p tianyan-core` 编出的产物

**编译指纹不同** ⇒ 后者会**整棵依赖树重编译**，前一步的 29 分钟白干。
（本仓库 core 本身无 features、server/mcp 依赖 core 也不带 feature —— 差异来自"选中集合"本身，不是 feature 声明。）

### 本机对照实验（独立 `--target-dir`，2026-10-08）

| 步骤 | 命令 | 结果 |
| --- | --- | --- |
| STEP1 | `cargo test -p tianyan-core -p tianyan-server -p tianyan-mcp --lib --no-run`（冷） | **1310s（21.8 min）**、637 条 `Compiling`、产物 **6.29 GB** |
| STEP2 | **完全相同命令**再跑 | **2.8s、0 条编译** ⇒ 复用成立 |
| STEP3 | `cargo test --workspace --exclude tianyan-tauri --lib --no-run` | 2.7s、0 条编译（本机两集合恰好无差异；CI 上则出现过重编译，见 §0） |

CI 侧另有直接证据（run `37730710190`）：`reuse probe` 步用**与测试步骤逐字相同**的参数跑 `--no-run`，
耗时 **1.58s**（另一轮 0.53s / 0.23s / 0.01s）—— 证明"同参数必复用"。

### 纪律（写 workflow 时必须遵守）

**预编译步骤与使用它的测试步骤必须同包集合、逐字一致**，允许的差异只有这几项（都不影响编译指纹）：

- `--no-run`（只编译不运行）
- 测试过滤参数（libtest 的 filter）
- `--test-threads N` / nextest 的 `-j N`（并发度）
- nextest 的 `--partition count:i/n`（测试选择）

`quality.yml` 如今有**两组**集合（core 单独 / server+mcp），**各配一条预编译** —— 这比"强行统一成一组"
更贴合"不把 3 个重二进制同时拉起"的初衷，代价是两条 Build tests 步骤。

> ⚠️ 0.6.3 排查期间我自己也踩过：诊断用的"模块二分"步写成 `-p tianyan-core`（单包），
> 而其余步骤是 3 包 ⇒ 白编译 ~15 分钟（`mod_agent.log` 开头整片 `Compiling`）。

---

## 2. runner 失联会丢掉**整个** job 日志 → 诊断产物必须就近上传

### 现象

`bdddfe1` 的 Release run：`Rust unit tests (1/4)` 一直 `in_progress` 到 job 被判失败（79 分钟），
`timeout-minutes: 12` / 步骤内的 `timeout 900` 都**没能生效**（runner 已失联，无人执行收尾），
且 **该 job 日志 blob 不存在**（`BlobNotFound`）—— 这正是历史上反复出现的"**日志为空 / 点不开**"的真因：
**不是日志为空，而是 runner 死前没能把日志 finalize 上传**。

### 处置纪律

1. **诊断输出要写进 artifact，且 artifact 步要放在"可能挂住"的步骤之前**。
   （0.6.3 实测反例：把 `upload-artifact` 放在 libtest 对照步**之后** ⇒ 该步挂住 ⇒ 上传根本没执行 ⇒ 整轮诊断白跑。）
2. 一步一产物（`ci-probe-<run_id>` 等），别共用 name。
3. 想让结论**抗失联**，用 `$GITHUB_STEP_SUMMARY`（**步骤级**收集）作为第二通道 —— 0.6.3 实测：
   19 个模块的汇总行在 runner 失联的 run 里依然在 UI 的 Job summary 中可见。
4. 要"挂住也要有结论"，就把大动作**拆成多个独立步骤**（每步带自己的 `timeout-minutes`）。

### 取证命令（本机，需要 GitHub 凭据）

```powershell
# job 日志（匿名 403，必须带 token；token 从 git credential fill 取，勿内联进命令行）
curl.exe -sSL --compressed -H "Authorization: Bearer $tok" -H "User-Agent: tianyan-qa" `
  "https://api.github.com/repos/andrelive/tianyan/actions/jobs/<job_id>/logs" -o job.log

# artifact（含 diag.txt / mod_*.log / st1.log）
curl.exe -sSL --compressed -H "Authorization: Bearer $tok" `
  "https://api.github.com/repos/andrelive/tianyan/actions/artifacts/<artifact_id>/zip" -o art.zip
```

---

## 3. 单测把 runner 拖死（**已定位 + 已修复**）

### 现象

`Rust unit tests` 步骤 in_progress **15–44 分钟**直至 job 被判失败；**该 job 的日志 blob 不存在**
（UI 点开是空的）。GitHub 的 Annotation 原文：

> The hosted runner lost communication with the server. Anything in your workflow that
> terminates the runner process, starves it for CPU/Memory, or ...

### 判据：同一批 1683 个测试的实测对照

| 跑法 | 引擎 | 结果 |
| --- | --- | --- |
| 单线程全量（run `37779645788`） | libtest | ✅ 1683 ok / 44s |
| 按模块逐个（连续 3 轮，如 run `37751282476`） | libtest | ✅ 全过 |
| 分 4 片（多轮，如 `bdddfe1` / `6a78aee`） | nextest | ❌ 挂死 15–44 分钟 |
| **只跑 4/4 片、且放在最前**（run `37787101693`） | nextest | ✅ **7s** |
| **同样的 4/4 片，接在 1/4+2/4+3/4 之后**（同 run） | nextest | ❌ **挂死** |
| 3 包全量 + 多线程（run `37742472765`） | libtest | ❌ 挂死 46 分钟 |
| 单包全量（WSL Ubuntu） | libtest | ❌ 767 个后 WSL 崩溃（环境性，见 §4） |
| 单包全量（本机 Windows） | libtest | ✅ 1485 测试 22s |

**决定性的一条**：**同一个 4/4 分片，单独跑 7 秒通过、接在前三片之后跑就挂** ⇒ 不是
"某个测试自身挂住"，而是**累积效应**。

### 根因

**nextest 是 process-per-test（每个测试一个进程）**。在 4 核免费 runner 上，分片累计创建
上千个测试进程（每个还要加载数百 MB 的测试二进制）之后，runner 会**失去与 GitHub 的通信**，
且**整份丢掉 job 日志** —— 历史上所有"日志为空 / 点不开"都是这个，不是日志真的为空。

另：libtest 在 **3 包全量 + 多线程**下**也会挂**（run `37742472765`）⇒ 除进程累积外，还存在
"**并发 × 测试量**"这一因素（见下节"仍未坐实"）。

**已排除的解释**（都做过对照，别再重复）：重编译（probe 0.01–6.5s 秒级复用）、磁盘
（build 后仍余 74 GB）、内存（15.9 GB 仅用 1.5 GB）、pid 复用误杀（§4 加固后仍复现）。

### 修复（`87aa123`，验证全绿：run `37868892911`）

- 单测改用 **libtest**：`core` 单包逐模块（19 个，`::group::` 折叠 + `--test-threads=2`）
  + `core` 全量兜底（覆盖新模块 / 无模块前缀的测试）+ `server`/`mcp` 两包全量；
- 两组包集合（core 单独 / server+mcp）**各配一条预编译**（§1 纪律）；
- 效果：单测步骤 **0:52**（19 模块全过），整条 quality **~13 分钟**（此前挂死 79 分钟）；
- 已移除 `cargo-nextest` 的安装与使用（`.config/nextest.toml` 保留备查）；
- **别改回 nextest** —— 原因就是上面那条"process-per-test 累积"。

### 已坐实并修复：跨模块并发改进程环境变量（2026-10-09，run `37876186817`）

**结论**：干扰源就是「**多个测试模块并发改动进程环境变量**」。

**依据链**（三步，缺一不可）：
1. 「全量 + 多线程」挂死（run `37742472765`，46 分钟），而「单线程全量」与「按模块」都稳过
   ⇒ 与"**并发 × 测试量**"相关；
2. 代码侧核对：`config` 下三处 env 操作（`set_var` / `remove_var`）的保护锁**是模块私有**的、
   彼此不互斥，其中 `config/model.rs::test_resolve_api_key_env_var` **完全没加锁**；
3. **收敛到一把跨模块共享锁**（`config::test_support::ENV_LOCK` + `with_env_var` 辅助函数，
   提交 `51ae9b3`）之后：**「全量 + 4 线程」一步 20 秒全过**（run `37876186817`）。

**机制**：`std::env::set_var/remove_var` 会改写进程级 `environ`，glibc 上要 realloc 该数组
并与分配器锁交互；多线程下这些操作与其它测试交错时极脆弱。**Windows 的 `_putenv` 路径不同**
⇒ 这解释了长期存在的「**Linux 挂 / Windows 全绿**」。

**现状与纪律**：
- 门禁保留「**按模块**分片」（最稳，0:53）；
- 另保留一步「**全量 + 4 线程**」作为**并发回归哨兵**（20 秒）—— 若日后有人在测试里裸用
  `set_var`，它会立刻暴露；
- 新增/修改涉及进程环境变量的测试时，**一律走 `config::test_support::with_env_var`**
  （或至少持 `ENV_LOCK`），不要自建私有锁。
- 根治方向（如需进一步解耦）：彻底改为**不依赖进程环境变量**（配置以参数注入）。

### 排查手法留档（可复用）

- **单线程全量**（`--test-threads=1` + `stdbuf -oL`）：区分"并发相关" vs "测试自身挂"；
  单线程下 libtest 逐个打印 `test X ... ok`，挂点即"最后一条 ok 的下一个测试"。
- **隔离分片**（`--partition count:4/4` 单独跑、且放在最前）：区分"该片自身" vs "累积"。
- **模块二分**（逐模块 `timeout 300`，结果同时写 stdout 与 `$GITHUB_STEP_SUMMARY`）。

---

## 4. 进程组信号：负 PID 必须先校验「pid 未被复用」

### 背景

`kill_process_tree`（`core/src/executor/command.rs`）用 `process_group(0)` spawn 子进程，
故可用 `kill -9 -<pid>` 一次带走整棵进程树（对齐 opencode 教训：只杀 shell 会留孤儿服务进程）。

### 两次加固

1. **2026-10（`040ec2e`）**：负信号前先读 `/proc/<pid>/stat` 校验 `pgid == pid`。
   实发事故：CI runner 被整组 SIGKILL ⇒ 该步骤**无任何日志** + `The hosted runner lost communication`；
   WSL 侧表现为服务崩溃（`Wsl/Service/E_UNEXPECTED`，kill-wrapper trace 抓到 `kill -9 -<pgid>` 之后立即崩）。
2. **2026-10（`f0e8627`）**：再加 `ppid == std::process::id()`。
   因为 `pgid == pid` **不足以排除 pid 复用** —— 复用的新进程若恰好是另一个组的组长，判据同样成立。
   `ppid` 是唯一能区分「我们自建的组长」与「恰好同号的陌生组长」的廉价判据。

> Windows 走 `taskkill /PID /T /F`（无进程组语义）⇒ **本地从不复现**这条路径的问题。

---

## 5. 本地验收入口（避免"本地绿 / CI 红"）

推送前跑 **`.\scripts\test.ps1 lint`**（= fmt + `clippy --all-targets -D warnings` + 工具目录 freshness + 前端 lint/typecheck）。

**不要**用单条手工命令替代：0.6.3 排查期间只跑了 `cargo check` 就推送，
CI 的 clippy 立刻红（`doc_lazy_continuation`：doc 注释里列表项后紧跟无缩进段落）。

改 workflow 文件时另需本地校验语法（YAML 里 `- name:` 含 `: ` 必须加引号，0.6.3 踩了两次）：

```powershell
python -c "import yaml; d=yaml.safe_load(open('.github/workflows/quality.yml',encoding='utf-8')); print('OK', len(d['jobs']['quality']['steps']))"
```

---

## 6. rust-cache 行为（决定 CI 时长）

- **tag 推送**（`refs/tags/*`）既非默认分支也非 PR ⇒ rust-cache 默认**不保存**缓存；
  必须显式 `save-if: true`（已在 `quality.yml` 配置）。
- **失败轮次不保存**（运行时 env 可见 `CACHE_ON_FAILURE: false`）⇒ 连续失败期间每轮都冷编译。
- 一旦某轮 job **success**，缓存落盘 ⇒ 后续 `Build tests` 从 30 分钟降到 **2–4 分钟**
  （0.6.3 实测：`30:53 → 04:40 → 02:14`）。
- rust-cache 的 **restore/prune 在 job 首尾**，不会在步骤之间清 target。

---

## 7. 其它已知点与取证要点

### protoc 步骤（已加固）

原先用 `arduino/setup-protoc@v3`（需从 GitHub Releases 下载），0.6.3 期间**命中 2 次
00:00 瞬时失败**，每次整轮 skipped 白跑。现改为 **apt 安装 + 3 次重试**，并打印
`protoc --version`（`quality.yml`）。`release.yml` 的 **windows build job** 仍用
`arduino/setup-protoc@v3`（Windows 无 apt；该 job 频率低，暂不改）。

### 取证通道（0.6.3 排查踩过）

- **GitHub API 匿名请求 60 次/小时**，监控脚本轮询很快打满
  （`API rate limit exceeded for <ip>` ⇒ 脚本异常退出、误判为"任务失败"）。**一律带 token**：
  从 `git credential fill`（host=github.com）取，**不要内联进命令行**（会进 shell 历史/日志）。
- **job 日志端点匿名 403**，必须带 token；artifact 同理。
- **SSH 通道可能整体不通**（0.6.3 期间 `github.com:443` 与 `ssh.github.com:443` 同时
  `Connection timed out`，重试 5 次全败）。此时改走 **HTTPS 推送**：
  `git push https://github.com/andrelive/tianyan.git <ref>`（凭据由 GCM 提供）。
- ⚠️ **push 失败不影响 `workflow_dispatch`** —— 有过一次"push 挂了但 dispatch 成功"，
  结果触发的是**旧提交**，白跑一轮还污染 run 列表。**先校验远端 refs 再 dispatch**。

### 其它

- **`rust-toolchain.toml` 的 `channel = "stable"`** 有版本漂移风险（待定是否 pin 具体版本）。
- **工具目录 freshness 门禁**：改了 `def_*` 描述或 `*Params` schema 必须
  `.\scripts\gen-tool-catalog.ps1` 重生成（命令单一来源在 `.cargo/config.toml` 的 alias）。

---

## 附：本轮相关 commit / run（便于复核）

| 对象 | 值 |
| --- | --- |
| 包集合统一修复 | `bdddfe1` |
| 诊断步（disk/mem + reuse probe） | `6a78aee` |
| libtest 对照实验 | `8800e28` |
| 模块二分 + artifact 就近上传 | `4e8d415` |
| 单线程全量 + 隔离分片判别实验 | `1506b8a` / `3035fc7` |
| 进程组 `ppid` 加固 | `f0e8627` |
| **CI 手册 + 诊断沉淀** | `61310b0` |
| **单测改 libtest + 按模块分片（最终修复）** | **`87aa123`** |
| 0.6.3 发布（全绿） | Release run `37760821766` / Quality run `37760821879` |
| 修复验证（全绿，单测 0:52） | Quality run `37868892911`（tag 仍指 `4e8d415`） |
| protoc 改 apt + 重试 | `e13d65c`（验证 run `37870943324`） |
| **env 锁收敛（并发干扰源修复）** | **`51ae9b3`** ／ 验证 run **`37876186817`**（哨兵步 0:20 全过） |

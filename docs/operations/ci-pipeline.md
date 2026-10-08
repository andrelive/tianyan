# CI 流水线排障手册（ci-pipeline）

> 适用版本：0.6.3（撰写基线 HEAD `1506b8a`）
> 覆盖：GitHub Actions 上 `quality.yml` / `release.yml` 的**挂死、日志丢失、假失败**类问题。
> 结论全部来自 0.6.3 发布周期的真机实测（附 run id 便于复核）；未结案项显式标注。
> 相关文档：[`troubleshooting.md`](troubleshooting.md)（应用运行时）· [`release-msi.md`](release-msi.md)（打包发布）

---

## 0. 症状 → 根因 速查表

| 症状 | 根因 | 处置 |
| --- | --- | --- |
| 步骤挂死 15–44 分钟；job 结束后**该 job 的日志 blob 不存在**（UI 点开是空的） | **runner 失联**（CPU/内存饥饿 → GitHub 判定 lost communication，job 异常收尾，日志未 finalize） | §2：诊断产物**就近上传 artifact**，别指望"挂住之后"的步骤还能跑 |
| 单测步骤耗时 15 分钟却**全程在 `Compiling`**（一个测试都没跑到就被 timeout 杀掉） | **包集合不一致** → cargo 编译指纹不同 → 前一步的编译产物整棵作废 | §1 |
| `Starting N tests across M binaries` 之后**再无任何输出**（连一条 nextest SLOW 都没有），随后失联 | "全量挂 / 分模块不挂"现象，见 §3（0.6.3 未完全结案） | §3 判别实验 |
| 本地 `cargo check` 全绿、CI `Rust clippy (deny warnings)` 红 | 本地用了比 CI 弱的检查（如漏跑 clippy） | §5：以 `.\scripts\test.ps1 lint` 为验收入口 |
| 每轮 CI 都冷编译 ~30 分钟 | rust-cache 失败轮次不保存（`CACHE_ON_FAILURE: false`）；tag 推送默认不保存 | §6 |
| `kill -9 -<pid>` 之后 runner 失联 / WSL 服务崩溃（`E_UNEXPECTED`） | 负 PID 信号**误杀无关进程组**（pid 已被系统复用） | §4 |

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

CI 侧另有直接证据（run `37730710190`）：`reuse probe` 步用**与分片逐字相同**的参数跑 `--no-run`，
耗时 **1.58s**（另一轮 0.53s / 0.23s）—— 证明"同参数必复用"。

### 纪律（写 workflow 时必须遵守）

**同一 workflow 内所有 cargo / nextest 步骤共用同一包集合**，允许的差异只有这几项（都不影响编译指纹）：

- `--partition count:i/n`（nextest 测试选择）
- `-j N`（并发度 / test-threads）
- `--no-run`（只编译不运行）
- 测试过滤参数（libtest 的 filter）

> ⚠️ 0.6.3 排查期间我自己也踩过一次：诊断用的"模块二分"步写成 `-p tianyan-core`（单包），
> 而其余步骤是 3 包 ⇒ 又白编译 ~15 分钟（`mod_agent.log` 开头整片 `Compiling`）。

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
2. 一步一产物（`ci-probe-<run_id>` / `ci-bisect-<run_id>` / `ci-singlethread-<run_id>`），别共用 name。
3. 想让结论**抗失联**，可用 `$GITHUB_STEP_SUMMARY`（步骤级收集）作为第二通道。
4. 要"挂住也要有结论"，就把大动作**拆成多个独立步骤**（每步带自己的 `timeout-minutes`），
   前面已结束的步骤结论就不会被后面的挂死抹掉。

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

## 3. "全量挂 / 分模块不挂"（**0.6.3 未完全结案**）

### 观察表（全部 CI 实测）

| 跑法 | 引擎 | 范围 | 结果 |
| --- | --- | --- | --- |
| 3 包全量 libtest（run `37742472765`） | libtest | core+server+mcp | ❌ 挂死 46 分钟 |
| 3 包 hash 1/4 分片（多轮） | nextest | 混合所有模块 | ❌ 挂死 15–44 分钟 |
| 单包 × 每模块单独（run `37751282476`） | libtest | 19 个模块逐个 | ✅ 全过（步耗时 23:26，含编译） |
| 3 包 hash 1/4 分片（同 run，**在模块二分之后**） | nextest | 混合所有模块 | ✅ **15/9/8/8 秒** |
| 单包全量（WSL Ubuntu） | libtest | core | ❌ 767 个后 WSL 崩溃（环境性，见 §4） |
| 单包全量（本机 Windows） | libtest | core | ✅ 1485 测试 22s |
| 3 包 1/4 分片（本机 Windows） | nextest 0.9.148 | 3 包 | ✅ 422 passed / 39.8s |

**已排除的解释**（都做过对照，别再重复）：

- ❌ **nextest 是根因 / nextest×Rust 1.99 冲突** —— 同一份产物换 libtest（`cargo test`）**同样挂死**（run `37742472765`）；
  nextest 0.9.148 在本机同命令全绿 ⇒ 与引擎无关。
- ❌ **重编译**（probe 0.2–6.5s 秒级复用）、**磁盘**（build 后仍余 74 GB）、**内存**（15.9 GB 仅用 1.5 GB）。
- ❌ **pid 复用误杀**（见 §4；`ppid` 加固后仍复现）。

**当前可复现的稳定配方**（0.6.3 发布即用此，run `37760821766` 全绿）：
在分片步骤**之前**保留「模块二分」诊断步（逐模块串行 libtest）。**但这属于"靠一个步骤的副作用维持"**，
根因未坐实 —— 属于**已知债务**，见下。

### 判别实验（下一步要做）

CI 上跑 **单线程全量** libtest（`--test-threads=1`，包集合与分片一致）：

```bash
stdbuf -oL -eL timeout 900 cargo test -p tianyan-core -p tianyan-server -p tianyan-mcp --lib -- --test-threads=1
```

- **通过** ⇒ 与**并发/资源竞争**相关。
- **也挂** ⇒ 某个测试**自身**挂住，与并发无关；单线程下 libtest 逐个打印 `test X ... ok`，
  **日志最后一条 ok 的"下一个测试"即挂点**。（`stdbuf -oL` 行缓冲保证进度可见；结果连 `st1.log` 一起传 artifact。）

### 候选根因（待验证）

测试代码在**多个模块**里改进程环境变量，而**保护锁是模块私有的**：

| 位置 | 锁 |
| --- | --- |
| `core/src/config/mod.rs`（`with_tianyan_config_env`） | `ENV_CONFIG_LOCK`（模块内 static） |
| `core/src/config/storage.rs` | `ENV_DATA_DIR_LOCK`（模块内 static） |
| `core/src/config/model.rs` | **无锁** |

这些锁**彼此不互斥**，全量并发跑时环境变量测试会与大量其它测试线程交错。
glibc 的 `setenv` 会 realloc `environ` 数组（牵涉分配器），多线程下素来脆弱；
而 **Windows 的 `_putenv` 路径完全不同** ⇒ 天然解释「**Linux 挂、Windows 全绿**」。
（若坐实，修复方向：收敛到**一把全局共享锁**，或彻底改为不依赖进程环境变量。）

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
   实测指纹：**全量单测必崩、该测试单跑必过**（跑量足够大才会命中 pid 复用）。

> Windows 走 `taskkill /PID /T /F`（无进程组语义）⇒ **本地从不复现**这条路径的问题。

---

## 5. 本地验收入口（避免"本地绿 / CI 红"）

推送前跑 **`.\scripts\test.ps1 lint`**（= fmt + `clippy --all-targets -D warnings` + 工具目录 freshness + 前端 lint/typecheck）。

**不要**用单条手工命令替代：0.6.3 排查期间我只跑了 `cargo check` 就推送，
CI 的 clippy 立刻红（`doc_lazy_continuation`：doc 注释里列表项后紧跟无缩进段落）。

另注：**本机从未装过 nextest** ⇒ "本地全绿"一直是 libtest 的成绩，与 CI 的 nextest 不是同一执行引擎，
不能相互替代判据。

---

## 6. rust-cache 行为（决定 CI 时长）

- **tag 推送**（`refs/tags/*`）既非默认分支也非 PR ⇒ rust-cache 默认**不保存**缓存；
  必须显式 `save-if: true`（已在 `quality.yml` 配置）。
- **失败轮次不保存**（运行时 env 可见 `CACHE_ON_FAILURE: false`）⇒ 连续失败期间每轮都冷编译。
- 一旦某轮 job **success**，缓存落盘 ⇒ 后续 `Build tests` 从 30 分钟降到 **4 分钟**（0.6.3 实测：`30:53 → 04:40`）。
- rust-cache 的 **restore/prune 在 job 首尾**，不会在步骤之间清 target。

---

## 7. 其它已知点

- **nextest 版本未 pin**：`taiki-e/install-action@nextest` 装的是 **latest**（0.6.3 期间恰好 0.9.148，
  于排查当天发布）。版本漂移本身是风险源，建议后续 pin 一个版本（如 `0.9.146`）。
- **`rust-toolchain.toml` 的 `channel = "stable"`** 同理有漂移风险（待定是否 pin）。
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
| 单线程全量判别实验 | `1506b8a` |
| `ppid` 加固 | `f0e8627` |
| 0.6.3 发布（全绿） | Release run `37760821766` / Quality run `37760821879` |

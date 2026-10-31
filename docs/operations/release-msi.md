# 发布：native MSI（GPUI 客户端）

> **当前唯一发布渠道 = `gui-native`（GPUI 原生客户端，内嵌 server，单 exe）。**
> `tauri/` 已于 2026-10 退役删除、`scripts/build.ps1` 同步退役；`gui-vite/` 保留做对照，**不随包发布**。
> 本文替代 2026-10 之前的 Tauri 版发布文档（旧流程见 git 历史）。

## 1. 版本号只有一个源

| 位置 | 作用 | 发版时 |
|---|---|---|
| 根 `Cargo.toml` → `[workspace.package] version` | **产品版本单源**（3 段，如 `1.1.0`） | 改这里 |
| `gui-native/Cargo.toml` → `[package] version` | exe 的 `FileVersion` / `ProductVersion`；`build-msi.ps1` **从这里读** | 必须与上面一致 |
| `gui-native/installer/tianyan.wxs` | `Package/@Version` 必须是 `$(var.ProductVersion)`（脚本有门禁，硬编码会直接报错） | 不改 |
| MSI 内部 `ProductVersion` | 4 段（`1.1.0.0`）—— Windows Installer 规范要求，仅用于升级比较 | 脚本自动补 `.0` |
| ARP `DisplayVersion` | 3 段（`1.1.0`）——「设置 → 应用」里显示这个 | 脚本自动注入 |

**产物名同源**：`gui-native/installer/tianyan-native-<ver>-x64.msi`。

门禁：`.\scripts\test.ps1 lint` 里有「版本一致性校验」（workspace / `gui-native` / wxs 注入 / 打包脚本四处对齐）。

## 2. 本地出包

```powershell
cd gui-native
cargo build --release                        # build.rs 把 assets/icon.ico 嵌进 exe 资源
pwsh -NoProfile -File installer\build-msi.ps1             # 默认会先 build（幂等）
pwsh -NoProfile -File installer\build-msi.ps1 -SkipBuild  # exe 已就绪时跳过构建
```

- 耗时量级（本机增量）：release 增量 40–60s + 打包 80–110s，产物约 **61 MB**。
- 依赖 **WiX v6**（`dotnet tool install --global wix --version 6.0.2`）。**固定 v6**：v7 引入 OSMF 收费条款。
- **别直接 `wix build`**：版本注入、产物命名、`$(var.ProductVersion)` 门禁都在脚本里。

安装（perUser，无 UAC，约 3 秒）：

```powershell
msiexec /i gui-native\installer\tianyan-native-1.1.0-x64.msi /qn /norestart
```

## 3. CI 发布链（`.github/workflows/release.yml`）

- **触发**：推 `v*` tag = 真发布；`workflow_dispatch` = **只验构建链、不发布**。
  > 推 tag 之前先手动跑一次。GPUI 全量冷编译在 CI 上与本地增量差异很大（依赖、缓存命中、耗时），等推 tag 才发现编不过就晚了。
- **流程**：`quality`（复用 `quality.yml`，红则 build 跳过、不出包）→ `build-native`（windows-latest）
  1. tag ↔ 版本比对：`scripts/check-version.ps1` + 读根 `Cargo.toml` 的 `[workspace.package] version`，先内部一致再比 tag（防「发了 v1.2.0 标签、包里还是 1.1.0」）
  2. `Swatinem/rust-cache`：**两个 workspace 的 target 都要缓存**（`.` 与 `gui-native`），且必须 `save-if: true` —— tag 推送既非默认分支也非 PR，该 action 默认不存缓存，否则每次发版都冷编译 lance / datafusion / GPUI（实测 21 分钟 → 30:53）
  3. `build-msi.ps1` → MSI + `SHA256SUMS.txt`
  4. 只有 tag 才 `softprops/action-gh-release` 上传
- **不产出 `latest.json`**：那是 tauri updater 的清单（带 `.sig`）。native 目前没有自动更新，硬塞一个无签名的 `latest.json` 只会让残留的 tauri 客户端拿到「格式对但签名缺失」的更新而报错。改为 `SHA256SUMS.txt` 供手动校验。
- **不构建 `gui-vite`**：native 是纯 Rust 前端，MSI 只装 exe。注意 server 的静态服务读 `gui-vite/dist`，当前安装包里没有它。
- **已知风险**：`arduino/setup-protoc` 从 GitHub Releases 拉取，历史上出现过「00:00 瞬时失败」导致整轮白跑（Linux 侧已改 apt，见 [`ci-pipeline.md`](ci-pipeline.md) §0）。Windows runner 无 apt，暂保留该 action；再遇到改用 `choco install protoc -y`。

## 4. 已知坑

- **`Get-Process tianyan-gui-native | Stop-Process` 会杀掉用户正在用的实例**（隔离实例与已装实例**进程同名**，实测踩过）。只按 **PID** 或 `Path` 精确操作。
- **打包前必须退出运行中的实例**：exe 被占用时 `cargo build --release` 报 `failed to remove file ... 拒绝访问 (os error 5)`。
- **`-SkipBuild` 会用旧 exe**：脚本会比对 exe 的 `ProductVersion` 与 `Cargo.toml`，不一致只**告警**（不是错误）—— 别忽略。
- **装到哪、数据在哪**：程序在 `%LOCALAPPDATA%\Programs\Tianyan\`（perUser）；数据目录由配置 `storage.data_dir` 决定（本机 `D:\tianyan`），**卸载不删数据**（ADR-023）。

## 5. 验收清单

### 5.1 装前

- [ ] `.\scripts\test.ps1 lint` 绿（版本一致性 + tool-catalog freshness）
- [ ] 没有运行中的实例（按 `Path` 确认，别按进程名一刀切）
- [ ] exe 时间戳与本次构建一致（用 `-SkipBuild` 时尤其要看）
- [ ] MSI 文件名里的版本 = 根 `Cargo.toml` 的版本
- [ ] 记下 `SHA256`（CI 发布时会写进 `SHA256SUMS.txt`）

### 5.2 装后

- [ ] 「设置 → 应用」显示 `1.1.0`（3 段；`1.1.0.0` 只是 MSI 内部版本）
- [ ] 设置 → 连接 里的服务版本与 `/health` 一致，且 **数据目录指原处**（`D:\tianyan`，设置页该字段只读 —— ADR-023 §4）
- [ ] 默认 `approval_mode = autonomous`（黑名单外放行、零打扰，ADR-033）
- [ ] 若开启了 `[logging]` 文件日志，日志文件非空且按天轮转
- [ ] 首轮对话可用（模型服务已配置；未配置时前端显示配置向导）

## 附：核实来源

`gui-native/installer/build-msi.ps1`（版本注入 + 产物名 + 门禁）· `gui-native/installer/tianyan.wxs` ·
`.github/workflows/release.yml` · `scripts/check-version.ps1` · `gui-native/README.md`「出包一条链」。

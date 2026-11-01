# 发布：native MSI（GPUI 客户端）

> **当前唯一发布渠道 = `gui-native`（GPUI 原生客户端，内嵌 server，单 exe）。**
> `tauri/` 已于 2026-10 删除，`scripts/build.ps1` / `build.sh`（tauri 打包脚本）一并删除；`gui-vite/` 保留做对照，**不随包发布**。
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

### 2.1 静默安装与同版本升级（已实测）

产品包是 `Scope="perUser"`（装到 `%LOCALAPPDATA%\Programs\Tianyan\`），**全程无 UAC**。
装（§2 出包后执行；约 1.3–3 秒）：

```powershell
msiexec /i installer\tianyan-native-<ver>-x64.msi /qn /norestart   # 静默，实测 1.3–3 秒
```

| 场景 | 结果 |
|---|---|
| perMachine（`ALLUSERS=1`） | 需管理员提权 → 每次更新必弹 UAC |
| **perUser** | ✅ `msiexec /i /qn` 静默安装成功，1.3 秒，无 UAC |
| 不同版本升级（MajorUpgrade） | ✅ 正确替换，注册表恒单条目，**无新旧并存** |
| **同版本号重新构建**（修 bug 但版本仍是 1.1.0） | ✅ 开 `AllowSameVersionUpgrades="yes"` 后走 MajorUpgrade 序（先卸旧、再装新），ARP 恒单条目，实测 3.3 秒；**不开则报 1638**（已安装此产品的另一个版本） |
| 应用运行中执行升级 | ⚠️ msiexec 退出码 0、版本已升，**但进程被 Restart Manager 直接杀掉**（无提示，也不帮你重启） |

**为什么必须开 `AllowSameVersionUpgrades`**：`installer/tianyan.wxs` 未显式指定 `ProductCode`，
WiX **每次 `wix build` 都生成新 GUID**，而 MSI 把「同版本 + 不同 ProductCode」判成**另一个产品**
→ 默认直接拒绝（错误 1638）。改 bug 但版本号仍保持 1.1.0（不擅自升版本）就会踩到，
因此该属性在 `tianyan.wxs` 里**固定开着**。不同版本号的升级行为不受影响
（`Upgrade` 表同版本行 `VersionMax=当前版本` / `Attributes=513`，降级检测行
`VersionMin=当前版本` / `Attributes=2`）。

**升级前后版本比对的做法**：用 `WindowsInstaller.Installer` COM 的
`ProductInfo(productCode, 'VersionString')` 读已安装版本（4 段 `1.1.0.0`），
装前记一次、装后再读一次比对；ARP（设置 → 应用）显示的是 3 段 `DisplayVersion`
`1.1.0`，两者不是同一口径，别混着比。

卸载**不删数据**（数据目录由配置 `storage.data_dir` 决定，ADR-023），见 §4「装到哪、数据在哪」。

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

## 6. 自动更新：当前没有（若要做，编排清单在这）

**现状**：native 客户端**没有自动更新**。发布件随附 `SHA256SUMS.txt`，升级 = 下载新 MSI 手工安装
（同版本号重建要靠 `AllowSameVersionUpgrades`，见 §2.1）。不产出 `latest.json` 的理由见 §3
（那是 tauri updater 的清单，带 `.sig`；native 没有自动更新，硬塞只会让残留客户端拿到「格式对但签名缺失」的更新而报错）。

若以后要做自研 updater，**必须做的编排（约 200–400 行）**：

1. 检查更新（HTTP 拉 release 清单，比对版本）
2. 下载 MSI + 校验（sha256，或沿用 minisign 签名）
3. **主动优雅关闭自己**（保存状态；否则会被 Restart Manager 强杀）
4. 以分离进程调 `msiexec /i <msi> /qn /norestart`
5. 成功后**重新启动新版本**；退出码 `3010` 表示需重启
6. 失败要可见（日志 + 提示），MSI 自身有事务回滚

## 附：核实来源

`gui-native/installer/build-msi.ps1`（版本注入 + 产物名 + 门禁）· `gui-native/installer/tianyan.wxs` ·
`.github/workflows/release.yml` · `scripts/check-version.ps1` · `scripts/verify-gui-native.ps1`（发布前断言）。

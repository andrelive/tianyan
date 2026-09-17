# 天演 MSI 打包与发布（release-msi）

> 适用版本：0.5.1（撰写基线 HEAD `19c6d2a`）
> 覆盖：本地打包（Windows）、版本号落点、CI 发布链、已知坑、安装验收清单。
> 全部结论取自脚本/配置/日志实读，未核实项标注 **待核实**。

---

## 1. 版本号落点（**哪一处决定 MSI 文件名与版本**）

| 文件 | 当前值 | 是否决定 MSI 版本 | 说明 |
| --- | --- | --- | --- |
| `tauri/tauri.conf.json` → `version` | **`0.5.1`** | ✅ **是**（唯一权威） | **MSI 文件名与 ProductVersion 由它决定**（安装后在「应用和功能」可见） |
| `Cargo.toml` → `[workspace.package] version` | `0.5.1` | ❌ 否（但决定 **exe 文件属性**） | 各 crate 版本 + 编译产物的 `FileVersion`/`ProductVersion`。**0.3.16 起与产品版本统一**：此前为 `0.1.0`，导致 exe 属性里显示旧版本（MSI 名不受影响） |
| `gui-vite/package.json` → `version` | `0.1.0` | ❌ 否 | 前端包版本（name `tianyan-gui`），不参与 MSI |
| `docs/release/RELEASE_NOTES.md` | `0.5.1` | ❌ 否 | 发布说明（人工维护） |
| `CHANGELOG.md` | `0.5.1` | ❌ 否 | 变更日志（人工维护） |

**结论（务必记住）**

- **MSI 文件名 = `Tianyan_<tauri.conf.json.version>_x64_<lang>.msi`**，
  即 `<productName>_<version>_x64_<language>.msi`。
- 语言取 `tauri.conf.json.bundle.windows.wix.language = ["zh-CN","en-US"]` → **一次构建产出两个 MSI**：
  - `target/release/bundle/msi/Tianyan_0.5.1_x64_zh-CN.msi`
  - `target/release/bundle/msi/Tianyan_0.5.1_x64_en-US.msi`
- **只改 `Cargo.toml` / `package.json` 的版本不会改变 MSI 名**——必须改 `tauri/tauri.conf.json`
  的 `version`。发布时四处（`tauri.conf.json` / `Cargo.toml`（workspace）/ CHANGELOG / RELEASE_NOTES）应保持一致。

> ✅ **已修正（0.3.16）**：`RELEASE_NOTES.md` 的「安装与使用」此前写下载包为
> `tianyan_0.2.0_x64.msi`（全小写、无语言段），现改为 `Tianyan_<版本>_x64_<lang>.msi`，
> 并纠正「自签名」表述（实际未做 Windows 代码签名；`.sig` 是 Tauri 更新签名）。
> 同时 `Cargo.toml` workspace 版本由 `0.1.0` 统一为 `0.3.16`（exe 文件属性随之一致）。

---

## 2. 本地打包流程（`scripts/build.ps1` 实读）

`.\scripts\build.ps1`（默认构建模式）步骤：

1. **依赖检查**（`Check-Dependencies`）：
   - `cargo` 必须存在；
   - `node`、`npm` 必须存在（Node.js 18+）；
   - `tauri-cli`：用 `cargo install --list | Select-String "tauri-cli"` 探测，缺失则 `cargo install tauri-cli`。
2. **构建前端**（`Build-Gui`）：`cd gui-vite` → `npm install` → `npm run build`
   （`npm run build` = `tsc -b && vite build`），产物 `gui-vite/dist/`。
   - 可用 `-SkipGui` 跳过。
3. **构建 Tauri**（`Build-Tauri`）：`cd tauri` → `cargo tauri build`；
   - `tauri.conf.json.beforeBuildCommand` 会**再跑一次** `npm run build`（cwd `../gui-vite`）
     —— 所以第 2 步与第 3 步的前端构建**是重复的**（见 §4 已知坑）。
4. **打印产物路径**：脚本查 `tauri\target\release\bundle`（**此路径与实际不符**，见 §4），
   列出 `*.msi` / `*.exe` / `*.msi.zip`。

其他开关：`-Dev`（`cargo tauri dev` 开发模式）、`-Verbose`。
脚本顶部设 `$ErrorActionPreference = "Continue"`，成败以 `$LASTEXITCODE` 判定（见排障文档 §5）。

### 2.1 产物路径（实际）

```
<仓库根>\target\release\bundle\msi\Tianyan_<ver>_x64_zh-CN.msi
<仓库根>\target\release\bundle\msi\Tianyan_<ver>_x64_en-US.msi
```

> 注意是**仓库根的 `target/`**（workspace 共享 target-dir），**不是** `tauri/target/`。

### 2.2 耗时量级（据根目录 `build-msi*.log`）

| 日志 | 命令 | release 编译耗时 | MSI 产物 |
| --- | --- | --- | --- |
| `build-msi.log` | `cargo tauri build --no-bundle` | `Finished release ... in 11m 13s` | 无（--no-bundle） |
| `build-msi-2.log` | `cargo tauri build` | `Finished ... in 12m 05s` | `Tianyan_0.3.7_x64_{zh-CN,en-US}.msi` |
| `build-msi-3.log` | `cargo tauri build` | `Finished ... in 12m 50s` | `Tianyan_0.3.8_x64_{zh-CN,en-US}.msi` |

**实测量级：release 编译约 11–13 分钟**（首次全量含依赖更久；CI 首次可达 ~2 小时，见 release.yml 注释）。
（任务描述中的“约 9 分钟”与日志不符，以日志为准。）

---

## 3. CI 发布链

### 3.1 `release.yml`（按 tag 发布）

触发：`push` tag `v*`。Job `build`（`windows-latest`，`permissions: contents: write`）。

步骤（实读）：

1. `actions/checkout@v5`
2. `actions/setup-node@v5`（node 24，npm 缓存 `gui-vite/package-lock.json`）
3. `dtolnay/rust-toolchain@stable`
4. `Swatinem/rust-cache@v2`（Rust 编译缓存）
5. `cd gui-vite && npm ci`
6. `cd gui-vite && npm run build`
7. **`arduino/setup-protoc@v3`**（安装 protoc —— `lance-encoding` 构建依赖）
8. `cargo install tauri-cli --locked`
9. `cd tauri && cargo tauri build`（注入 `TAURI_SIGNING_PRIVATE_KEY` /
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 做 **updater 签名**）
10. **生成 `latest.json`**（pwsh）：从 tag 去 `v` 得 version，读第一个 `*.msi` 的 `*.sig` 签名，
    拼 `platforms.windows-x86_64.{signature,url}`，写出 `latest.json`
11. `softprops/action-gh-release@v2` 上传 `*.msi` + `*.sig` + `latest.json`

> `latest.json` 是 Tauri updater 的更新清单；`tauri.conf.json.plugins.updater.endpoints`
> 指向 `https://github.com/andrelive/tianyan/releases/latest/download/latest.json`。

### 3.2 `quality.yml`（0.3.15 新增的质量门禁）

触发：`push` 到 `master` + 所有 `pull_request`。Job `quality`（`ubuntu-latest`）。

门禁项（实读）：

| 步骤 | 命令 |
| --- | --- |
| Rust fmt | `cargo fmt --all -- --check` |
| Rust clippy（deny warnings） | `cargo clippy --workspace --exclude tianyan-tauri --all-targets -- -D warnings` |
| Rust 单测 | `cargo test --workspace --exclude tianyan-tauri --lib` |
| 工具目录 freshness | `cargo run -q -p tianyan-core --example tool_catalog -- --check` |
| 前端依赖 | `cd gui-vite && npm ci` |
| 前端 lint | `cd gui-vite && npm run lint`（eslint + prettier） |
| 前端 typecheck | `cd gui-vite && npm run typecheck` |
| 前端单测 | `cd gui-vite && npm test`（vitest） |

> 注：两个 job 均**排除 `tianyan-tauri`**（桌面依赖在 ubuntu/CI 上不便构建）。

---

## 4. 已知坑

| 坑 | 说明 | 规避 |
| --- | --- | --- |
| **同版本号 MSI 覆盖/缓存** | 版本号不变时重新打包会**覆盖同名 MSI**；updater 按 `version` 判断，同版本**不会触发更新** | 发布前务必递增 `tauri/tauri.conf.json` 的 `version` |
| **SmartScreen 警告** | `bundle.windows.certificateThumbprint = null` → MSI **无 Authenticode 代码签名**（`TAURI_SIGNING_PRIVATE_KEY` 只是 updater 签名，不等于代码签名），首次运行触发 SmartScreen | 用户点“仍要运行”；如需消除需自备代码签名证书并填 `certificateThumbprint` |
| **动态端口** | 桌面端首选 3000，被占用则动态递增；实际端口经 `window.__TIANYAN_API_BASE__` 注入前端；CSP `connect-src` 已放行 `http://localhost:*` / `http://127.0.0.1:*` | 排障时别写死 3000；看日志“端口 3000 已被占用，动态选择端口 N” |
| **打包时后端占用 build 锁 / 数据文件锁** | 本地若还有 `tianyan-tauri.exe` / `tianyan-server` 在跑，会**抢 build 锁**（`Blocking waiting for file lock`）并占用 `tianyan.db` | 打包前 `taskkill` 掉运行中的天演进程，并等待其它 cargo 构建结束 |
| **脚本产物路径错误** | `build.ps1` 列产物查的是 `tauri\target\release\bundle`，实际产物在 `<仓库根>\target\release\bundle`（workspace 共享 target） | 以 §2.1 实际路径为准；或手动 `Get-ChildItem <仓库根>\target\release\bundle\msi\*.msi` |
| **本地缺 protoc** | `release.yml` 装了 protoc（`lance-encoding` 构建依赖），但 **`build.ps1` 没有**该步骤 | 本地首次打包前确保 `protoc` 在 PATH（或安装 `protoc`），否则 lancedb/lance 相关编译可能失败 |
| **前端重复构建** | `build.ps1` 先 `npm run build`，`cargo tauri build` 的 `beforeBuildCommand` 又跑一次 | 可用 `-SkipGui` 依赖 `beforeBuildCommand`（注意 `beforeBuildCommand` 仍会执行） |
| **PowerShell 5.1 stderr 陷阱** | `cargo ... 2>&1 \| ...` 会把 cargo 的 warning/Info 当错误抛出 | 见排障文档 §5；`$ErrorActionPreference="Continue"` + `$LASTEXITCODE` |

---

## 5. 验收清单（装前 / 装后）

### 5.1 装前（打包/分发）

- [ ] `tauri/tauri.conf.json` 的 `version` 与本次发布版本一致（决定 MSI 文件名）。
- [ ] 四处版本一致：`tauri.conf.json`、`Cargo.toml`（workspace）、`CHANGELOG.md`、`docs/release/RELEASE_NOTES.md`
      （`package.json` 的 `0.1.0` 不影响 MSI，见 §1）。
- [ ] 无天演进程在运行（`Get-Process tianyan-tauri,tianyan-server -ErrorAction SilentlyContinue`）。
- [ ] 无其它 cargo 构建在抢锁。
- [ ] 本机有 `protoc`（若从零编译 lancedb）。
- [ ] `target/release/bundle/msi/` 下产出**两份** MSI（`zh-CN` + `en-US`），文件名版本正确。

### 5.2 装后（安装 MSI 后首次启动）

- [ ] **版本号**：应用内“关于/更新检查”显示 `0.5.1`（与 MSI 版本一致）。
- [ ] **思考语言**：新会话内部思考为**中文**（默认 soul「思考语言」约束；需重启应用生效）。
- [ ] **审批默认自主**：默认 `approval_mode = autonomous`（黑名单外全放行、零打扰，ADR-033）；
      如需收紧可设为 `confirm` / `interactive`。
- [ ] **文件日志**：`%APPDATA%\com.tianyan.app\logs\tianyan_<时间戳>.log` **非空**、
      内容随操作增长（0.3.15 修复 `file_filter` 后应稳定）。
- [ ] **日志轮转/保留**：同目录文件数 ≤ `[logging].max_files`（默认 5，含当前文件）；
      写满 `max_file_size`（默认 10 MB）时出现 `tianyan_<时间戳>.1.log` 等同族轮转文件。
- [ ] **服务可达**：`http://127.0.0.1:3000/health`（端口被占用时以日志中的动态端口为准）返回 200。
- [ ] **数据/配置就位**：`%LOCALAPPDATA%\tianyan\`（数据）、`~/.tianyan/tianyan.toml`（配置）存在。
- [ ] （发布链）GitHub Release 含 `*.msi`、`*.sig`、`latest.json`；`latest.json.version` 与 tag 一致。

---

## 附：核实来源

- `tauri/tauri.conf.json`（`productName`/`version`/`bundle.targets=msi`/`wix.language`/`updater`）
- `tauri/Cargo.toml`（`version.workspace = true`）、`Cargo.toml`（`[workspace.package] version=0.1.0`）
- `gui-vite/package.json`（`version`、`build` 脚本）
- `scripts/build.ps1`（依赖检查 / Build-Gui / Build-Tauri / 产物枚举）
- `.github/workflows/release.yml`、`.github/workflows/quality.yml`
- `build-msi.log`、`build-msi-2.log`、`build-msi-3.log`（耗时与产物名证据）
- `target/release/bundle/msi/`（实际产物目录）
- `docs/release/RELEASE_NOTES.md`、`CHANGELOG.md`（版本与发布说明）

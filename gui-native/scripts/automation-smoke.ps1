# GUI 自动化端口冒烟验证（ADR-052 阶段 1）
#
# 用法（**必须用 pwsh**：PowerShell 5.1 会把无 BOM 的 UTF-8 中文读成乱码）：
#   $env:TIANYAN_GUI_AUTOMATION_PORT = "3010"
#   .\target\debug\tianyan-gui-native.exe        # 另开一个终端
#   pwsh -NoProfile -File .\scripts\automation-smoke.ps1
#
# 全程不截图、不点坐标：全部走 GET /state 与 POST /act。断言的是界面**内部状态**
# （消息条数 / 正文头 / 目录层级 / diff 行数 / 绑定后的工作目录），不是像素。
#
# 注意：第 5 段会真的调 `PUT /sessions/{id}/workspace`，但**选回原目录**（同值幂等），
# 不改动任何会话绑定。想跳过就加 `-SkipBinding`。
param(
    [string]$BaseUrl = "http://127.0.0.1:3010",
    [switch]$SkipBinding
)

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

$script:fail = 0
function Check($cond, $msg) {
    if ($cond) { Write-Host "  [ok] $msg" -ForegroundColor Green }
    else { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:fail++ }
}

function Act {
    param([hashtable]$Payload)
    $json = $Payload | ConvertTo-Json -Compress -Depth 8
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    try {
        return Invoke-RestMethod -Uri "$BaseUrl/act" -Method Post -Body $bytes `
            -ContentType "application/json; charset=utf-8" -TimeoutSec 40
    } catch {
        Write-Host "  ACT 请求失败：$_" -ForegroundColor Red
        $script:fail++
        return $null
    }
}

function State {
    param([int]$Settle = 0)
    try { return Invoke-RestMethod -Uri "$BaseUrl/state?settle_ms=$Settle" -TimeoutSec 30 }
    catch { Write-Host "  STATE 请求失败：$_" -ForegroundColor Red; $script:fail++; return $null }
}

Write-Host "== 0. health ==" -ForegroundColor Cyan
try {
    $h = Invoke-RestMethod -Uri "$BaseUrl/health" -TimeoutSec 10
    Write-Host ("  service={0}  pid={1}" -f $h.service, $h.pid)
    Check ($h.service -eq "tianyan-gui") "端口上是天演 GUI"
} catch {
    Write-Host "  端口 $BaseUrl 不可达 —— 是否忘了设 TIANYAN_GUI_AUTOMATION_PORT？" -ForegroundColor Red
    exit 1
}

Write-Host "== 1. 初始状态 ==" -ForegroundColor Cyan
$s = State
"  view={0}  session.count={1}  messages.loaded={2}/{3}" -f `
    $s.state.view, $s.state.session.count, $s.state.messages.loaded, $s.state.messages.total
$rows = @($s.state.sessions_rows)
"  sessions_rows={0}（group={1} / session={2} / more={3}）" -f $rows.Count,
    @($rows | Where-Object { $_.kind -eq "group" }).Count,
    @($rows | Where-Object { $_.kind -eq "session" }).Count,
    @($rows | Where-Object { $_.kind -eq "more" }).Count
Check ($rows.Count -gt 0) "左栏有派生行（分组/会话/查看更多）"
Check (@($s.state.views).Count -eq 11) "11 个一级入口"

Write-Host "== 2. switch_tab（侧栏点击与自动化共用 switch_view）==" -ForegroundColor Cyan
$r = Act @{ action = "switch_tab"; view = "workspace"; settle_ms = 2500 }
"  view={0}  ws.root='{1}'  ws.status='{2}'" -f $r.state.view, $r.state.workspace.root, $r.state.workspace.status
Check ($r.state.view -eq "workspace") "切到文件页"
# 注意：未选中会话时工作区根可能是**空目录**（如 F:\workspace），所以这里只要求根非空，
# 「有条目」留到第 4 段（已选中工作区非空的会话）再断言。
Check ($r.state.workspace.root -ne "") "首次切页即拉到工作区根（不依赖点侧栏）"

$r = Act @{ action = "switch_tab"; view = "roles"; settle_ms = 800 }
"  roles.count={0}  stats.total_calls={1}" -f $r.state.roles.count, $r.state.roles.stats.total_calls

Write-Host "== 3. 选会话 + 消息断言 ==" -ForegroundColor Cyan
$s = State
$target = @($s.state.sessions_data) | Where-Object { $_.working_directory } |
    Sort-Object updated_at -Descending | Select-Object -First 1
if (-not $target) { $target = @($s.state.sessions_data) | Sort-Object updated_at -Descending | Select-Object -First 1 }
"  选中 {0}（wd={1}）" -f $target.id, $target.working_directory
$r = Act @{ action = "select_session"; id = $target.id; settle_ms = 2500 }
"  messages.loaded={0}/{1}  streaming={2}" -f `
    $r.state.messages.loaded, $r.state.messages.total, $r.state.streaming
Check ($r.state.session.current -eq $target.id) "当前会话已切换"
Check ($r.state.messages.loaded -gt 0) "消息已加载"
$last = $r.state.messages.last
if ($last) {
    "  最后一条：role={0}  chars={1}  segs={2}  tool_calls={3}" -f `
        $last.role, $last.chars, $last.segs, $last.tool_calls
    "  正文头：{0}" -f (($last.head -replace "`r?`n", " / ") | ForEach-Object { if ($_.Length -gt 90) { $_.Substring(0, 90) + "…" } else { $_ } })
    Check ($last.chars -gt 0) "最后一条消息有正文（以前只能靠看图确认）"
    Check ($last.segs -ge 0) "结构化分段已解析（segs=$($last.segs)）"
}

Write-Host "== 4. 文件页：树 / 进子目录 / 回根 / 开文件 ==" -ForegroundColor Cyan
$r = Act @{ action = "switch_tab"; view = "workspace"; settle_ms = 2500 }
$entries = @($r.state.workspace.entries)
"  ws.root='{0}'  entries={1}" -f $r.state.workspace.root, $entries.Count
Check ($entries.Count -gt 0) "目录树有条目"
if ($entries.Count -gt 0) {
    "  前几项：{0}" -f (($entries | Select-Object -First 6 | ForEach-Object { "$($_.name)$(if ($_.dir) { '/' })" }) -join ", ")
}
$dir = $entries | Where-Object { $_.dir } | Select-Object -First 1
$file = $entries | Where-Object { -not $_.dir } | Select-Object -First 1
if ($dir) {
    $r1 = Act @{ action = "ws_enter"; path = $dir.path; settle_ms = 2000 }
    "  ws_enter('{0}') -> path='{1}'  entries={2}" -f $dir.path, $r1.state.workspace.path, @($r1.state.workspace.entries).Count
    Check ($r1.state.workspace.path -eq $dir.path) "进子目录后 ws_path 正确"
    $r2 = Act @{ action = "ws_enter"; path = ""; settle_ms = 2000 }
    "  ws_enter('') 回根 -> path='{0}'  entries={1}" -f $r2.state.workspace.path, @($r2.state.workspace.entries).Count
    Check ($r2.state.workspace.path -eq "") "回根后 ws_path 清空"
}
if ($file) {
    $r3 = Act @{ action = "ws_open_file"; path = $file.path; settle_ms = 2500 }
    $f = $r3.state.workspace.file
    "  ws_open_file('{0}') -> path={1}  lines={2}  chars={3}" -f $file.path, $f.path, $f.lines, $f.chars
    Check ($f.lines -gt 0) "文件内容已加载"
}

Write-Host "== 5. diff 面板（set 语义，不是 toggle）==" -ForegroundColor Cyan
$r = Act @{ action = "ws_diff"; open = $true; settle_ms = 3000 }
$files = @($r.state.workspace.diff.files)
"  open={0}  files={1}  status='{2}'" -f $r.state.workspace.diff.open, $files.Count, $r.state.workspace.diff.status
Check ($r.state.workspace.diff.open -eq $true) "diff 面板已打开"
Check ($files.Count -gt 0) "改动文件列表非空"
if ($files.Count -gt 0) {
    # 注意 old_lines/new_lines 是**文件总行数**（旧/新），不是改动行数；改动行数在 single diff 里
    $files | Select-Object -First 5 | ForEach-Object { "    {0,-9} {1}  ({2} 行 → {3} 行)" -f $_.status, $_.path, $_.old_lines, $_.new_lines }
    $one = $files | Where-Object { $_.status -ne "binary" } | Select-Object -First 1
    if (-not $one) { $one = $files[0] }
    $r2 = Act @{ action = "ws_diff_open"; path = $one.path; settle_ms = 3000 }
    $cur = $r2.state.workspace.diff.current
    "  ws_diff_open('{0}') -> {1} 行  +{2} -{3}" -f $one.path, $cur.lines, $cur.added, $cur.removed
    Check ($cur.lines -gt 0) "unified diff 已渲染"
}
$r = Act @{ action = "ws_diff"; open = $false }
Check ($r.state.workspace.diff.open -eq $false) "可显式关闭"

Write-Host "== 6. picker 内层导航 + 真实绑定（PUT /sessions/{id}/workspace）==" -ForegroundColor Cyan
$r = Act @{ action = "open_picker"; target = "session"; settle_ms = 1500 }
"  open={0}  current='{1}'  dialog={2}" -f $r.state.picker.open, $r.state.picker.current, ($r.state.dialog | ConvertTo-Json -Compress)
Check ($r.state.picker.open -eq $true) "选择器已打开"
Check ($r.state.dialog.kind -eq "workspace_picker") "对话框状态为 workspace_picker"

# 逐级导航到该会话的**原**工作目录（同值幂等，不污染数据）
$segs = $target.working_directory -split '\\' | Where-Object { $_ -ne '' }
$cur = ""
foreach ($seg in $segs) {
    $want = if ($cur -eq "") { "$seg\" } else { "$cur\$seg" }
    $r = Act @{ action = "picker_enter"; name = $seg; settle_ms = 1500 }
    if (-not $r.ok) {
        # 第一级是盘符，列表里的名字带反斜杠（`F:\`），按名字匹配不到就走绝对路径
        $r = Act @{ action = "picker_enter"; path = $want; settle_ms = 1500 }
    }
    $cur = $r.state.picker.current
    "    enter '{0}' -> current='{1}'  entries={2}" -f $seg, $cur, @($r.state.picker.names).Count
}
Check ($cur -eq $target.working_directory) "逐级导航到目标目录"

if (-not $SkipBinding -and $cur -eq $target.working_directory) {
    $r = Act @{ action = "picker_confirm"; settle_ms = 3500 }
    "  picker_confirm -> ok={0}  picker.open={1}" -f $r.ok, $r.state.picker.open
    Check ($r.state.picker.open -eq $false) "确认后选择器关闭"
    $after = @($r.state.sessions_data) | Where-Object { $_.id -eq $target.id }
    "  绑定后 working_directory='{0}'  ws.root='{1}'" -f $after.working_directory, $r.state.workspace.root
    Check ($after.working_directory -eq $target.working_directory) "PUT workspace 生效（同值）"
    Check ($r.state.workspace.root -ne "") "绑定后按新根重拉目录树"
} else {
    Act @{ action = "picker_cancel" } | Out-Null
    Write-Host "  （已取消，未做绑定验证）" -ForegroundColor Yellow
}

Write-Host "== 6.5 @ 文件引用（按钮 → 弹窗 → 光标处插入）==" -ForegroundColor Cyan
# 先在输入框里放一段文字，验证插入是「插在光标处」而不是「覆盖整个输入框」
$r = Act @{ action = "composer_clear" }
Check ($r.state.usage.composer_text -eq "") "输入框已清空（测试前置）"

$r = Act @{ action = "mention_file"; settle_ms = 1500 }
Check ($r.state.dialog.kind -eq "mention_file_picker") "打开的是引用弹窗（不是知识导入弹窗）"
Check ($r.state.dialog.purpose -eq "mention") "弹窗用途 = mention（自动化可区分）"
Check ($r.state.knowledge.browse.dir -ne "") "弹窗已定位到起始目录（$($r.state.knowledge.browse.dir)）"

$files = @($r.state.knowledge.browse.entries) | Where-Object { -not $_.is_dir }
Check ($files.Count -gt 0) "起始目录有可选文件（$($files.Count) 个）"
if ($files.Count -gt 0) {
    $f = $files | Select-Object -First 1
    $r2 = Act @{ action = "mention_pick"; path = $f.path; settle_ms = 500 }
    Check ($r2.ok -eq $true) "点文件即插入（不需要勾选 + 确认两步）"
    Check ($null -eq $r2.state.dialog) "插入后弹窗自动关闭"

    # 契约断言：序列化口径必须与 Web 版 `serializeInlineChip` 逐字一致
    # （Markdown 行内代码 + 绝对路径），否则同一句话在两个客户端里格式不同
    $expect = '@file:"' + $f.path + '"'
    $text = [string]$r2.state.usage.composer_text
    Check ($text.Contains($expect)) "输入框含 @file 引用路径"
    Check ($text.Contains('`' + $expect + '`')) "序列化口径 = Markdown 行内 code（与 Web 版一致）"
    Write-Host ("  插入结果：{0}" -f $text.Trim())

    # 连续引用两个文件：末尾补了空格，两个 chip 不会粘在一起
    $r3 = Act @{ action = "mention_pick"; path = $f.path; settle_ms = 300 }
    $text3 = [string]$r3.state.usage.composer_text
    # 用正则计数而不是 `.Split()`：PowerShell 的 `String.Split(string)` 会退化成
    # 按**字符**切分（`Split('.')` 切句号就是这么来的），数出来永远是 0
    $hits = ([regex]::Matches($text3, [regex]::Escape('@file:"'))).Count
    Check ($hits -eq 2) "可连续引用同一个文件（插入不覆盖已有内容，出现 $hits 次）"
}

# 收尾：别让测试文本留在用户输入框里
$r = Act @{ action = "composer_clear" }
Check ($r.state.usage.composer_text -eq "") "收尾清空输入框（不污染下次发送）"

Write-Host "== 6.6 系统通知（D1：AUMID 注册 → 全局通道 → WinRT）==" -ForegroundColor Cyan
# 通知是否真的弹出来只能截图确认（系统通知浮在窗口之外，/state 读不到）；
# 能自动断言的是「链路没断」+「归属名不是系统默认的 Windows PowerShell」。
$appId = Get-ItemProperty "HKCU:\Software\Classes\AppUserModelId\Tianyan.GuiNative" -ErrorAction SilentlyContinue
Check ($null -ne $appId) "AppUserModelID 已注册（HKCU，免管理员）"
Check ($appId.DisplayName -eq "天演") "通知归属名 = 天演（不是 Windows PowerShell）"
$r = Act @{ action = "test_notification"; settle_ms = 600 }
Check ($r.ok -eq $true) "测试通知经全局通道发出（注册 → 弹窗链路通）"

Write-Host "== 6.7 系统托盘（D2：图标 / 显隐 / 关闭到托盘）==" -ForegroundColor Cyan
$r = Act @{ action = "switch_tab"; view = "chat"; settle_ms = 400 }
Check ($r.state.usage.tray.available -eq $true) "已定位主窗口（托盘显隐的句柄来源）"
Check ($r.state.usage.tray.window_visible -eq $true) "主窗口当前可见"

# 隐藏：窗口从任务栏消失（真 hide，不是 minimize —— 后者任务栏仍有按钮）
$r = Act @{ action = "tray_hide"; settle_ms = 600 }
Check ($r.state.usage.tray.window_visible -eq $false) "隐藏后窗口不可见"
# 这条是回归断言：句柄按 IsWindowVisible 枚举时，藏起来就再也找不回来，
# 「再显示」会静默失效（实测踩过）。缓存后隐藏态仍须持有句柄。
Check ($r.state.usage.tray.available -eq $true) "隐藏后仍持有句柄（否则「再显示」永久失效）"

$r = Act @{ action = "tray_show"; settle_ms = 600 }
Check ($r.state.usage.tray.window_visible -eq $true) "显示后窗口恢复可见"

# 关闭按钮 → 藏到托盘（进程不退）。走 WM_CLOSE，与用户点标题栏 × 完全同一条路径。
$r = Act @{ action = "window_close"; settle_ms = 900 }
Check ($r.state.usage.tray.window_visible -eq $false) "点关闭按钮 = 藏到托盘（不是退出）"
Check ($r.state.usage.tray.quitting -eq $false) "未进入退出状态（能被 /act 应答即进程仍活）"

$r = Act @{ action = "tray_show"; settle_ms = 600 }
Check ($r.state.usage.tray.window_visible -eq $true) "从托盘恢复窗口"

Write-Host "== 6.8 全局快捷键（D3：动作路径；真实按键见 simulate-hotkey.ps1）==" -ForegroundColor Cyan
Check ($null -ne $r.state.usage.hotkey) "快捷键状态可读（combo=$($r.state.usage.hotkey.combo)）"
if ($r.state.usage.hotkey.registered) {
    Check ($true) "组合键 $($r.state.usage.hotkey.combo) 已注册"
} else {
    # 不算失败：组合键被占用（例如 tauri 版正在跑，它也注册 Ctrl+Alt+T）是环境的
    # 正确行为，不是代码缺陷。真正验证按键路径的是 simulate-hotkey.ps1。
    Write-Host "  [注意] 组合键未注册（被其他程序占用？tauri 版也注册 Ctrl+Alt+T）" -ForegroundColor Yellow
}
# 动作本身（热键线程收到 WM_HOTKEY 后调的就是这个函数）：不依赖注册是否成功
$r = Act @{ action = "hotkey_toggle"; settle_ms = 600 }
Check ($r.state.usage.tray.window_visible -eq $false) "toggle 动作：可见 → 隐藏"
$r = Act @{ action = "hotkey_toggle"; settle_ms = 600 }
Check ($r.state.usage.tray.window_visible -eq $true) "toggle 动作：隐藏 → 可见"

Write-Host "== 6.9 内建截图（C：需 --features screenshot 构建）==" -ForegroundColor Cyan
$shotPath = Join-Path $env:TEMP "tianyan-smoke-shot.png"
Remove-Item $shotPath -ErrorAction SilentlyContinue
$r = Act @{ action = "screenshot"; path = $shotPath; settle_ms = 400 }
if ($r.ok -eq $true) {
    Check (Test-Path $shotPath) "截图文件已落盘"
    $dim = $r.state.usage.screenshot
    Check ($dim.width -gt 0 -and $dim.height -gt 0) "截图尺寸有效（$($dim.width)x$($dim.height)）"
    # 隐藏态截图是内建方案独有的能力：render_to_image 渲染的是 scene，
    # 不读屏幕 —— 抓屏方案此时只会拍到桌面
    $r = Act @{ action = "tray_hide"; settle_ms = 500 }
    $r = Act @{ action = "screenshot"; path = $shotPath; settle_ms = 400 }
    Check ($r.ok -eq $true) "窗口隐藏时仍能截图（抓屏方案做不到）"
    $r = Act @{ action = "tray_show"; settle_ms = 500 }
    Check ($r.state.usage.tray.window_visible -eq $true) "截图后恢复窗口"
} else {
    Write-Host "  [跳过] 本二进制未启用截图：$($r.error)" -ForegroundColor Yellow
    Write-Host "         需要像素级验证时用 cargo build --features screenshot" -ForegroundColor Yellow
}

Write-Host "== 7. 错误路径 ==" -ForegroundColor Cyan
$r = Act @{ action = "no_such_action" }
Check ($r.ok -eq $false -and $r.error) "未知动作 -> ok=false + error"
$r = Act @{ action = "switch_tab"; view = "nope" }
Check ($r.ok -eq $false) "未知视图 -> ok=false"
$r = Act @{ action = "select_session" }
Check ($r.ok -eq $false) "缺参数 -> ok=false"
$r = Act @{ action = "picker_confirm" }
Check ($r.ok -eq $false) "选择器未打开时确认 -> ok=false"

Write-Host "== 8. 收尾 ==" -ForegroundColor Cyan
$r = Act @{ action = "switch_tab"; view = "chat" }
Write-Host ("  view={0}" -f $r.state.view)
Write-Host ""
if ($script:fail -eq 0) {
    Write-Host "全部断言通过" -ForegroundColor Green
    exit 0
} else {
    Write-Host ("{0} 项断言失败" -f $script:fail) -ForegroundColor Red
    exit 1
}

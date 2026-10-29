# 原生端（GPUI）自动化验收脚本
#
# 为什么要有它：验证界面行为不能靠「反复启动 + 人眼看 + 截图比对」——那既慢，又会在
# 用户桌面上反复闪窗口（实测用户会注意到「图标出现又消失」）。正确做法是 ADR-052 的
# 自动化通道：一次启动、窗口隐藏、全部断言走 HTTP。
#
#   1. 复用：3010 上已有实例就直接用（不重启、不闪）；
#   2. 隐藏：需要新起时带 `TIANYAN_GUI_START_HIDDEN=1`，窗口建好但不显示；
#   3. 一次跑完：基础 / 设置页 14 栏目 / 字段引擎 / 用量 / 人设 / MCP / 存储 / 连接。
#
# 用法：
#   .\scripts\verify-gui-native.ps1              # 复用或隐藏启动，跑全部断言
#   .\scripts\verify-gui-native.ps1 -Restart     # 先关掉旧实例再起（改了 Rust 代码后用）
#   .\scripts\verify-gui-native.ps1 -KeepAlive   # 跑完不关（下次复用，零等待）
#
# 退出码：0 = 全部通过；1 = 有失败项。

#requires -Version 7
# ⚠️ 必须 PowerShell 7+（`pwsh`），不能用 Windows PowerShell 5.1（`powershell`）：
# 5.1 的 `Invoke-RestMethod` 在响应头没有 `charset` 时按 ISO-8859-1 解码 JSON，
# 于是「读到的中文」本身就是乱码——再写回服务端就把数据写坏（实测事故：人设被
# 编码坏两次，靠 latin1→utf8 解码才救回来）。这个 requires 是硬闸门。

param(
    [int]$Port = 3010,
    [switch]$Restart,
    [switch]$KeepAlive,
    # 用 release 构建验证（打包 MSI 前必跑这一遍：debug 与 release 的 PE
    # subsystem / 优化行为不同，出包前要验的是 release 那份）
    [switch]$Release
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$repo = Split-Path -Parent $PSScriptRoot
$exe = if ($Release) {
    Join-Path $repo "gui-native\target\release\tianyan-gui-native.exe"
} else {
    Join-Path $repo "gui-native\target\debug\tianyan-gui-native.exe"
}
$base = "http://127.0.0.1:$Port"

$script:pass = 0
$script:fail = 0
$script:failures = @()

function Check {
    param([string]$Name, [bool]$Cond, [string]$Detail = "")
    if ($Cond) {
        $script:pass++
        Write-Host "  OK   $Name" -ForegroundColor DarkGreen
    } else {
        $script:fail++
        $script:failures += $Name
        Write-Host "  FAIL $Name $Detail" -ForegroundColor Red
    }
}

function Act {
    param([hashtable]$Payload, [int]$SettleMs = 400)
    $body = @{} + $Payload
    $body["settle_ms"] = $SettleMs
    $json = $body | ConvertTo-Json -Depth 8 -Compress
    # ⚠️ 必须「落 UTF-8 文件 → -InFile」发送，不能把中文直接塞 `-Body`：
    # PowerShell 会按 ANSI/ISO 编码处理非 ASCII 正文，服务端收到的既是**乱码又是
    # 非法码点**（实测事故：一次 soul_set + 两次 PUT 让人设被三次编码写坏，
    # 只能靠 latin1→utf8 反复解码救回来）。落文件是最不容易再踩的写法。
    $tmp = Join-Path $env:TEMP ("gui-native-act-" + [guid]::NewGuid().ToString("N") + ".json")
    [System.IO.File]::WriteAllText($tmp, $json, (New-Object System.Text.UTF8Encoding($false)))
    try {
        return Invoke-RestMethod -Method Post -Uri "$base/act" -ContentType "application/json; charset=utf-8" -InFile $tmp -TimeoutSec 60
    } catch {
        throw "act 失败（$($Payload.action)）：$_"
    } finally {
        Remove-Item -LiteralPath $tmp -ErrorAction SilentlyContinue
    }
}

function Put-JsonFile {
    param([string]$Uri, [hashtable]$Payload)
    $json = $Payload | ConvertTo-Json -Depth 8 -Compress
    $tmp = Join-Path $env:TEMP ("gui-native-put-" + [guid]::NewGuid().ToString("N") + ".json")
    [System.IO.File]::WriteAllText($tmp, $json, (New-Object System.Text.UTF8Encoding($false)))
    try {
        return Invoke-RestMethod -Method Put -Uri $Uri -ContentType "application/json; charset=utf-8" -InFile $tmp -TimeoutSec 30
    } finally {
        Remove-Item -LiteralPath $tmp -ErrorAction SilentlyContinue
    }
}

function Wait-Until {
    param([scriptblock]$Probe, [int]$TimeoutSec = 12)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        if (& $Probe) { return $true }
        Start-Sleep -Milliseconds 400
    }
    return $false
}

function StateOf {
    param($Resp)
    # `/act` 的响应是 `{ok, state: <界面状态>}`——`state` 本身就是状态对象（不再套一层）
    if ($null -eq $Resp) { return $null }
    return $Resp.state
}

function Get-State {
    param([int]$SettleMs = 0)
    $r = Invoke-RestMethod -Uri "$base/state?settle_ms=$SettleMs" -TimeoutSec 60
    return $r.state
}

function Wait-Ready {
    param([int]$TimeoutSec = 60)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $h = Invoke-RestMethod -Uri "$base/health" -TimeoutSec 2
            if ($h.ok) { return $true }
        } catch { }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

# ── 1. 保证有实例（优先复用，避免反复启停） ──────────────────────────────
$running = $false
try {
    $h = Invoke-RestMethod -Uri "$base/health" -TimeoutSec 2
    $running = [bool]$h.ok
} catch { $running = $false }

if ($running -and $Restart) {
    Write-Host "关闭已有实例…" -ForegroundColor Yellow
    # ⚠️ 必须用 `quit`（= 托盘「退出天演」），不能用 `window_close`：
    # 后者被应用拦成「藏到托盘」，进程继续活着占着 exe —— 于是 `cargo build
    # --release` 报「拒绝访问」，而且每跑一次验收就攒一个隐形实例。
    try { Act @{ action = "quit" } 200 | Out-Null } catch { }
    Start-Sleep -Milliseconds 1500
    Get-Process tianyan-gui-native -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $running = $false
}

if (-not $running) {
    if (-not (Test-Path $exe)) {
        Write-Host "可执行文件不存在：$exe（先 cargo build）" -ForegroundColor Red
        exit 1
    }
    Write-Host "以隐藏窗口启动原生端…" -ForegroundColor Yellow
    $env:TIANYAN_GUI_AUTOMATION_PORT = "$Port"
    $env:TIANYAN_GUI_START_HIDDEN = "1"
    $proc = Start-Process -FilePath $exe -WorkingDirectory (Join-Path $repo "gui-native") -WindowStyle Hidden -PassThru
    if (-not (Wait-Ready -TimeoutSec 90)) {
        Write-Host "自动化端口未就绪（$base）" -ForegroundColor Red
        exit 1
    }
    Write-Host "已就绪（窗口隐藏，不打扰桌面）" -ForegroundColor DarkGreen
} else {
    Write-Host "复用已在运行的实例（$base）" -ForegroundColor DarkGreen
}

$st = Get-State
$serverBase = "http://127.0.0.1:$($st.server_port)"
Write-Host "内嵌 server：$serverBase" -ForegroundColor DarkGray

# ── 2. 基础 ────────────────────────────────────────────────────────────
Write-Host "`n[基础]" -ForegroundColor Cyan
Check "自动化端口 /health 可用" ($null -ne $st)
Check "界面已渲染（view 非空）" (-not [string]::IsNullOrEmpty($st.view))
Check "内嵌 server 端口已上报" ($st.server_port -gt 0)
$health = Invoke-RestMethod -Uri "$serverBase/health" -TimeoutSec 5
Check "内嵌 server /health 正常" ($health.status -eq "ok")

# ── 3. 设置页：14 个栏目逐个切 ─────────────────────────────────────────
Write-Host "`n[设置页栏目]" -ForegroundColor Cyan
Act @{ action = "switch_tab"; tab = "settings" } 300 | Out-Null
$tabs = @("models", "storage", "agent", "soul", "security", "logging", "memory",
    "retrieval", "appearance", "connection", "mcp", "usage", "web", "about")
foreach ($t in $tabs) {
    $r = Act @{ action = "settings_tab"; tab = $t } 500
    $s = StateOf $r
    Check "切到 $t" ($s.models_page.tab -eq $t) "（实际 $($s.models_page.tab)）"
}

# ── 4. 字段引擎（7 个可编辑栏目） ───────────────────────────────────────
Write-Host "`n[字段引擎]" -ForegroundColor Cyan
$r = Act @{ action = "settings_tab"; tab = "security" } 500
$s = StateOf $r
Check "字段总数 ≥ 40" ($s.settings_fields.field_count -ge 40) "（实际 $($s.settings_fields.field_count)）"
Check "配置段已加载" ($s.settings_fields.config_loaded -eq $true)
$editable = @($s.settings_fields.editable_tabs)
foreach ($t in @("storage", "agent", "security", "logging", "memory", "retrieval", "web")) {
    Check "可编辑栏目包含 $t" ($editable -contains $t)
}
$fields = @($s.settings_fields.fields)
Check "字段快照非空" ($fields.Count -ge 40)

$cfg = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
$origApproval = $cfg.security.approval_mode
$origMaxTurns = $cfg.agent.max_turns
$origCleanup = $cfg.storage.cleanup_days
$origMinScore = $cfg.retrieval.min_score
$origSoul = (Invoke-RestMethod -Uri "$serverBase/api/v1/config/soul" -TimeoutSec 10).content

# 4a. 枚举字段：改 → 脏 → 保存 → 后端一致
$nextApproval = if ($origApproval -eq "autonomous") { "confirm" } else { "autonomous" }
Act @{ action = "settings_set"; path = "security.approval_mode"; value = $nextApproval } 200 | Out-Null
$s = Get-State
Check "改枚举后脏计数 = 1" ($s.settings_fields.dirty_count -eq 1) "（实际 $($s.settings_fields.dirty_count)）"
$r = Act @{ action = "settings_save" } 1500
$s = StateOf $r
# 落库是异步链（GET → PUT → reload），等它真落地再断言，不靠固定 sleep 猜
$null = Wait-Until { ((Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config.security.approval_mode) -eq $nextApproval }
$cfg2 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
Check "枚举保存后脏计数归零" ($s.settings_fields.dirty_count -eq 0) "（实际 $($s.settings_fields.dirty_count)）"
Check "枚举保存后后端已落库" ($cfg2.security.approval_mode -eq $nextApproval) "（实际 $($cfg2.security.approval_mode)）"
Check "保存状态行无错误" ($s.settings_fields.msg_err -eq $false) "（$($s.settings_fields.msg)）"

# 4b. 数字字段：改 → 保存 → 后端一致
Act @{ action = "settings_text"; path = "agent.max_turns"; value = "$($origMaxTurns + 3)" } 200 | Out-Null
$s = Get-State
Check "改数字后是脏的" ($s.settings_fields.dirty_count -ge 1)
Act @{ action = "settings_save" } 1500 | Out-Null
$null = Wait-Until { ((Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config.agent.max_turns) -eq ($origMaxTurns + 3) }
$cfg3 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
Check "数字保存后后端一致" ($cfg3.agent.max_turns -eq ($origMaxTurns + 3)) "（实际 $($cfg3.agent.max_turns)）"

# 4c. 非法值：保存被拒，且后端不变
Act @{ action = "settings_text"; path = "retrieval.min_score"; value = "9" } 200 | Out-Null
$r = Act @{ action = "settings_save" } 800
$s = StateOf $r
Check "非法值被拒（状态行报错）" ($s.settings_fields.msg_err -eq $true) "（$($s.settings_fields.msg)）"
$cfg4 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
Check "非法值未污染后端" ($cfg4.retrieval.min_score -eq $origMinScore)
Act @{ action = "settings_discard" } 600 | Out-Null

# 4d. 恢复所有改动
Act @{ action = "settings_tab"; tab = "security" } 300 | Out-Null
Act @{ action = "settings_set"; path = "security.approval_mode"; value = $origApproval } 200 | Out-Null
Act @{ action = "settings_save" } 1500 | Out-Null
Act @{ action = "settings_text"; path = "agent.max_turns"; value = "$origMaxTurns" } 200 | Out-Null
Act @{ action = "settings_save" } 1500 | Out-Null
$null = Wait-Until {
    $c = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
    ($c.security.approval_mode -eq $origApproval) -and ($c.agent.max_turns -eq $origMaxTurns)
}
$cfg5 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
Check "枚举已恢复原值" ($cfg5.security.approval_mode -eq $origApproval)
Check "数字已恢复原值" ($cfg5.agent.max_turns -eq $origMaxTurns)

# ── 5. 用量统计栏目 ────────────────────────────────────────────────────
Write-Host "`n[用量统计]" -ForegroundColor Cyan
$r = Act @{ action = "usage_range"; days = 7 } 1200
$s = StateOf $r
Check "时段 7 天生效" ($s.settings_extras.usage_page.days -eq 7)
Check "用时分组默认按模型" ($s.settings_extras.usage_page.group_by -eq "model")
$r = Act @{ action = "usage_group"; group_by = "provider" } 1200
$s = StateOf $r
Check "分组可切到提供商" ($s.settings_extras.usage_page.group_by -eq "provider")
Check "用量加载无错误" ([string]::IsNullOrEmpty($s.settings_extras.usage_page.msg)) "（$($s.settings_extras.usage_page.msg)）"
Act @{ action = "usage_group"; group_by = "model" } 800 | Out-Null
Act @{ action = "usage_range"; days = 1 } 800 | Out-Null
$s = Get-State
Check "时段可切到 1 天" ($s.settings_extras.usage_page.days -eq 1)
Act @{ action = "usage_range"; days = 7 } 800 | Out-Null

# ── 6. 人设栏目 ────────────────────────────────────────────────────────
Write-Host "`n[人设]" -ForegroundColor Cyan
$r = Act @{ action = "settings_tab"; tab = "soul" } 400
Act @{ action = "soul_load" } 1200 | Out-Null
$s = Get-State
Check "人设已加载" ($s.settings_extras.soul.loaded -eq $true)
Check "默认人设可用（填入默认按钮有内容）" ($s.settings_extras.soul.default_chars -gt 0)
Check "人设正文非空" ($s.settings_extras.soul.chars -gt 0)
$r = Act @{ action = "soul_fill_default" } 600
$s = StateOf $r
Check "填入默认后与默认一致" ($s.settings_extras.soul.same_as_default -eq $true)
# 写一段可识别的正文 → 保存 → 重载 → 校验 → 恢复
$probe = "# 验收探针`n`n这是一段用于自动化验收的人设正文（跑完会恢复原文）。"
Act @{ action = "soul_set"; content = $probe } 400 | Out-Null
Act @{ action = "soul_save" } 1200 | Out-Null
Act @{ action = "soul_load" } 1200 | Out-Null
$s = Get-State
Check "人设保存后正文长度符合探针" ($s.settings_extras.soul.chars -eq $probe.Length) "（实际 $($s.settings_extras.soul.chars) / 期望 $($probe.Length)）"
# 恢复走**后端直写**：这是清理动作（跑完必须把用户人设还原），不必再经一次 UI；
# UI 写入路径已由上面的探针断言覆盖。注意用 Put-JsonFile（落文件发送），
# 否则长中文正文会被 PowerShell 编码坏掉（见 Act 里的注释）。
Put-JsonFile "$serverBase/api/v1/config/soul" @{ content = $origSoul } | Out-Null
$soulNow = (Invoke-RestMethod -Uri "$serverBase/api/v1/config/soul" -TimeoutSec 10).content
Check "人设已恢复原文" ($soulNow -eq $origSoul)

# ── 7. MCP 栏目 ────────────────────────────────────────────────────────
Write-Host "`n[MCP]" -ForegroundColor Cyan
$r = Act @{ action = "settings_tab"; tab = "mcp" } 300
$r = Act @{ action = "mcp_load" } 1200
$s = StateOf $r
$mcpCount = $s.settings_extras.mcp.count
Check "MCP 列表已加载（无错误）" ($s.settings_extras.mcp.msg_err -eq $false) "（$($s.settings_extras.mcp.msg)）"
Check "MCP 服务器数量可读" ($mcpCount -ge 0)
if ($mcpCount -gt 0) {
    $first = @($s.settings_extras.mcp.servers)[0]
    $name = $first.name
    $origEnabled = $first.enabled
    Act @{ action = "mcp_toggle"; name = $name; enabled = (-not $origEnabled) } 1500 | Out-Null
    $s = Get-State
    $now = @($s.settings_extras.mcp.servers) | Where-Object { $_.name -eq $name } | Select-Object -First 1
    Check "MCP 启停切换生效" ($now.enabled -eq (-not $origEnabled))
    Act @{ action = "mcp_toggle"; name = $name; enabled = $origEnabled } 2500 | Out-Null
    $null = Wait-Until {
        $st2 = Get-State
        $x = @($st2.settings_extras.mcp.servers) | Where-Object { $_.name -eq $name } | Select-Object -First 1
        $x.enabled -eq $origEnabled
    }
    $s = Get-State
    $now2 = @($s.settings_extras.mcp.servers) | Where-Object { $_.name -eq $name } | Select-Object -First 1
    Check "MCP 启停已恢复" ($now2.enabled -eq $origEnabled) "（实际 $($now2.enabled) / 期望 $origEnabled）"
} else {
    Write-Host "  （本机未配置 MCP 服务器：跳过启停/测试断言）" -ForegroundColor DarkGray
}

# ── 8. 模型服务（偏好切换 / 增删） ─────────────────────────────────────
Write-Host "`n[模型服务]" -ForegroundColor Cyan
$r = Act @{ action = "settings_tab"; tab = "models" } 1500
$s = StateOf $r
$models = @($s.models_page.models)
Check "模型列表非空" ($models.Count -gt 0) "（实际 $($models.Count)）"
Check "模型带能力标签原值" ($null -ne $models[0].caps)
Check "模型带三种偏好标记" ($null -ne $models[0].is_embedding -and $null -ne $models[0].is_vision)

# 8a. 偏好切换：找两个都具备 vision 的模型，互相切再切回
$visionModels = @($models | Where-Object { $_.caps -contains "vision" })
if ($visionModels.Count -ge 2) {
    $orig = $visionModels | Where-Object { $_.is_vision } | Select-Object -First 1
    $target = $visionModels | Where-Object { -not $_.is_vision } | Select-Object -First 1
    if ($orig -and $target) {
        Act @{ action = "pref_set"; provider = $target.provider; model = $target.name; kind = "vision" } 1800 | Out-Null
        $null = Wait-Until {
            $p = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config.models.preferences.vision
            ($p.model -eq $target.name) -and ($p.provider -eq $target.provider)
        }
        $pref = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config.models.preferences.vision
        Check "视觉偏好切换已落库" (($pref.model -eq $target.name) -and ($pref.provider -eq $target.provider)) "（实际 $($pref.provider)/$($pref.model)）"
        Act @{ action = "pref_set"; provider = $orig.provider; model = $orig.name; kind = "vision" } 1800 | Out-Null
        $null = Wait-Until {
            $p = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config.models.preferences.vision
            $p.model -eq $orig.name
        }
        $pref2 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config.models.preferences.vision
        Check "视觉偏好已恢复" ($pref2.model -eq $orig.name) "（实际 $($pref2.model)）"
    }
} else {
    Write-Host "  （本机 vision 模型不足 2 个：跳过偏好切换断言）" -ForegroundColor DarkGray
}

# 8b. 增删模型（净零）：加一个探针模型再删掉
$probeModel = "zzz-acceptance-probe"
$anyProvider = @($s.models_page.providers) | Select-Object -First 1
if ($anyProvider) {
    Act @{ action = "model_form"; open = $true } 300 | Out-Null
    Act @{ action = "model_form_set"; field = "provider"; value = $anyProvider.name } 200 | Out-Null
    Act @{ action = "model_form_set"; field = "name"; value = $probeModel } 200 | Out-Null
    Act @{ action = "model_form_set"; field = "cap"; value = "chat" } 200 | Out-Null
    Act @{ action = "model_add" } 1800 | Out-Null
    $null = Wait-Until {
        $c = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
        $null -ne (@($c.models.providers | ForEach-Object { $_.models }) | ForEach-Object { $_ } | Where-Object { $_.name -eq $probeModel })
    }
    $cfg8 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
    $added = @($cfg8.models.providers | ForEach-Object { $_.models } | Where-Object { $_.name -eq $probeModel })
    Check "添加模型已落库" ($added.Count -ge 1)
    Act @{ action = "model_remove"; provider = $anyProvider.name; model = $probeModel } 1800 | Out-Null
    $null = Wait-Until {
        $c = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
        -not (@($c.models.providers | ForEach-Object { $_.models } | Where-Object { $_.name -eq $probeModel }).Count -ge 1)
    }
    $cfg9 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
    $left = @($cfg9.models.providers | ForEach-Object { $_.models } | Where-Object { $_.name -eq $probeModel })
    Check "探针模型已删除（净零）" ($left.Count -eq 0) "（残留 $($left.Count)）"
} else {
    Write-Host "  （没有任何提供商：跳过模型增删断言）" -ForegroundColor DarkGray
}

Act @{ action = "prov_form"; open = $true } 300 | Out-Null
$s = Get-State
Check "新增提供商表单可展开" ($s.models_page.prov_form_open -eq $true)

# 8c. 增删提供商（净零）：后端校验要求「至少一个模型」，所以一次带一个模型
$probeProv = "zzz-provider-probe"
Act @{ action = "prov_form_set"; field = "name"; value = $probeProv } 200 | Out-Null
Act @{ action = "prov_form_set"; field = "endpoint"; value = "http://127.0.0.1:1/v1" } 200 | Out-Null
Act @{ action = "prov_form_set"; field = "model"; value = "probe-model" } 200 | Out-Null
Act @{ action = "prov_form_set"; field = "cap"; value = "chat" } 200 | Out-Null
Act @{ action = "prov_add" } 2200 | Out-Null
$null = Wait-Until {
    $c = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
    @($c.models.providers | Where-Object { $_.name -eq $probeProv }).Count -ge 1
}
$cfg10 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
$probe = @($cfg10.models.providers | Where-Object { $_.name -eq $probeProv })
Check "添加提供商已落库（含首个模型）" ($probe.Count -eq 1) "（实际 $($probe.Count)）"
if ($probe.Count -eq 1) {
    Check "提供商带上了首个模型" (@($probe[0].models).Count -ge 1)
}
Act @{ action = "prov_remove"; name = $probeProv } 2200 | Out-Null
$null = Wait-Until {
    $c = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
    -not (@($c.models.providers | Where-Object { $_.name -eq $probeProv }).Count -ge 1)
}
$cfg11 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
$probeLeft = @($cfg11.models.providers | Where-Object { $_.name -eq $probeProv })
Check "探针提供商已删除（净零）" ($probeLeft.Count -eq 0) "（残留 $($probeLeft.Count)）"

# ── 9. 数据存储 + 连接/关于 ─────────────────────────────────────────────
Write-Host "`n[数据存储 / 连接]" -ForegroundColor Cyan
$r = Act @{ action = "settings_tab"; tab = "storage" } 600
$s = StateOf $r
$dirField = @($s.settings_fields.fields) | Where-Object { $_.path -eq "storage.data_dir" }
Check "数据目录字段存在" ($null -ne $dirField)
Check "数据目录需重启标注" ($dirField.restart -eq $true)
Act @{ action = "settings_text"; path = "storage.cleanup_days"; value = "$($origCleanup + 1)" } 200 | Out-Null
Act @{ action = "settings_save" } 1500 | Out-Null
$null = Wait-Until { ((Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config.storage.cleanup_days) -eq ($origCleanup + 1) }
$cfg6 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
Check "存储字段保存生效" ($cfg6.storage.cleanup_days -eq ($origCleanup + 1))
Act @{ action = "settings_text"; path = "storage.cleanup_days"; value = "$origCleanup" } 200 | Out-Null
Act @{ action = "settings_save" } 1500 | Out-Null
$null = Wait-Until { ((Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config.storage.cleanup_days) -eq $origCleanup }
$cfg7 = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
Check "存储字段已恢复" ($cfg7.storage.cleanup_days -eq $origCleanup) "（实际 $($cfg7.storage.cleanup_days) / 期望 $origCleanup）"

$r = Act @{ action = "settings_tab"; tab = "connection" } 200
$r = Act @{ action = "health_refresh" } 900
$s = StateOf $r
Check "连接页探活成功" ($s.settings_extras.health.status -eq "ok") "（$($s.settings_extras.health.msg)）"
Check "连接页拿到服务版本" (-not [string]::IsNullOrEmpty($s.settings_extras.health.version))
Check "连接页拿到数据目录" (-not [string]::IsNullOrEmpty($s.settings_extras.health.data_dir))
Check "版本与服务端一致" ($s.settings_extras.health.version -eq $health.version)
Check "配置数据目录与服务实际一致（不误报）" ($s.settings_extras.health.dir_matches_config -eq $true)
Act @{ action = "settings_tab"; tab = "about" } 300 | Out-Null
$s = Get-State
Check "关于页复用同一份探活结果" ($s.settings_extras.health.version -eq $health.version)

# ── 9. 收尾 ────────────────────────────────────────────────────────────
Write-Host "`n[收尾]" -ForegroundColor Cyan
Act @{ action = "settings_tab"; tab = "models" } 300 | Out-Null
$cfgEnd = (Invoke-RestMethod -Uri "$serverBase/api/v1/config" -TimeoutSec 10).config
Check "验收后配置回到基线（approval_mode）" ($cfgEnd.security.approval_mode -eq $origApproval)
Check "验收后配置回到基线（max_turns）" ($cfgEnd.agent.max_turns -eq $origMaxTurns)
Check "验收后配置回到基线（cleanup_days）" ($cfgEnd.storage.cleanup_days -eq $origCleanup)

if (-not $KeepAlive) {
    Write-Host "关闭实例…" -ForegroundColor Yellow
    # 语义化退出（与托盘菜单同路径），而不是 window_close（那只是藏到托盘）
    try { Act @{ action = "quit" } 200 | Out-Null } catch { }
    Start-Sleep -Milliseconds 1500
    if ($proc) {
        $alive = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
        if ($alive) {
            $alive | Stop-Process -Force
            Write-Host "（quit 未生效，已强制结束 PID $($proc.Id)）" -ForegroundColor DarkYellow
        }
    }
} else {
    Write-Host "实例保持运行（下次直接复用，零启动等待）" -ForegroundColor DarkGreen
}

Write-Host ""
if ($script:fail -eq 0) {
    Write-Host "全部通过：$($script:pass) 项断言" -ForegroundColor Green
    exit 0
} else {
    Write-Host "通过 $($script:pass) 项，失败 $($script:fail) 项：" -ForegroundColor Red
    $script:failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}



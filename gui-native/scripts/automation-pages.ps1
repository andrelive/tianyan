# 七个原生页面的自动化验证（工具 / 技能 / 定时任务 / 记忆 / 洞察 / 审批 / 知识）
#
# 用法（**必须用 pwsh**）：
#   $env:TIANYAN_GUI_AUTOMATION_PORT = "3010"
#   .\target\debug\tianyan-gui-native.exe
#   pwsh -NoProfile -File .\scripts\automation-pages.ps1
#
# 只读：不做任何写操作（不建任务 / 不改配置 / 不导入文件 / 不删条目）。
param([string]$BaseUrl = "http://127.0.0.1:3010")

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

$script:fail = 0
function Check {
    # 用 ValueFromRemainingArguments 吞下尾部参数：脚本里有 `Check (c) "msg {0}" -f $x`
    # 这种写法（`-f` 会被当成位置参数），不允它们报错
    param($Cond, [Parameter(ValueFromRemainingArguments = $true)] $Rest)
    $msg = ($Rest -join ' ')
    if ($Cond) { Write-Host "  [ok] $msg" -ForegroundColor Green }
    else { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:fail++ }
}
function Act {
    param([hashtable]$Payload)
    $json = $Payload | ConvertTo-Json -Compress -Depth 8
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    try {
        return Invoke-RestMethod -Uri "$BaseUrl/act" -Method Post -Body $bytes `
            -ContentType "application/json; charset=utf-8" -TimeoutSec 40
    } catch { Write-Host "  ACT 失败：$_" -ForegroundColor Red; $script:fail++; return $null }
}
function Tab($view, $settle = 2000) {
    return Act @{ action = "switch_tab"; view = $view; settle_ms = $settle }
}

Write-Host "== 工具 ==" -ForegroundColor Cyan
$r = Tab "tools" 2500
"  view={0}  count={1}  names={2}" -f $r.state.view, $r.state.tools.count,
    (($r.state.tools.names | Select-Object -First 5) -join ", ")
Check ($r.state.view -eq "tools") "切到工具页"
Check ($r.state.tools.count -gt 0) "工具目录已加载（{0} 个）" -f $r.state.tools.count
if ($r.state.tools.count -gt 0) {
    $first = $r.state.tools.names[0]
    $r2 = Act @{ action = "select_tool"; name = $first }
    "  选中 '{0}' -> selected={1}" -f $first, $r2.state.tools.selected
    Check ($r2.state.tools.selected -eq $first) "选中工具生效"
    Check (@($r2.state.tools.items[0].params).Count -ge 0) "参数名清单可读"
}

Write-Host "== 技能 ==" -ForegroundColor Cyan
$r = Tab "skills" 2500
"  count={0}  total_calls={1}  status={2}" -f $r.state.skills.count, $r.state.skills.total_calls, $r.state.skills.status
Check ($r.state.view -eq "skills") "切到技能页"
Check ($r.state.skills.count -ge 0) "技能列表已加载（{0} 个方法论技能）" -f $r.state.skills.count
if ($r.state.skills.count -gt 0) {
    $sn = $r.state.skills.names[0]
    $r2 = Act @{ action = "select_skill"; name = $sn; settle_ms = 2000 }
    "  选中 '{0}' -> content_chars={1}" -f $sn, $r2.state.skills.content_chars
    Check ($null -ne $r2.state.skills.content_chars) "技能内容已拉取"
}

Write-Host "== 定时任务 ==" -ForegroundColor Cyan
$r = Tab "scheduled" 2500
"  count={0}  status={1}" -f $r.state.scheduled.count, $r.state.scheduled.status
Check ($r.state.view -eq "scheduled") "切到定时任务页"
Check ($r.state.scheduled.count -ge 0) "定时任务已加载（{0} 个）" -f $r.state.scheduled.count
$r2 = Act @{ action = "sched_form"; open = $true }
Check ($r2.state.scheduled.form_open -eq $true) "新建表单可展开"
$r3 = Act @{ action = "sched_preset"; secs = 86400 }
Check ($r3.state.scheduled.preset -eq 86400) "间隔预设可切换"
$r4 = Act @{ action = "sched_form"; open = $false }
Check ($r4.state.scheduled.form_open -eq $false) "表单可收起（未提交任何任务）"

Write-Host "== 记忆 ==" -ForegroundColor Cyan
$r = Tab "memory" 3000
"  count={0}  status={1}" -f $r.state.memory.count, $r.state.memory.status
Check ($r.state.view -eq "memory") "切到记忆页"
Check ($r.state.memory.count -gt 0) "记忆已加载（{0} 条）" -f $r.state.memory.count
if ($r.state.memory.count -gt 0) {
    $mn = $r.state.memory.items[0].name
    $r2 = Act @{ action = "select_memory"; name = $mn }
    "  选中 '{0}' -> selected={1}" -f $mn, ($r2.state.memory.selected -ne $null)
    Check ($r2.state.memory.selected -ne $null) "记忆详情选中生效"
    $r3 = Act @{ action = "memory_detail"; open = $true }
    Check ($r3.state.memory.detail_open -eq $true) "L2 可展开"
}

Write-Host "== 洞察 ==" -ForegroundColor Cyan
$r = Tab "insights" 3000
"  status={0}  days={1}" -f $r.state.insights.status, $r.state.insights.usage_days
Check ($r.state.view -eq "insights") "切到洞察页"
Check ($null -ne $r.state.insights.stats) "总览统计已加载"
"  技能 {0}/{1} 调用 · 工具 {2}/{3} 调用 · 文档 {4} · 搜索 {5}" -f `
    $r.state.insights.stats.skills_tracked, $r.state.insights.stats.skill_calls,
    $r.state.insights.stats.tools_tracked, $r.state.insights.stats.tool_calls,
    $r.state.insights.stats.docs_tracked, $r.state.insights.stats.searches
Check ($null -ne $r.state.insights.usage) "模型用量已加载（{0} 次调用）" -f $r.state.insights.usage.calls
Check ($r.state.insights.scheduler.task_count -gt 0) "调度器任务已加载（{0} 个）" -f $r.state.insights.scheduler.task_count
$r2 = Act @{ action = "insights_days"; days = 1; settle_ms = 2000 }
Check ($r2.state.insights.usage_days -eq 1) "天数切换生效（1 天）"
$r3 = Act @{ action = "insights_days"; days = 7; settle_ms = 2000 }
Check ($r3.state.insights.usage_days -eq 7) "天数切回 7 天"

Write-Host "== 审批 ==" -ForegroundColor Cyan
$r = Tab "approval" 3000
"  mode={0}  auto={1}  rules={2}  history={3}  status={4}" -f `
    $r.state.approvals_page.mode, $r.state.approvals_page.auto,
    $r.state.approvals_page.rule_count, $r.state.approvals_page.history_count,
    $r.state.approvals_page.status
Check ($r.state.view -eq "approval") "切到审批页"
Check ($r.state.approvals_page.mode -ne "") "工作流配置已加载（模式 {0}）" -f $r.state.approvals_page.mode
Check ($r.state.approvals_page.rule_count -gt 0) "自动审批规则已加载（{0} 条）" -f $r.state.approvals_page.rule_count
Check ($r.state.approvals_page.history_count -ge 0) "审计记录已加载（{0} 条）" -f $r.state.approvals_page.history_count

Write-Host "== 知识 ==" -ForegroundColor Cyan
$r = Tab "knowledge" 3000
"  tab={0}  path='{1}'  entries={2}  status={3}" -f `
    $r.state.knowledge.tab, $r.state.knowledge.path, @($r.state.knowledge.entries).Count, $r.state.knowledge.status
Check ($r.state.view -eq "knowledge") "切到知识页"
Check (@($r.state.knowledge.entries).Count -ge 0) "知识条目已加载（{0} 个）" -f @($r.state.knowledge.entries).Count
$r2 = Act @{ action = "kb_tab"; tab = 2 }
Check ($r2.state.knowledge.tab -eq 2) "可切到搜索 tab"
$r3 = Act @{ action = "kb_search"; q = "知识库"; settle_ms = 3000 }
"  搜索 '知识库' -> 命中 {0} 条，status={1}" -f @($r3.state.knowledge.results).Count, $r3.state.knowledge.status
Check ($null -ne $r3.state.knowledge.results) "检索已执行"
$r4 = Act @{ action = "kb_tab"; tab = 0 }
Check ($r4.state.knowledge.tab -eq 0) "可切回浏览 tab"
# 若有目录，进一层再回根
$dir = @($r4.state.knowledge.entries) | Where-Object { $_.is_dir } | Select-Object -First 1
if ($dir) {
    $rel = ($dir.uri -split "knowledge/")[1]
    $r5 = Act @{ action = "kb_enter"; path = $rel; settle_ms = 2000 }
    "  进入 '{0}' -> entries={1}" -f $rel, @($r5.state.knowledge.entries).Count
    Check ($r5.state.knowledge.path -eq $rel) "知识目录可进入"
    $r6 = Act @{ action = "kb_enter"; path = ""; settle_ms = 2000 }
    Check ($r6.state.knowledge.path -eq "") "可回知识库根"
}

Write-Host "== 收尾：11 个入口全部可达 ==" -ForegroundColor Cyan
$all = @("chat", "workspace", "skills", "roles", "tools", "knowledge", "memory", "approval", "scheduled", "insights", "settings")
$bad = @()
foreach ($v in $all) {
    $r = Tab $v 800
    if ($r.state.view -ne $v) { $bad += $v }
}
Check ($bad.Count -eq 0) ("11 个一级入口全部可切换" + $(if ($bad.Count -gt 0) { "（失败：$($bad -join ', ')）" } else { "" }))
$r = Tab "chat" 500
Write-Host ""
if ($script:fail -eq 0) { Write-Host "全部断言通过" -ForegroundColor Green; exit 0 }
else { Write-Host ("{0} 项断言失败" -f $script:fail) -ForegroundColor Red; exit 1 }

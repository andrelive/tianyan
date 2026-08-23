# 捕获后端契约快照（脱敏后写入 gui-vite/src/test/fixtures/contract/）。
# 前置：后端已在本机 3000 端口运行（cargo run -p tianyan-server）。
# 用法：scripts/capture-contract-fixtures.ps1

$ErrorActionPreference = 'Stop'
$base = 'http://127.0.0.1:3000'
$outDir = Join-Path $PSScriptRoot '..gui-vitesrc	estixturescontract'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

function Capture([string]$name, [string]$url) {
  $resp = Invoke-WebRequest -Uri "$base$url" -UseBasicParsing -TimeoutSec 15
  $content = $resp.Content
  # 脱敏：api_key / 机器特定路径
  $content = $content -replace '"api_key":"[^"]*"', '"api_key":"REDACTED"'
  $content = $content -replace 'F:\\\\work\\\\[^"]*', 'F:\\\\work\\\\example'
  $content = $content -replace 'C:\\\\Users\\\\[^"]*', 'C:\\\\Users\\\\example'
  $content | Out-File (Join-Path $outDir $name) -Encoding utf8 -NoNewline
  Write-Host "captured $name"
}

Capture 'config.json'   '/api/v1/config'
Capture 'models.json'   '/api/v1/config/models'
Capture 'sessions.json' '/api/v1/sessions'

Write-Host '完成：请检查 git diff 确认契约变更符合预期，且 api_key 保持 REDACTED。'

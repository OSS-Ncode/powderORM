# docs-site 폴더를 감시하다가 파일이 저장되면(디바운스 후) sync-docs.sh로 맥미니에 동기화한다.
# 빌드/서버 재시작은 맥미니 쪽 docs-site-watch 데몬이 알아서 처리한다.
# 사용법: pwsh -File scripts/watch-sync-docs.ps1  (창을 열어두거나 백그라운드로 실행)

$repoRoot = Split-Path -Parent $PSScriptRoot
$docsDir = Join-Path $repoRoot "docs-site"
$syncScript = Join-Path $PSScriptRoot "sync-docs.sh"
$debounceSeconds = 2

if (-not (Test-Path $docsDir)) {
    Write-Error "docs-site 디렉터리를 찾을 수 없습니다: $docsDir"
    exit 1
}

$watcher = New-Object System.IO.FileSystemWatcher
$watcher.Path = $docsDir
$watcher.IncludeSubdirectories = $true
$watcher.NotifyFilter = [System.IO.NotifyFilters]::LastWrite -bor [System.IO.NotifyFilters]::FileName -bor [System.IO.NotifyFilters]::DirName
$watcher.EnableRaisingEvents = $true

# node_modules/.next/.source/out 등 빌드 산출물 변경은 무시한다.
$ignorePattern = '\\(node_modules|\.next|\.source|out)\\'

$script:pendingSync = $false
$script:lastChangeTime = Get-Date

$action = {
    $path = $Event.SourceEventArgs.FullPath
    if ($path -match $using:ignorePattern) { return }
    $script:lastChangeTime = Get-Date
    $script:pendingSync = $true
}

Register-ObjectEvent -InputObject $watcher -EventName Changed -Action $action | Out-Null
Register-ObjectEvent -InputObject $watcher -EventName Created -Action $action | Out-Null
Register-ObjectEvent -InputObject $watcher -EventName Deleted -Action $action | Out-Null
Register-ObjectEvent -InputObject $watcher -EventName Renamed -Action $action | Out-Null

Write-Host "[watch-sync-docs] 감시 시작: $docsDir"
Write-Host "[watch-sync-docs] 파일 저장 후 ${debounceSeconds}초 뒤 자동 동기화됩니다. (Ctrl+C로 종료)"

try {
    while ($true) {
        Start-Sleep -Seconds 1
        if ($script:pendingSync -and ((Get-Date) - $script:lastChangeTime).TotalSeconds -ge $debounceSeconds) {
            $script:pendingSync = $false
            Write-Host "`n[watch-sync-docs] 변경 감지 -> 동기화 시작 $(Get-Date -Format 'HH:mm:ss')"
            & bash $syncScript
            if ($LASTEXITCODE -ne 0) {
                Write-Host "[watch-sync-docs] 동기화 실패 (exit $LASTEXITCODE)" -ForegroundColor Red
            } else {
                Write-Host "[watch-sync-docs] 동기화 완료 $(Get-Date -Format 'HH:mm:ss')" -ForegroundColor Green
            }
        }
    }
} finally {
    Get-EventSubscriber | Unregister-Event
    $watcher.Dispose()
}

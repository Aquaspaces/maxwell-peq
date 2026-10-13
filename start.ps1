$ErrorActionPreference = 'Stop'
$peqRoot = $PSScriptRoot
$peqPython = (Get-Command python -ErrorAction Stop).Source
$peqLogs = Join-Path $peqRoot 'artifacts'
New-Item -ItemType Directory -Path $peqLogs -Force | Out-Null
$peqListener = Get-NetTCPConnection -LocalAddress 127.0.0.1 -LocalPort 8769 -State Listen -ErrorAction SilentlyContinue
if ($peqListener) {
    Write-Output 'Port 8769 is already in use; no new server started. Check http://127.0.0.1:8769'
    exit 0
}
$peqProcess = Start-Process -FilePath $peqPython -ArgumentList 'server.py' -WorkingDirectory $peqRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $peqLogs 'server.out.log') -RedirectStandardError (Join-Path $peqLogs 'server.err.log')
Set-Content -LiteralPath (Join-Path $peqLogs 'server.pid') -Value $peqProcess.Id
Write-Output "Editor: http://127.0.0.1:8769 | PID: $($peqProcess.Id) | Logs: $peqLogs"

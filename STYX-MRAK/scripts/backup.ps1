# Back up the SQLite database (Windows PowerShell). Stop the bot first unless
# the sqlite3 CLI is on PATH (then the online backup is used).
param(
  [string]$Db = $(if ($env:DB_PATH) { $env:DB_PATH } else { "data/styx.db" }),
  [string]$OutDir = $(if ($env:BACKUP_DIR) { $env:BACKUP_DIR } else { "backups" }),
  [int]$Keep = 30
)
$ErrorActionPreference = "Stop"
if (-not (Test-Path $Db)) { Write-Error "database not found: $Db"; exit 1 }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
$dest = Join-Path $OutDir "styx-$stamp.db"

if (Get-Command sqlite3 -ErrorAction SilentlyContinue) {
  & sqlite3 $Db ".backup '$dest'"
} else {
  Copy-Item $Db $dest
  if (Test-Path "$Db-wal") { Copy-Item "$Db-wal" "$dest-wal" }
  Write-Warning "sqlite3 not found: plain file copy made. Make sure the bot was stopped."
}
Write-Host "backup written: $dest"
Get-ChildItem $OutDir -Filter "styx-*.db" | Sort-Object LastWriteTime -Descending |
  Select-Object -Skip $Keep | Remove-Item -Force

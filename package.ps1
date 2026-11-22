# PC Monitor 2.0: the release folder and zip.
#   dist2\PC-Monitor\PC-Monitor.exe
#   dist2\PC-Monitor\_internal\   the sensor helper and LibreHardwareMonitor's DLLs
#   dist2\PC-Monitor.zip           with PC-Monitor\ inside: the shape 1.8's updater
#                                  accepts, so 1.8 users get 2.0 as an ordinary update
# Needs a release build of PC-Monitor.exe and sensors-helper\bin (sensors-helper\build.bat).
#   powershell -ExecutionPolicy Bypass -File package.ps1 [-Exe path\to\PC-Monitor.exe]
param(
    [string]$Exe = "target\release\PC-Monitor.exe",
    [string]$Out = "dist2",
    # the plan's limit for the installed folder
    [int]$MaxMB = 25
)
$ErrorActionPreference = "Stop"
Set-Location -LiteralPath $PSScriptRoot

if (-not (Test-Path $Exe)) { throw "no ${Exe}: cargo build --release -p pcm-app" }
if (-not (Test-Path "sensors-helper\bin\pcm-sensors.exe")) { throw "no sensors-helper\bin\pcm-sensors.exe: sensors-helper\build.bat" }

$app = Join-Path $Out "PC-Monitor"
if (Test-Path $Out) { Remove-Item -LiteralPath $Out -Recurse -Force }
New-Item -ItemType Directory (Join-Path $app "_internal") | Out-Null
Copy-Item $Exe (Join-Path $app "PC-Monitor.exe")
Copy-Item "sensors-helper\bin\*" (Join-Path $app "_internal")
Copy-Item "LICENSE" (Join-Path $app "LICENSE.txt")

$zip = Join-Path $Out "PC-Monitor.zip"
# the folder itself, so the zip holds PC-Monitor\...
Compress-Archive -Path $app -DestinationPath $zip -CompressionLevel Optimal

$folder = (Get-ChildItem -LiteralPath $app -Recurse -File | Measure-Object Length -Sum).Sum
$files = (Get-ChildItem -LiteralPath $app -Recurse -File).Count
"{0}: {1} files, {2:N1} MB; zip {3:N1} MB" -f $app, $files, ($folder / 1MB), ((Get-Item $zip).Length / 1MB)
if ($folder -gt $MaxMB * 1MB) { throw ("the program folder is {0:N1} MB, over {1} MB" -f ($folder / 1MB), $MaxMB) }

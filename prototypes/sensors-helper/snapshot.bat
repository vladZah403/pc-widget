@echo off
rem One reading of all sensors into snapshot-*.json next to this file.
rem Run it twice: normally and with right click - Run as administrator.
cd /d "%~dp0"
net session >nul 2>&1 && (set KIND=admin) || (set KIND=user)
bin\pcm-sensors.exe --once > snapshot-%KIND%.json
echo saved snapshot-%KIND%.json
pause

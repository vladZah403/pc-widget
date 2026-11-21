@echo off
rem PC Monitor 2.0: PC-Monitor.exe --diag with the sensor helper from
rem sensors-helper\bin, report saved as diag-v2-user.txt / diag-v2-admin.txt
rem in the project folder. Run it twice: normally and with right click -
rem Run as administrator. (The exe path is this PC's local build folder.)
cd /d "%~dp0..\.."
net session >nul 2>&1 && (set KIND=admin) || (set KIND=user)
"%LOCALAPPDATA%\pcm-target\release\PC-Monitor.exe" --diag --helper "%CD%\sensors-helper\bin\pcm-sensors.exe" --out "diag-v2-%KIND%.txt"
echo.
echo saved diag-v2-%KIND%.txt
pause

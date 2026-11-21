@echo off
rem PC Monitor 2.0 alpha: starts the widget built on this PC (local build
rem folder) with the sensor helper from sensors-helper\bin next to it.
cd /d "%~dp0..\.."
set EXE_DIR=%LOCALAPPDATA%\pcm-target\release
if not exist "%EXE_DIR%\PC-Monitor.exe" (
    echo no %EXE_DIR%\PC-Monitor.exe: build it with  cargo build --release -p pcm-app
    pause
    exit /b 1
)
if not exist "%EXE_DIR%\_internal" mkdir "%EXE_DIR%\_internal"
xcopy /y /q "sensors-helper\bin\*" "%EXE_DIR%\_internal\" >nul
start "" "%EXE_DIR%\PC-Monitor.exe"

@echo off
rem PC Monitor 2.0: sign PC-Monitor.zip with the release key before publishing.
rem The secret key lives only on Vlad's PC, never in the repository:
rem   %USERPROFILE%\.pcm-release-key\pc-monitor.key  (+ pc-monitor.pub)
rem Its public half is pcm_core::update::PUBLIC_KEY; the program installs an
rem update only with a valid signature. Needs rsign2 (cargo install rsign2).
rem   sign-release.bat path\to\PC-Monitor.zip 2.0.1
rem writes PC-Monitor.zip.minisig next to the zip: upload both to the release.
setlocal
set KEY=%USERPROFILE%\.pcm-release-key
set RSIGN=%USERPROFILE%\.cargo\bin\rsign.exe
if "%~1"=="" (
    echo usage: sign-release.bat path\to\PC-Monitor.zip [version]
    exit /b 2
)
if not exist "%KEY%\pc-monitor.key" (
    echo no release key in %KEY%
    exit /b 1
)
"%RSIGN%" sign -W -s "%KEY%\pc-monitor.key" -p "%KEY%\pc-monitor.pub" -x "%~1.minisig" -t "PC Monitor %~2" "%~1" || exit /b 1
"%RSIGN%" verify -p "%KEY%\pc-monitor.pub" -x "%~1.minisig" "%~1"

@echo off
rem Builds bin\pcm-sensors.exe with the C# compiler that comes with Windows
rem (.NET Framework 4.8), next to the LibreHardwareMonitor DLLs from ..\..\lhm.
setlocal
set CSC=%WINDIR%\Microsoft.NET\Framework64\v4.0.30319\csc.exe
set HERE=%~dp0
set OUT=%HERE%bin
if not exist "%HERE%..\..\lhm\LibreHardwareMonitorLib.dll" (
    echo no lhm\LibreHardwareMonitorLib.dll in the project folder
    exit /b 1
)
if not exist "%OUT%" mkdir "%OUT%"
copy /y "%HERE%..\..\lhm\*.dll" "%OUT%\" >nul
copy /y "%HERE%pcm-sensors.exe.config" "%OUT%\" >nul
"%CSC%" /nologo /target:exe /platform:anycpu /optimize+ /warn:4 ^
    /out:"%OUT%\pcm-sensors.exe" /reference:"%OUT%\LibreHardwareMonitorLib.dll" "%HERE%Program.cs"

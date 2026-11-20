@echo off
rem Builds dist\PC-Monitor.exe: one file with Python, the widget and the sensor DLLs from lhm\
cd /d "%~dp0"
if not exist lhm\LibreHardwareMonitorLib.dll (
  echo Put LibreHardwareMonitorLib.dll and its DLLs into the lhm folder first.
  exit /b 1
)
python -m PyInstaller --noconfirm --clean --onefile --windowed --name PC-Monitor ^
  --icon assets\app.ico --add-data "lhm;lhm" --add-data "assets\app.png;assets" widget.pyw

@echo off
rem Builds dist\PC-Monitor\ (PC-Monitor.exe + _internal) and dist\PC-Monitor.zip for a release.
rem A folder, not one big exe: it starts faster (nothing to unpack into %TEMP%) and in admin
rem mode the widget copies this folder to Program Files, where other programs can't change it.
cd /d "%~dp0"
if not exist lhm\LibreHardwareMonitorLib.dll (
  echo Put LibreHardwareMonitorLib.dll and its DLLs into the lhm folder first.
  exit /b 1
)
python -m PyInstaller --noconfirm --clean --windowed --name PC-Monitor ^
  --icon assets\app.ico --add-data "lhm;lhm" --add-data "assets\app.png;assets" ^
  --exclude-module numpy --exclude-module setuptools --exclude-module pkg_resources ^
  --exclude-module pywintypes --exclude-module pythoncom --exclude-module win32com ^
  --exclude-module charset_normalizer --exclude-module pyreadline3 widget.pyw
if errorlevel 1 exit /b 1
powershell -NoProfile -Command "Compress-Archive -Path dist\PC-Monitor -DestinationPath dist\PC-Monitor.zip -Force"

; Inno Setup script: PC-Monitor-Setup.exe from the built dist\PC-Monitor folder.
; Installs for the current user only (no admin rights, no UAC), into
; %LOCALAPPDATA%\Programs\PC-Monitor, so the in-app update can replace the folder.
; Build: ISCC /DAppVersion=1.5 installer.iss   (GitHub Actions does this for releases)

#ifndef AppVersion
  #define AppVersion "0.0"
#endif

[Setup]
AppId={{8F0D6F2A-6C1B-4E9B-9D5E-2B7C3A1F4E11}
AppName=PC Monitor
AppVersion={#AppVersion}
AppVerName=PC Monitor {#AppVersion}
AppPublisher=vladZah403
AppPublisherURL=https://github.com/vladZah403/pc-widget
AppSupportURL=https://github.com/vladZah403/pc-widget/issues
DefaultDirName={localappdata}\Programs\PC-Monitor
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=dist
OutputBaseFilename=PC-Monitor-Setup
SetupIconFile=assets\app.ico
UninstallDisplayIcon={app}\PC-Monitor.exe
UninstallDisplayName=PC Monitor
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; the widget's single-instance mutex: setup asks to close a running widget
AppMutex=PCMonitorWidget
CloseApplications=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "uk"; MessagesFile: "compiler:Languages\Ukrainian.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "dist\PC-Monitor\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\PC Monitor"; Filename: "{app}\PC-Monitor.exe"
Name: "{autodesktop}\PC Monitor"; Filename: "{app}\PC-Monitor.exe"; Tasks: desktopicon

[Registry]
; the widget's own "start with Windows" entry: removed on uninstall, never created here
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "PCMonitorWidget"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\PC-Monitor.exe"; Description: "{cm:LaunchProgram,PC Monitor}"; Flags: nowait postinstall skipifsilent

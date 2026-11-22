; Inno Setup script of PC Monitor 2.0: PC-Monitor-Setup.exe from dist2\PC-Monitor
; (package.ps1). Like 1.8's installer.iss: for the current user only (no admin
; rights, no UAC), into %LOCALAPPDATA%\Programs\PC-Monitor, so the in-app update
; can replace the folder.
;   ISCC /DAppVersion=2.0.0 [/DBeside18] installer2.iss   (GitHub Actions does this)
; /DBeside18 while 2.0 is tried next to 1.8 (elevation.rs BESIDE_1_8 = true):
; its own AppId, folder and names, so it neither upgrades nor removes 1.8.
; Without it 2.0 takes 1.8's AppId: running this setup over 1.8 upgrades it.
; Saved as UTF-8 with a BOM: without it ISCC reads the Cyrillic messages as ANSI.

#ifndef AppVersion
  #define AppVersion "0.0"
#endif

#ifdef Beside18
  #define TheAppId "{{8F0D6F2A-6C1B-4E9B-9D5E-2B7C3A1F4E12}"
  #define Folder "PC-Monitor 2"
  #define Names "PCMonitor2Widget"
  #define Title "PC Monitor 2"
#else
  #define TheAppId "{{8F0D6F2A-6C1B-4E9B-9D5E-2B7C3A1F4E11}"
  #define Folder "PC-Monitor"
  #define Names "PCMonitorWidget"
  #define Title "PC Monitor"
#endif

[Setup]
AppId={#TheAppId}
AppName={#Title}
AppVersion={#AppVersion}
AppVerName={#Title} {#AppVersion}
AppPublisher=vladZah403
AppPublisherURL=https://github.com/vladZah403/pc-widget
AppSupportURL=https://github.com/vladZah403/pc-widget/issues
DefaultDirName={localappdata}\Programs\{#Folder}
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=dist2
OutputBaseFilename=PC-Monitor-Setup
SetupIconFile=assets\app.ico
UninstallDisplayIcon={app}\PC-Monitor.exe
UninstallDisplayName={#Title}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; the program's single-instance mutex: setup asks to close a running copy
AppMutex={#Names}
CloseApplications=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "uk"; MessagesFile: "compiler:Languages\Ukrainian.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[CustomMessages]
en.StartWithWindows=Start with Windows
ru.StartWithWindows=Запускать вместе с Windows
uk.StartWithWindows=Запускати разом з Windows
de.StartWithWindows=Mit Windows starten

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "{cm:StartWithWindows}"; Flags: unchecked

[InstallDelete]
; 1.8 kept ~1000 Python files in _internal: none of them is 2.0's
Type: filesandordirs; Name: "{app}\_internal"

[Files]
Source: "dist2\PC-Monitor\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#Title}"; Filename: "{app}\PC-Monitor.exe"
Name: "{autodesktop}\{#Title}"; Filename: "{app}\PC-Monitor.exe"; Tasks: desktopicon

[Registry]
; the program's own "start with Windows" entry (quietly, without the window);
; removed on uninstall either way
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#Names}"; ValueData: """{app}\PC-Monitor.exe"" --autostart"; Tasks: autostart
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "{#Names}"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\PC-Monitor.exe"; Description: "{cm:LaunchProgram,{#Title}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; what admin mode left: the elevated autostart task and the protected copy in
; Program Files (Windows asks for admin rights only when there is something)
Filename: "{app}\PC-Monitor.exe"; Parameters: "--cleanup"; Flags: runhidden waituntilterminated; RunOnceId: "cleanup"

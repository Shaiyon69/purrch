; Builds dist\PurrchSetup.exe: cargo build --release, then iscc installer.iss
#define Version "1.0.2"

[Setup]
AppId={{6B1F7C2E-4D3A-4C1B-9E2F-50A7C1D8B3E4}
AppName=Purrch
AppVersion={#Version}
AppPublisher=Shaiyon69
AppPublisherURL=https://github.com/Shaiyon69/purrch
; Per-user install, like Nusa: no admin prompt.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\Purrch
DisableProgramGroupPage=yes
DisableDirPage=yes
OutputDir=dist
OutputBaseFilename=PurrchSetup
SetupIconFile=assets\purrch.ico
UninstallDisplayIcon={app}\purrch.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; Closes a running Purrch before upgrading or uninstalling.
CloseApplications=force

[Files]
Source: "target\release\purrch.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Purrch"; Filename: "{app}\purrch.exe"

[Tasks]
Name: "autostart"; Description: "Start Purrch with Windows"

[Registry]
; Same value the tray's "Start with Windows" toggle writes.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Purrch"; ValueData: """{app}\purrch.exe"""; Tasks: autostart
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "Purrch"; Flags: uninsdeletevalue dontcreatekey

[Run]
Filename: "{app}\purrch.exe"; Description: "Launch Purrch"; Flags: nowait postinstall

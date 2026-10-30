; Termurion installer (Inno Setup 6). Per-user: no admin prompt, installs to
; %LOCALAPPDATA%\Programs\Termurion, adds Start Menu and optional Desktop shortcuts and an
; Apps > Installed apps entry.
;
; Build after `cargo build --release`, from the repo root:
;   ISCC.exe /DAppVersion=0.0.1-beta7 installer\termurion.iss
; The setup file lands in target\installer\.

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=x.y.z installer\termurion.iss
#endif

#define AppName "Termurion"
#define AppExe "termurion.exe"
#define Rel "..\target\release"

[Setup]
; Never change AppId: it ties upgrades and the uninstaller to earlier installs.
AppId={{329D8970-182C-40BF-A034-89D8AAFB8063}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Termurion
AppPublisherURL=https://github.com/datbird/termurion
AppSupportURL=https://github.com/datbird/termurion/issues
AppUpdatesURL=https://github.com/datbird/termurion/releases
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
LicenseFile=..\LICENSE
SetupIconFile=..\docs\assets\termurion\termurion.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir=..\target\installer
OutputBaseFilename=Termurion-{#AppVersion}-setup
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Close a running Termurion (and its session host) before files are replaced.
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#Rel}\termurion.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Rel}\termurion-host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Rel}\termurion-git.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Rel}\termurion-utils.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Rel}\cc-status.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Rel}\conpty.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Rel}\OpenConsole.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
Source: "..\resources\LICENSE-iTerm2"; DestDir: "{app}"; DestName: "LICENSE-iTerm2.txt"
Source: "..\resources\LICENSE"; DestDir: "{app}"; DestName: "NOTICE-iTerm2.txt"

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{%USERPROFILE}"; AppUserModelID: "Termurion.Terminal"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{%USERPROFILE}"; AppUserModelID: "Termurion.Terminal"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Termurion can stay running in the notification area with no window; stop it so its files can go.
; Only the copies that run from this install folder are stopped, never another install or a build.
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -NonInteractive -Command ""Get-Process termurion,termurion-host -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -and $_.Path.StartsWith('{app}\', 'OrdinalIgnoreCase') } | Stop-Process -Force"""; Flags: runhidden; RunOnceId: "StopTermurion"

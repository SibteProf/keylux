; Compile with /DAppVersion=... /DSourceDir=... /DOutputDir=...
#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\target\installer"
#endif

[Setup]
AppId={{B6C30245-3979-41B1-971D-4B87DA24C487}
AppName=Keylux
AppVersion={#AppVersion}
AppPublisher=Sibte Hussain
AppPublisherURL=https://github.com/SibteProf/keylux
AppSupportURL=https://github.com/SibteProf/keylux/issues
DefaultDirName={commonpf32}\Keylux
DisableDirPage=no
DefaultGroupName=Keylux
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=Keylux-{#AppVersion}-Setup-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\keylux.exe
CloseApplications=yes
RestartApplications=no
LicenseFile=..\..\LICENSE-MIT

[Tasks]
Name: desktopicon; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\keylux.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\effects\*.rhai"; DestDir: "{app}\effects"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\docs\*.md"; DestDir: "{app}\docs"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "installed.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Keylux"; Filename: "{app}\keylux.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\Keylux"; Filename: "{app}\keylux.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\keylux.exe"; Description: "Launch Keylux"; Flags: nowait postinstall skipifsilent

[Code]
// Move an existing startup registration to the installed executable. Do not
// enable startup for users who have never enabled it in Keylux.
procedure CurStepChanged(CurStep: TSetupStep);
var Command: String;
begin
  if CurStep = ssPostInstall then
    if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Keylux', Command) then
      if Command <> '' then
        RegWriteStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Keylux', '"' + ExpandConstant('{app}\keylux.exe') + '"');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var Command: String;
begin
  if CurUninstallStep = usUninstall then
    if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Keylux', Command) then
      if CompareText(Command, '"' + ExpandConstant('{app}\keylux.exe') + '"') = 0 then
        RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Keylux');
end;

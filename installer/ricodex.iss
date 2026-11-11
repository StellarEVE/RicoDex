#define AppName "RicoDex"
#ifndef AppVersion
  #define AppVersion "0.1.0-alpha.9"
#endif
#define AppPublisher "StellarEVE"
#define AppExeName "pmv-app.exe"

[Setup]
AppId={{4A37DF7A-9A21-4BE3-A8D8-703B1D5D27C1}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\RicoDex
DefaultGroupName=RicoDex
OutputDir=..\dist
OutputBaseFilename=RicoDex-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
WizardStyle=modern
Uninstallable=yes

[Files]
Source: "..\dist\app\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\RicoDex"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\RicoDex"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional icons:"

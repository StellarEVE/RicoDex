#define AppName "HyperDex Studio"
#ifndef AppVersion
  #define AppVersion "0.1.0-alpha.9"
#endif
#define AppPublisher "StellarEVE"
#define AppExeName "hyperdex-app.exe"

[Setup]
AppId={{4A37DF7A-9A21-4BE3-A8D8-703B1D5D27C1}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\HyperDex Studio
DefaultGroupName=HyperDex Studio
OutputDir=..\dist
OutputBaseFilename=HyperDexStudio-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
WizardStyle=modern
Uninstallable=yes

[Files]
Source: "..\dist\portable\HyperDexStudio\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\dist\portable\vc_redist.x64.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Icons]
Name: "{autoprograms}\HyperDex Studio"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\HyperDex Studio"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{tmp}\vc_redist.x64.exe"; Parameters: "/install /quiet /norestart"; StatusMsg: "Installing Microsoft Visual C++ runtime"; Flags: runhidden waituntilterminated

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional icons:"

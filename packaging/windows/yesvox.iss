; YesVox v1.0 unsigned Windows setup recipe (#20).
; Build with Inno Setup after staging release binaries, runtime files, and the
; approved model bundle. This source file is intentionally committed before the
; final weight artifacts; the model hashes remain in licenses/model-weights.tsv.

#define AppVersion GetEnv("YESVOX_PACKAGE_VERSION")
#if AppVersion == ""
#define AppVersion "0.1.0-dev"
#endif

#define BinDir GetEnv("YESVOX_BIN_DIR")
#if BinDir == ""
#define BinDir "target\release"
#endif

#define RuntimeBundleDir GetEnv("YESVOX_RUNTIME_BUNDLE_DIR")
#if RuntimeBundleDir == ""
#define RuntimeBundleDir "target\runtime-bundle"
#endif

#define ModelBundleDir GetEnv("YESVOX_MODEL_BUNDLE_DIR")
#if ModelBundleDir == ""
#define ModelBundleDir "target\model-bundle"
#endif

[Setup]
AppId={{4734FC0B-3D2C-4C96-8E0F-6D4F72F6F61E}
AppName=YesVox
AppVersion={#AppVersion}
AppPublisher=Yes Suite
DefaultDirName={autopf}\YesVox
DefaultGroupName=YesVox
DisableProgramGroupPage=yes
OutputDir=target\package
OutputBaseFilename=yesvox-v1-unsigned
Compression=lzma2/ultra64
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
SetupLogging=yes
WizardStyle=modern
LicenseFile=packaging\windows\THIRD-PARTY-NOTICES.txt
UninstallDisplayIcon={app}\yesvox-gui.exe

[Files]
Source: "{#BinDir}\yesvox-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\yesvox.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RuntimeBundleDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#ModelBundleDir}\*"; DestDir: "{app}\models"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "packaging\windows\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; DestName: "THIRD-PARTY-NOTICES.txt"; Flags: ignoreversion
Source: "licenses\model-weights.tsv"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "licenses\DeepFilterNet3\WEIGHTS_LICENSE.md"; DestDir: "{app}\licenses\DeepFilterNet3"; Flags: ignoreversion
Source: "licenses\MossFormer2_SE_48K\WEIGHTS_LICENSE.md"; DestDir: "{app}\licenses\MossFormer2_SE_48K"; Flags: ignoreversion

[Icons]
Name: "{group}\YesVox"; Filename: "{app}\yesvox-gui.exe"
Name: "{autodesktop}\YesVox"; Filename: "{app}\yesvox-gui.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional icons:"; Flags: unchecked

[Run]
Filename: "{app}\yesvox-gui.exe"; Description: "Launch YesVox"; Flags: nowait postinstall skipifsilent

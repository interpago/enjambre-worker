#define MyAppName "Enjambre Worker Node"
#define MyAppVersion "0.5.1"
#define MyAppPublisher "Enjambre"
#define MyAppExeName "worker-node.exe"

[Setup]
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={localappdata}\Enjambre
DefaultGroupName={#MyAppName}
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=Enjambre-Worker-Node-Setup-{#MyAppVersion}
Compression=lzma/ultra64
SolidCompression=yes
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName}
AppCopyright=(c) 2025 Enjambre
AppModifyPath={uninstallexe}
AppComments=Nodo worker distribuido para inferencia LLM
CreateAppDir=yes

[Languages]
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[Files]
Source: "..\target\release\worker-node.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Enjambre Worker Node"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; Comment: "Inicia el nodo worker"
Name: "{group}\Desinstalar Enjambre Worker Node"; Filename: "{uninstallexe}"; WorkingDir: "{app}"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; \
  ValueType: string; ValueName: "EnjambreWorkerNode"; \
  ValueData: "{app}\{#MyAppExeName}"; Flags: uninsdeletevalue

[Tasks]
Name: "desktopicon"; Description: "Crear acceso directo en el Escritorio"; GroupDescription: "Accesos directos:"; Flags: unchecked
Name: "startmenu"; Description: "Crear acceso directo en el Menú Inicio"; GroupDescription: "Accesos directos:"; Flags: checkedonce

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Iniciar Enjambre Worker Node ahora"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}\logs"

[Code]
function InitializeSetup: Boolean;
begin
  Result := True;
end;
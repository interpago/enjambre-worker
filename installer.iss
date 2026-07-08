; Enjambre Worker Installer — solo para PCs con GPU NVIDIA
; Inno Setup Script

#define MyAppName "Enjambre Worker"
#define MyAppVersion "0.5.1"
#define MyAppPublisher "Enjambre"

[Setup]
AppId={{A1B2C3D4-E5F6-7890-ABCD-EF1234567890}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={localappdata}\Enjambre
DefaultGroupName={#MyAppName}
UninstallDisplayIcon={app}\worker-node.exe
Compression=lzma2/max
SolidCompression=yes
OutputDir=.
OutputBaseFilename=EnjambreWorker-Setup-0.5.1
PrivilegesRequired=lowest
DisableProgramGroupPage=yes
AllowNoIcons=yes
DisableDirPage=auto

[Languages]
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[Files]
Source: "C:\Users\Pc\worker-node\target\release\worker-node.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "C:\Users\Pc\worker-node\install-cuda-only.ps1"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Icons]
Name: "{group}\Enjambre Worker"; Filename: "{app}\worker-node.exe"; WorkingDir: "{app}"
Name: "{group}\Desinstalar Enjambre Worker"; Filename: "{uninstallexe}"

[Run]
Filename: "powershell.exe"; Parameters: "-ExecutionPolicy Bypass -File ""{tmp}\install-cuda-only.ps1"""; Flags: runhidden
Filename: "{app}\worker-node.exe"; Description: "Iniciar Enjambre Worker ahora"; Flags: postinstall nowait skipifsilent unchecked

[Code]
function InitializeSetup: Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  if not (Exec('cmd.exe', '/c nvidia-smi >nul 2>&1', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) and (ResultCode = 0)) then
  begin
    MsgBox('Este instalador requiere una GPU NVIDIA compatible.' + #13#10 + #13#10 +
           'No se detectó ninguna GPU NVIDIA en este equipo.' + #13#10 +
           'Si tienes una GPU NVIDIA, asegúrate de tener los drivers instalados.',
           mbError, MB_OK);
    Result := False;
  end;
end;

[UninstallRun]
Filename: "{cmd}"; Parameters: "/c taskkill /f /im worker-node.exe /im llama-server.exe /im ggml-rpc-server.exe 2>nul"; Flags: runhidden

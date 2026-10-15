; Script Inno Setup pour Switchboard.
; Compilation : ISCC.exe /DMyAppVersion=<version> installer.iss
; (depuis le dossier windows/, avec switchboard-windows/ deja stage a cote)

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0-dev"
#endif

#define MyAppName "Switchboard"
#define MyAppPublisher "SkollN"
#define MyAppURL "https://github.com/masmarino/switchboard"
#define MyAppExeName "Switchboard.exe"

[Setup]
AppId={{B1F1A6F0-6C2B-4C0E-9C0A-1F4E9B6C2D31}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; Le Visual C++ Redistributable (requis par Microsoft.UI.Xaml.dll) a besoin de droits
; admin pour s'installer. Avec PrivilegesRequired=lowest, son installation silencieuse
; echouait sans le signaler (pas de prompt UAC = pas d'install reelle). On accepte donc
; un seul prompt UAC au debut de l'installation plutot qu'une app qui ne se lance jamais.
PrivilegesRequired=admin
OutputBaseFilename=Switchboard-{#MyAppVersion}-setup
OutputDir=.
SetupIconFile=..\icons\icon.ico
LicenseFile=..\LICENSE
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64
WizardStyle=modern
UninstallDisplayIcon={app}\{#MyAppExeName}
; switchboard-cli.exe est installe a cote de l'app ; la tache "addtopath" rend {app} visible
; aux terminaux ouverts apres l'installation.
ChangesEnvironment=yes

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "..\switchboard-windows\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "vc_redist.x64.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Tasks]
Name: "desktopicon"; Description: "Créer un raccourci sur le Bureau"; GroupDescription: "Raccourcis supplémentaires:"
Name: "addtopath"; Description: "Ajouter switchboard-cli au PATH"; GroupDescription: "Ligne de commande:"

[Registry]
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: PathNeedsApp(ExpandConstant('{app}'))

; {group}/{userdesktop} se resolvent deja vers le profil de l'utilisateur reel
; meme sous installeur elevé (Inno Setup suit qui a lance l'installeur) — pas
; besoin de flag particulier ici.
[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{userdesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
; Microsoft.UI.Xaml.dll (WinUI3) plante au lancement sans le Visual C++
; Redistributable x64 — ni --self-contained (.NET) ni WindowsAppSDKSelfContained
; ne le couvrent, c'est une dependance native a part. On l'installe en
; silencieux, sauf s'il est deja present (cf. VCRedistNeedsInstall ci-dessous).
; Pas de runasoriginaluser ici : ce sous-installeur a justement besoin des
; droits admin herites de l'installeur principal pour fonctionner.
Filename: "{tmp}\vc_redist.x64.exe"; Parameters: "/install /quiet /norestart"; StatusMsg: "Installation du Visual C++ Redistributable…"; Check: VCRedistNeedsInstall
; runasoriginaluser : lance l'app en tant qu'utilisateur reel plutot que le
; compte elevé utilisé pendant l'installation (sinon Switchboard se lancerait
; avec des droits admin inutiles et potentiellement un profil different).
Filename: "{app}\{#MyAppExeName}"; Description: "Lancer {#MyAppName}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[Code]
const
  EnvironmentKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

function PathNeedsApp(Dir: string): Boolean;
var
  Path: string;
begin
  Result := True;
  if RegQueryStringValue(HKLM, EnvironmentKey, 'Path', Path) then
    Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path, Dir: string;
  P: Integer;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  if not RegQueryStringValue(HKLM, EnvironmentKey, 'Path', Path) then
    exit;
  Dir := ExpandConstant('{app}');
  P := Pos(';' + Uppercase(Dir), Uppercase(Path));
  if P > 0 then
  begin
    Delete(Path, P, Length(Dir) + 1);
    RegWriteExpandStringValue(HKLM, EnvironmentKey, 'Path', Path);
  end;
end;

function VCRedistNeedsInstall: Boolean;
var
  Installed: Cardinal;
begin
  Result := True;
  if RegQueryDWordValue(HKLM, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\X64', 'Installed', Installed) then
  begin
    if Installed = 1 then
      Result := False;
  end;
end;

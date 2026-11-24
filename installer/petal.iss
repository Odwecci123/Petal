; Установщик Petal. Собирается программой Inno Setup (iscc), см. .github/workflows/release.yml.
; Версию передаёт сборка: iscc /DAppVersion=1.0.0 installer\petal.iss

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif

[Setup]
AppId={{6F1C0E52-7B7A-4C58-9E3B-5A1D2B8E7A10}
AppName=Petal
AppVersion={#AppVersion}
AppPublisher=Petal
DefaultDirName={autopf}\Petal
DefaultGroupName=Petal
UninstallDisplayIcon={app}\petal.exe
SetupIconFile=..\assets\petal.ico
OutputDir=..\dist
OutputBaseFilename=Petal-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Без прав администратора: ставится для текущего пользователя, окно выбора можно показать.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
CloseApplications=yes

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\petal.exe"; DestDir: "{app}"; Flags: ignoreversion
; Установщик встроенного браузера от Microsoft: нужен, только если его ещё нет в системе.
Source: "..\dist\MicrosoftEdgeWebview2Setup.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall; Check: NeedsWebView2

[Icons]
Name: "{group}\Petal"; Filename: "{app}\petal.exe"
Name: "{autodesktop}\Petal"; Filename: "{app}\petal.exe"; Tasks: desktopicon

[Run]
Filename: "{tmp}\MicrosoftEdgeWebview2Setup.exe"; Parameters: "/silent /install"; StatusMsg: "Устанавливаю компонент WebView2 (встроенный браузер)..."; Flags: waituntilterminated; Check: NeedsWebView2
Filename: "{app}\petal.exe"; Description: "{cm:LaunchProgram,Petal}"; Flags: nowait postinstall skipifsilent
<<<<<<< HEAD
Filename: "{app}\petal.exe"; Flags: nowait; Check: WizardSilent
=======
>>>>>>> 8545295380960461b55fec532c475d151d4a0f94

[Code]
const
  WebView2Key = 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';

function HasWebView2(Root: Integer; const Key: String): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(Root, Key, 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0');
end;

{ WebView2 есть в Windows 11 и в большинстве обновлённых Windows 10; ставим только если его нет. }
function NeedsWebView2: Boolean;
begin
  Result := not (HasWebView2(HKLM32, WebView2Key) or HasWebView2(HKLM64, WebView2Key) or HasWebView2(HKCU, WebView2Key));
end;

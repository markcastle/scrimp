<#
Installs Scrimp silently, checks everything the installer should have done,
installs again over the top, uninstalls, and checks nothing is left
behind. CI runs it on every change; it changes this user's menu and PATH
while it runs, and puts both back.

  .\scripts\test-installer.ps1 installers\Scrimp-windows-x64-setup.exe
#>
param([Parameter(Mandatory)] [string] $Setup)

$ErrorActionPreference = 'Stop'
$failures = @()
function Check([bool] $ok, [string] $what) {
    if ($ok) { Write-Host "ok: $what" } else { Write-Host "::error::$what"; $script:failures += $what }
}

function UserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    try {
        [pscustomobject]@{
            Value = [string] $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            Kind  = if ($key.GetValueNames() -contains 'Path') { $key.GetValueKind('Path') } else { 'None' }
        }
    } finally { $key.Close() }
}

function Value([string] $key, [string] $name) {
    $item = Get-ItemProperty -LiteralPath "Registry::HKEY_CURRENT_USER\$key" -ErrorAction SilentlyContinue
    if ($null -eq $item) { return $null }
    if ($name -eq '') { return $item.'(default)' }
    return $item.$name
}

$setup = (Resolve-Path $Setup).Path
# A folder name with spaces, to prove the quoting holds.
$dir = Join-Path ([IO.Path]::GetTempPath()) 'Scrimp install test'
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
$before = UserPath
$compressClass = 'Software\Classes\CLSID\{F67547C1-2D5F-491C-A3C3-DF559DC9612D}'
$compressVerb = 'Software\Classes\*\shell\ScrimpCompress'
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\Scrimp'

# Install. /D= must come last and isn't quoted, as NSIS requires.
$p = Start-Process -FilePath $setup -ArgumentList '/S', "/D=$dir" -Wait -PassThru
Check ($p.ExitCode -eq 0) "the installer finished cleanly (exit code $($p.ExitCode))"

$version = ((& "$dir\scrimp.exe" --version) -split ' ')[1]
Check ($version -match '^\d+\.\d+\.\d+$') "scrimp.exe runs and says its version ($version)"
$dll = Join-Path $dir "shell\$version\scrimp_shell.dll"
foreach ($file in @("$dir\scrimp.exe", "$dir\scrimpw.exe", "$dir\uninstall.exe", $dll)) {
    Check (Test-Path -LiteralPath $file) "installed $file"
}
Check ((Value "$compressClass\InprocServer32" '') -eq $dll) "the menu points at $dll"
Check ((Value "$compressClass\InprocServer32" 'ThreadingModel') -eq 'Apartment') "the menu DLL is registered as Apartment"
Check ((Value $compressVerb 'ExplorerCommandHandler') -eq '{F67547C1-2D5F-491C-A3C3-DF559DC9612D}') "files get Scrimp Compress"
Check ($null -ne (Value 'Software\Classes\Directory\shell\ScrimpDecompress' 'ExplorerCommandHandler')) "folders get Scrimp Decompress"
Check ((Value $uninstallKey 'DisplayVersion') -eq $version) "Installed apps shows version $version"
Check ((Value $uninstallKey 'QuietUninstallString') -like '*uninstall.exe" /S') "Installed apps can uninstall quietly"

$afterInstall = UserPath
$expected = if ($before.Value -eq '') { $dir } elseif ($before.Value.EndsWith(';')) { "$($before.Value)$dir;" } else { "$($before.Value);$dir" }
Check ($afterInstall.Value -eq $expected) "PATH gained exactly one entry, at the end, and nothing else changed"
if ($afterInstall.Value -ne $expected) { Write-Host "expected: [$expected]"; Write-Host "got:      [$($afterInstall.Value)]" }
if ($before.Kind -ne 'None') {
    Check ($afterInstall.Kind -eq $before.Kind) "PATH kept its type ($($before.Kind))"
}

# Install again over the top, as an update does.
$p = Start-Process -FilePath $setup -ArgumentList '/S', "/D=$dir" -Wait -PassThru
Check ($p.ExitCode -eq 0) "installing again finished cleanly"
Check ((UserPath).Value -eq $expected) "installing again didn't add PATH twice"

# Uninstall. _?= runs the uninstaller in place, so this waits for it.
$p = Start-Process -FilePath "$dir\uninstall.exe" -ArgumentList '/S', "_?=$dir" -Wait -PassThru
Check ($p.ExitCode -eq 0) "the uninstaller finished cleanly"
$afterUninstall = UserPath
Check ($afterUninstall.Value -eq $before.Value) "PATH is exactly as it was before"
if ($afterUninstall.Value -ne $before.Value) { Write-Host "before: [$($before.Value)]"; Write-Host "after:  [$($afterUninstall.Value)]" }
Check ($afterUninstall.Kind -eq $before.Kind) "PATH has its original type"
Check ($null -eq (Value "$compressClass\InprocServer32" '')) "the menu's class is gone"
Check ($null -eq (Value $compressVerb 'ExplorerCommandHandler')) "the file menu entry is gone"
Check ($null -eq (Value $uninstallKey 'DisplayName')) "Scrimp is gone from Installed apps"
foreach ($file in @("$dir\scrimp.exe", "$dir\scrimpw.exe", "$dir\shell")) {
    Check (-not (Test-Path -LiteralPath $file)) "removed $file"
}
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue

if ($failures.Count -gt 0) {
    Write-Host "$($failures.Count) check(s) failed."
    exit 1
}
Write-Host 'Install, reinstall and uninstall all behaved.'

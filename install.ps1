# Installs or updates the Scrimp command line on Windows from the latest GitHub Release.
#
#   irm https://raw.githubusercontent.com/markcastle/scrimp/main/install.ps1 | iex
#
# It downloads scrimp.exe, checks it against the release's checksum file,
# puts it in your own Programs folder and adds that folder to your PATH. No
# administrator prompt. Running it again updates to the newest version.
#
# Adapted from Keevoro's install.ps1, with one difference: PATH is edited in
# the registry directly, so its other entries and its type (usually
# REG_EXPAND_SZ) are left exactly as they were. See docs/developing.md.

$ErrorActionPreference = 'Stop'
$repo = 'markcastle/scrimp'
$assetName = 'scrimp-windows-x64.exe'
$installDir = if ($env:SCRIMP_INSTALL_DIR) { $env:SCRIMP_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\Scrimp' }

$headers = @{ 'User-Agent' = 'scrimp-install' }
if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }

Write-Host 'Looking up the latest Scrimp release...'
try {
    $release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -Headers $headers
} catch {
    throw "Could not read the latest release of $repo. Check your internet connection and try again. ($($_.Exception.Message))"
}

$asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
$sums = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS' } | Select-Object -First 1
if (-not $asset -or -not $sums) { throw "Release $($release.tag_name) has no Windows build." }

# Files are fetched through the API address, which works whether or not the
# repository is private.
$download = $headers.Clone(); $download['Accept'] = 'application/octet-stream'
$tmp = Join-Path $env:TEMP "scrimp-$($release.tag_name)-$PID.exe"
$tmpSums = "$tmp.sums"
Write-Host "Downloading Scrimp $($release.tag_name) ($([math]::Round($asset.size / 1MB, 1)) MB)..."
Invoke-WebRequest $asset.url -OutFile $tmp -Headers $download -UseBasicParsing
Invoke-WebRequest $sums.url -OutFile $tmpSums -Headers $download -UseBasicParsing

# Never install a download that does not match the release's checksum.
$line = Get-Content $tmpSums | Where-Object { $_ -match "\s\*?$([regex]::Escape($assetName))$" } | Select-Object -First 1
$expected = if ($line) { ($line -split '\s+')[0].ToLower() } else { '' }
$actual = (Get-FileHash $tmp -Algorithm SHA256).Hash.ToLower()
Remove-Item $tmpSums -ErrorAction SilentlyContinue
if (-not $expected -or $expected -ne $actual) {
    Remove-Item $tmp -ErrorAction SilentlyContinue
    throw "The download did not match the release's checksum, so nothing was installed. Try again."
}
# The file came from the internet; without this Windows asks before running it.
Unblock-File $tmp

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$exe = Join-Path $installDir 'scrimp.exe'
try {
    Move-Item -Force $tmp $exe
} catch {
    Remove-Item $tmp -ErrorAction SilentlyContinue
    throw "Could not replace $exe. If Scrimp is running, let it finish and run this again. ($($_.Exception.Message))"
}

# Add the folder to this user's PATH, once. Never the system PATH.
#
# The value is read without expanding variables and written back with the
# type it already had, so entries such as %USERPROFILE%\bin stay as they
# were. [Environment]::SetEnvironmentVariable would expand them and write
# the whole value back as a plain string.
function Get-Normalised([string] $entry) {
    [Environment]::ExpandEnvironmentVariables($entry.Trim()).TrimEnd('\').ToLowerInvariant()
}

$environment = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
try {
    $current = [string] $environment.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $kind = if ($environment.GetValueNames() -contains 'Path') { $environment.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
    $wanted = Get-Normalised $installDir
    $present = @($current -split ';' | Where-Object { $_.Trim() -and (Get-Normalised $_) -eq $wanted }).Count -gt 0
    if (-not $present) {
        $separator = if ($current -eq '' -or $current.EndsWith(';')) { '' } else { ';' }
        $environment.SetValue('Path', "$current$separator$installDir", $kind)
        Write-Host "Added $installDir to your PATH. New terminal windows will find scrimp."

        # Tell running programs, so new terminals see the change without
        # signing out. The answer does not matter; some programs never reply.
        Add-Type -Namespace ScrimpInstall -Name Native -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
        $result = [System.UIntPtr]::Zero
        [ScrimpInstall.Native]::SendMessageTimeout([System.IntPtr] 0xffff, 0x001A, [System.UIntPtr]::Zero, 'Environment', 0x0002, 5000, [ref] $result) | Out-Null
    }
} finally {
    $environment.Close()
}
if (($env:Path -split ';') -notcontains $installDir) { $env:Path = "$env:Path;$installDir" }

Write-Host "Scrimp $($release.tag_name) is installed. Try it:  scrimp --help"

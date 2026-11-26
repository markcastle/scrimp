<#
Makes the small test drives scrimp-win's tests run against, or removes them.

Each is a VHDX file mounted as a drive letter:

  N:  NTFS, 4 KB clusters      everything works here
  L:  NTFS, 64 KB clusters     WOF works, NTFS compression doesn't
  X:  exFAT                    no compression at all
  R:  ReFS (as a Dev Drive is) no compression; WOF says "would not save space"
  W:  NTFS, WOF not attached   how a VHDX mounted after boot behaves

Phase 0 found that Windows doesn't attach its compression driver (WOF) to a
volume mounted after boot, so this attaches it to N:, L: and R: by hand and
leaves W: without it on purpose. See docs/windows-api.md.

For each drive it sets an environment variable, such as
SCRIMP_TEST_NTFS64K=L:\, for the tests to find. On GitHub it writes them to
GITHUB_ENV so later steps see them; on your own PC it prints them for you to
set. Tests skip a drive whose variable isn't set, except on CI.

Needs an administrator, because mounting a VHDX and attaching a filter
driver both do.

  .\scripts\test-volumes.ps1            make them
  .\scripts\test-volumes.ps1 -Remove    detach them and delete the files
#>
param(
    [switch] $Remove,
    [string] $Folder = $(if (Test-Path 'D:\') { 'D:\scrimp-test-volumes' } else { Join-Path $env:TEMP 'scrimp-test-volumes' })
)

$ErrorActionPreference = 'Stop'

$volumes = @(
    @{ Name = 'ntfs4k';  Variable = 'SCRIMP_TEST_NTFS4K';  Letter = 'N'; Size = 1024; Format = 'fs=ntfs unit=4096';  Wof = $true },
    @{ Name = 'ntfs64k'; Variable = 'SCRIMP_TEST_NTFS64K'; Letter = 'L'; Size = 512;  Format = 'fs=ntfs unit=65536'; Wof = $true },
    @{ Name = 'exfat';   Variable = 'SCRIMP_TEST_EXFAT';   Letter = 'X'; Size = 512;  Format = 'fs=exfat';           Wof = $false },
    @{ Name = 'refs';    Variable = 'SCRIMP_TEST_REFS';    Letter = 'R'; Size = 2048; Format = 'fs=refs';            Wof = $true },
    @{ Name = 'nowof';   Variable = 'SCRIMP_TEST_NO_WOF';  Letter = 'W'; Size = 512;  Format = 'fs=ntfs unit=4096';  Wof = $false }
)

function Invoke-Diskpart([string[]] $lines, [string] $name) {
    $script = Join-Path $Folder "$name.diskpart.txt"
    $lines | Set-Content -Encoding ascii $script
    $output = diskpart /s $script | Out-String
    Remove-Item $script -ErrorAction SilentlyContinue
    return $output
}

New-Item -ItemType Directory -Force -Path $Folder | Out-Null

if ($Remove) {
    foreach ($v in $volumes) {
        $file = Join-Path $Folder "$($v.Name).vhdx"
        if (Test-Path $file) {
            Invoke-Diskpart @("select vdisk file=`"$file`"", 'detach vdisk') $v.Name | Out-Null
            Remove-Item $file -ErrorAction SilentlyContinue
            Write-Host "Removed $($v.Letter): ($($v.Name))"
        }
    }
    exit 0
}

$made = @()
foreach ($v in $volumes) {
    $root = "$($v.Letter):\"
    if (Test-Path $root) {
        Write-Host "::warning::$root is already in use, so the $($v.Name) test drive wasn't made."
        continue
    }
    $file = Join-Path $Folder "$($v.Name).vhdx"
    Remove-Item $file -ErrorAction SilentlyContinue
    $output = Invoke-Diskpart @(
        "create vdisk file=`"$file`" maximum=$($v.Size) type=expandable",
        "select vdisk file=`"$file`"",
        'attach vdisk',
        'convert mbr',
        'create partition primary',
        "format $($v.Format) quick label=$($v.Name)",
        "assign letter=$($v.Letter)"
    ) $v.Name
    if (-not (Test-Path $root)) {
        Write-Host $output
        Write-Host "::warning::The $($v.Name) test drive couldn't be made."
        continue
    }
    if ($v.Wof) {
        fltmc attach wof "$($v.Letter):" | Out-Null
    }
    $made += $v
    Write-Host "$root is $($v.Name)$(if ($v.Wof) { ', WOF attached' } else { '' })"
}

foreach ($v in $made) {
    $line = "$($v.Variable)=$($v.Letter):\"
    if ($env:GITHUB_ENV) {
        Add-Content -Path $env:GITHUB_ENV -Value $line
    } else {
        Write-Host "  `$env:$($v.Variable) = '$($v.Letter):\'"
    }
}
if (-not $env:GITHUB_ENV -and $made.Count -gt 0) {
    Write-Host "Set the variables above in the window you run the tests from."
}
if ($made.Count -ne $volumes.Count) {
    exit 1
}

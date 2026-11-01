# Install a released jx binary on Windows (x64).
#
#   powershell -ExecutionPolicy Bypass -File .\install.ps1
#   .\install.ps1 -Version v1.0.0 -InstallDir "$env:LOCALAPPDATA\jx\bin"
#
# The archive and its SHA-256 are downloaded from the same GitHub release; the
# checksum is verified before the binary is moved into place.
# Environment overrides:
#   JX_VERSION             v1.0.0, 1.0.0 or "latest" (default)
#   JX_INSTALL_DIR         destination directory (default: %LOCALAPPDATA%\jx\bin)
#   JX_RELEASE_BASE_URL    download root for a mirror, including the tag
#   JX_REPO                owner/name (default: kael-tombo/Jx)

[CmdletBinding()]
param(
    [string]$Version = $(if ($env:JX_VERSION) { $env:JX_VERSION } else { "latest" }),
    [string]$InstallDir = $(if ($env:JX_INSTALL_DIR) { $env:JX_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "jx\bin" })
)

$ErrorActionPreference = "Stop"
$Repo = if ($env:JX_REPO) { $env:JX_REPO } else { "kael-tombo/Jx" }
$ReleaseBase = $env:JX_RELEASE_BASE_URL

# Windows PowerShell 5.1 may still default to an older protocol.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {
    # Newer PowerShell ignores this; TLS is already enforced.
}

$Arch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($Arch -ne [Runtime.InteropServices.Architecture]::X64) {
    throw "jx publishes Windows x64 binaries only; this machine is $Arch. Build from source with cargo."
}

switch -Regex ($Version) {
    '^latest$' { }
    '^v' { }
    '^[0-9]' { $Version = "v$Version" }
    default { throw "Invalid version: $Version" }
}
if ($Version -notmatch '^v[0-9A-Za-z._+-]+$') {
    throw "Invalid version: $Version"
}

if ($Version -eq "latest") {
    try {
        $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" `
            -Headers @{ "User-Agent" = "jx-installer" }
    } catch {
        throw "No published release was found for $Repo. Build from source or pass -Version."
    }
    if (-not $latest.tag_name) {
        throw "Could not determine the latest release tag for $Repo."
    }
    $Version = $latest.tag_name
}

if ($ReleaseBase) {
    $Base = $ReleaseBase.TrimEnd("/")
} else {
    $Base = "https://github.com/$Repo/releases/download/$Version"
}
$Target = "x86_64-pc-windows-msvc"
$Stage = "jx-$Version-$Target"
$Asset = "$Stage.zip"

$Tmp = Join-Path ([IO.Path]::GetTempPath()) ("jx-install-" + [Guid]::NewGuid().ToString("N"))
$TmpBin = Join-Path $InstallDir (".jx.tmp." + $PID)
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
$Extracted = Join-Path $Tmp "unpacked"

try {
    $Archive = Join-Path $Tmp $Asset
    $Sums = Join-Path $Tmp "SHA256SUMS"
    Write-Host "Downloading $Asset"
    Invoke-WebRequest -UseBasicParsing -Uri "$Base/$Asset" -OutFile $Archive
    Invoke-WebRequest -UseBasicParsing -Uri "$Base/SHA256SUMS" -OutFile $Sums

    $line = Get-Content -Path $Sums |
        Where-Object { $_ -match "\s+\*?$([Regex]::Escape($Asset))$" } |
        Select-Object -First 1
    if (-not $line) {
        throw "SHA256SUMS has no entry for $Asset"
    }
    $Expected = (($line -split '\s+')[0]).ToLowerInvariant()
    $Actual = (Get-FileHash -Algorithm SHA256 -Path $Archive).Hash.ToLowerInvariant()
    if ($Actual -ne $Expected) {
        throw "Checksum mismatch: expected $Expected, got $Actual"
    }

    Expand-Archive -Path $Archive -DestinationPath $Extracted -Force
    $Source = Join-Path (Join-Path $Extracted $Stage) "jx.exe"
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        throw "The archive does not contain $Stage\jx.exe"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -LiteralPath $Source -Destination $TmpBin -Force
    Move-Item -LiteralPath $TmpBin -Destination (Join-Path $InstallDir "jx.exe") -Force

    Write-Host "Installed $Version to $(Join-Path $InstallDir 'jx.exe')"
    & (Join-Path $InstallDir "jx.exe") --version

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathParts = @()
    if ($UserPath) { $PathParts = $UserPath.Split(";", [StringSplitOptions]::RemoveEmptyEntries) }
    if (-not ($PathParts | Where-Object { $_.TrimEnd("\") -ieq $InstallDir.TrimEnd("\") })) {
        Write-Host ""
        Write-Host "Add it to PATH for future terminals:"
        Write-Host "  [Environment]::SetEnvironmentVariable('Path', '$InstallDir;' + [Environment]::GetEnvironmentVariable('Path', 'User'), 'User')"
    }
} finally {
    Remove-Item -LiteralPath $Tmp -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $TmpBin -Force -ErrorAction SilentlyContinue
}

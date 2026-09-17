param(
  [string]$Stage,
  [string]$PreviousArchive,
  [string]$PreviousRevision,
  [string]$ExpectedSignerThumbprint
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $IsWindows) {
  throw "native-package-smoke.windows.ps1 requires Windows"
}
if ($env:GITHUB_ACTIONS -ne "true" -or $env:RUNNER_ENVIRONMENT -ne "github-hosted") {
  throw "Windows native package smoke is restricted to an ephemeral GitHub-hosted runner"
}
if ([string]::IsNullOrWhiteSpace($PreviousArchive) -ne [string]::IsNullOrWhiteSpace($PreviousRevision)) {
  throw "PreviousArchive and PreviousRevision must be provided together"
}
if ($PreviousRevision -and $PreviousRevision -notmatch "^[0-9a-fA-F]{40}$") {
  throw "PreviousRevision must be a full Git commit"
}
$ExpectedSigner = ("$ExpectedSignerThumbprint").Replace(" ", "").ToUpperInvariant()
if ($ExpectedSigner -and $ExpectedSigner -notmatch "^[0-9A-F]{40}$") {
  throw "ExpectedSignerThumbprint must be a SHA-1 certificate thumbprint"
}
if ($PreviousArchive -and -not $ExpectedSigner) {
  throw "a signed upgrade baseline requires ExpectedSignerThumbprint"
}

$Root = Split-Path -Parent $PSScriptRoot
function Contract-Field([string]$Name) {
  $Value = & node (Join-Path $Root "scripts/package-contract.mjs") windows --field $Name
  if ($LASTEXITCODE -ne 0) { throw "package contract lookup failed: $Name" }
  return $Value
}

$ProductName = Contract-Field "productName"
$Identifier = Contract-Field "identifier"
$Version = Contract-Field "version"
$WixUpgradeCode = Contract-Field "wixUpgradeCode"
$ReleaseBinary = Contract-Field "releaseBinary"
$Executable = Contract-Field "executable"
$Msi = Contract-Field "artifacts.msi"
$Nsis = Contract-Field "artifacts.nsis"
$Artifact = Contract-Field "migration.artifact"
$LegacyDirectory = Contract-Field "migration.legacyDirectory"
$MigrationClass = Contract-Field "migration.class"
$BeforeBase64 = Contract-Field "migration.beforeBase64"
$AfterBase64 = Contract-Field "migration.afterBase64"
if ($MigrationClass -ne "local-data") { throw "unsupported migration fixture class: $MigrationClass" }

function Get-MsiProperty([string]$Path, [string]$Name) {
  $Installer = $null
  $Database = $null
  $View = $null
  $Record = $null
  try {
    $Installer = New-Object -ComObject WindowsInstaller.Installer
    $Database = $Installer.GetType().InvokeMember("OpenDatabase", "InvokeMethod", $null, $Installer, @($Path, 0))
    $Query = "SELECT ``Value`` FROM ``Property`` WHERE ``Property``='$Name'"
    $View = $Database.GetType().InvokeMember("OpenView", "InvokeMethod", $null, $Database, @($Query))
    $View.GetType().InvokeMember("Execute", "InvokeMethod", $null, $View, $null) | Out-Null
    $Record = $View.GetType().InvokeMember("Fetch", "InvokeMethod", $null, $View, $null)
    if ($null -eq $Record) { throw "MSI property is missing: $Name" }
    return $Record.GetType().InvokeMember("StringData", "GetProperty", $null, $Record, @(1))
  } finally {
    foreach ($ComObject in @($Record, $View, $Database, $Installer)) {
      if ($null -ne $ComObject) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($ComObject) }
    }
  }
}

function Assert-MsiIdentity([string]$Path, [string]$ExpectedVersion) {
  $ActualName = Get-MsiProperty $Path "ProductName"
  $ActualVersion = Get-MsiProperty $Path "ProductVersion"
  $ActualUpgradeCode = Get-MsiProperty $Path "UpgradeCode"
  $ProductCode = Get-MsiProperty $Path "ProductCode"
  if ($ActualName -ne $ProductName) { throw "MSI ProductName differs: $Path" }
  if ($ActualVersion -ne $ExpectedVersion) { throw "MSI ProductVersion $ActualVersion differs from $ExpectedVersion" }
  if (([Guid]$ActualUpgradeCode) -ne ([Guid]$WixUpgradeCode)) { throw "MSI UpgradeCode differs from the pinned package contract" }
  return ([Guid]$ProductCode).ToString("B").ToUpperInvariant()
}

function Get-MsiProductState([string]$ProductCode) {
  $Installer = $null
  try {
    $Installer = New-Object -ComObject WindowsInstaller.Installer
    return [int]$Installer.GetType().InvokeMember("ProductState", "GetProperty", $null, $Installer, @($ProductCode))
  } finally {
    if ($null -ne $Installer) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($Installer) }
  }
}

function Assert-MsiProductState([string]$ProductCode, [int]$ExpectedState, [string]$Label) {
  $ActualState = Get-MsiProductState $ProductCode
  if ($ActualState -ne $ExpectedState) {
    throw "$Label product registration state $ActualState differs from $ExpectedState"
  }
}

function Assert-PeIdentity([string]$Path, [string]$ExpectedVersion, [string]$Label) {
  $VersionInfo = (Get-Item -LiteralPath $Path).VersionInfo
  $ActualName = ("$($VersionInfo.ProductName)").Trim()
  $ActualVersion = ("$($VersionInfo.ProductVersion)").Trim()
  if ($ActualName -ne $ProductName) { throw "$Label ProductName differs: $ActualName" }
  if ($ActualVersion -ne $ExpectedVersion) { throw "$Label ProductVersion $ActualVersion differs from $ExpectedVersion" }
}

function Assert-AuthenticodeTrust([string]$Path, [string]$ExpectedThumbprint, [string]$Label) {
  $Signature = Get-AuthenticodeSignature -LiteralPath $Path
  if ($Signature.Status -ne "Valid") {
    throw "$Label has Authenticode status $($Signature.Status)"
  }
  if ($null -eq $Signature.SignerCertificate) {
    throw "$Label has no Authenticode signer certificate"
  }
  $ActualThumbprint = $Signature.SignerCertificate.Thumbprint.ToUpperInvariant()
  if ($ExpectedThumbprint -and $ActualThumbprint -ne $ExpectedThumbprint) {
    throw "$Label signer does not match the configured release certificate"
  }
  if ($null -eq $Signature.TimeStamperCertificate) {
    throw "$Label does not carry an Authenticode timestamp"
  }
  return $ActualThumbprint
}

$CurrentProductCode = Assert-MsiIdentity $Msi $Version
Assert-PeIdentity $Nsis $Version "NSIS installer"
Assert-PeIdentity $ReleaseBinary $Version "release executable"
if ($ExpectedSigner) {
  [void](Assert-AuthenticodeTrust $ReleaseBinary $ExpectedSigner "release executable")
  [void](Assert-AuthenticodeTrust $Msi $ExpectedSigner "MSI installer")
  [void](Assert-AuthenticodeTrust $Nsis $ExpectedSigner "NSIS installer")
}

$PreviousMsi = $null
$PreviousNsis = $null
$PreviousVersion = $null
$PreviousProductCode = $null
if ($PreviousArchive) {
  if (-not (Test-Path -LiteralPath $PreviousArchive -PathType Leaf)) { throw "previous artifact archive is missing" }
  $PreviousRoot = Join-Path $env:RUNNER_TEMP ("zerocode-package-baseline-" + [Guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Path $PreviousRoot | Out-Null
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $Archive = [IO.Compression.ZipFile]::OpenRead($PreviousArchive)
  try {
    foreach ($Entry in $Archive.Entries) {
      if (
        [string]::IsNullOrWhiteSpace($Entry.Name) -or
        $Entry.FullName -ne $Entry.Name -or
        $Entry.FullName.Contains("\") -or
        $Entry.Name -in @(".", "..")
      ) {
        throw "unsafe path in previous package archive"
      }
    }
  } finally {
    $Archive.Dispose()
  }
  Expand-Archive -LiteralPath $PreviousArchive -DestinationPath $PreviousRoot
  $ReparsePoint = Get-ChildItem -LiteralPath $PreviousRoot -Recurse -Force | Where-Object {
    ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
  } | Select-Object -First 1
  if ($null -ne $ReparsePoint) { throw "previous package archive contains a reparse point" }
  $ExpectedPreviousRevision = $PreviousRevision.ToLowerInvariant()
  $ManifestJson = & node (Join-Path $Root "scripts/package-stage-manifest.mjs") verify windows $PreviousRoot --previous --revision $ExpectedPreviousRevision
  if ($LASTEXITCODE -ne 0) { throw "previous Windows package manifest verification failed" }
  $PreviousManifest = $ManifestJson | ConvertFrom-Json
  $PreviousVersion = $PreviousManifest.version
  $PreviousMsi = Join-Path $PreviousRoot $PreviousManifest.artifacts.msi.file
  $PreviousNsis = Join-Path $PreviousRoot $PreviousManifest.artifacts.nsis.file
  $PreviousProductCode = Assert-MsiIdentity $PreviousMsi $PreviousVersion
  Assert-PeIdentity $PreviousNsis $PreviousVersion "previous NSIS installer"
  if ($PreviousProductCode -eq $CurrentProductCode) {
    throw "current and previous MSI packages reuse one ProductCode"
  }
  [void](Assert-AuthenticodeTrust $PreviousMsi $ExpectedSigner "previous MSI installer")
  [void](Assert-AuthenticodeTrust $PreviousNsis $ExpectedSigner "previous NSIS installer")
}

$InstallStateUnknown = -1
$InstallStateDefault = 5
Assert-MsiProductState $CurrentProductCode $InstallStateUnknown "current MSI before smoke"
if ($PreviousProductCode) {
  Assert-MsiProductState $PreviousProductCode $InstallStateUnknown "previous MSI before smoke"
}

$UserProfile = [System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::UserProfile)
$LocalData = [System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::LocalApplicationData)
$LegacyRoot = Join-Path $UserProfile $LegacyDirectory
$TargetRoot = Join-Path $LocalData $Identifier
$Legacy = Join-Path $LegacyRoot $Artifact
$Target = Join-Path $TargetRoot $Artifact

foreach ($Path in @($LegacyRoot, $TargetRoot)) {
  if (Test-Path -LiteralPath $Path) {
    throw "native smoke refuses to touch pre-existing state: $Path"
  }
}
New-Item -ItemType Directory -Path $LegacyRoot | Out-Null
[IO.File]::WriteAllBytes($Legacy, [Convert]::FromBase64String($BeforeBase64))
$Expected = [Convert]::FromBase64String($BeforeBase64)
$ExpectedBase64 = [Convert]::ToBase64String($Expected)

function Target-MatchesExpected {
  if (-not (Test-Path -LiteralPath $Target)) { return $false }
  return [Convert]::ToBase64String([IO.File]::ReadAllBytes($Target)) -eq $ExpectedBase64
}

function Invoke-Process([string]$File, [string[]]$Arguments) {
  $Info = [System.Diagnostics.ProcessStartInfo]::new()
  $Info.FileName = $File
  $Info.UseShellExecute = $false
  foreach ($Argument in $Arguments) { $Info.ArgumentList.Add($Argument) }
  $Process = [System.Diagnostics.Process]::Start($Info)
  $Process.WaitForExit()
  return $Process.ExitCode
}

function Assert-InstallerExit([string]$Label, [int]$ExitCode, [int[]]$Allowed) {
  if ($Allowed -notcontains $ExitCode) {
    throw "$Label failed with exit code $ExitCode"
  }
}

function Find-InstalledBinary([string]$InstallRoot) {
  $Found = @(Get-ChildItem -LiteralPath $InstallRoot -Filter $Executable -File -Recurse)
  if ($Found.Count -ne 1) {
    throw "expected exactly one $Executable below $InstallRoot, found $($Found.Count)"
  }
  return $Found[0].FullName
}

function Assert-EmbeddedRoutes([string]$Binary) {
  & node (Join-Path $Root "scripts/check-package-artifacts.mjs") --binary $Binary
  if ($LASTEXITCODE -ne 0) { throw "installed executable has the wrong embedded UI route set" }
}

function Assert-FileDigestMatches([string]$Actual, [string]$Expected, [string]$Label) {
  $ActualDigest = (Get-FileHash -LiteralPath $Actual -Algorithm SHA256).Hash
  $ExpectedDigest = (Get-FileHash -LiteralPath $Expected -Algorithm SHA256).Hash
  if ($ActualDigest -ne $ExpectedDigest) {
    throw "$Label bytes differ from the inspected release binary"
  }
}

function Stop-SmokeProcess([System.Diagnostics.Process]$Process) {
  $Process.Refresh()
  if (-not $Process.HasExited) {
    if (-not $Process.CloseMainWindow()) {
      Stop-Process -Id $Process.Id -Force
    } elseif (-not $Process.WaitForExit(5000)) {
      Stop-Process -Id $Process.Id -Force
    }
    $Process.WaitForExit()
  }
}

function Start-And-Assert([string]$Binary, [bool]$WaitForMigration) {
  $Process = Start-Process -FilePath $Binary -WorkingDirectory (Split-Path -Parent $Binary) -PassThru
  try {
    for ($Attempt = 0; $Attempt -lt 80; $Attempt++) {
      Start-Sleep -Milliseconds 250
      $Process.Refresh()
      if ($Process.HasExited) {
        throw "installed app exited early with code $($Process.ExitCode)"
      }
      if (-not $WaitForMigration -or (Target-MatchesExpected)) {
        Start-Sleep -Seconds 1
        $Process.Refresh()
        if ($Process.HasExited) { throw "installed app did not remain alive" }
        if (-not (Target-MatchesExpected)) { throw "installed app changed activated platform state" }
        return
      }
    }
    throw "installed app did not expose the expected migrated state"
  } finally {
    Stop-SmokeProcess $Process
  }
}

function Assert-Installed(
  [string]$InstallRoot,
  [string]$ExpectedVersion,
  [bool]$WaitForMigration,
  [int]$Launches,
  [string]$ExpectedThumbprint,
  [string]$ExpectedBinary
) {
  $Binary = Find-InstalledBinary $InstallRoot
  Assert-PeIdentity $Binary $ExpectedVersion "installed executable"
  Assert-EmbeddedRoutes $Binary
  if ($ExpectedThumbprint) {
    [void](Assert-AuthenticodeTrust $Binary $ExpectedThumbprint "installed executable")
  }
  if ($ExpectedBinary) {
    Assert-FileDigestMatches $Binary $ExpectedBinary "installed executable"
    & node (Join-Path $Root "scripts/check-package-artifacts.mjs") --helpers $Binary
    if ($LASTEXITCODE -ne 0) { throw "installed package is missing a helper executable" }
  }
  for ($Index = 0; $Index -lt $Launches; $Index++) {
    Start-And-Assert $Binary ($WaitForMigration -and $Index -eq 0)
  }
}

function Install-Msi([string]$Package, [string]$InstallRoot, [string]$Label) {
  $Log = Join-Path $env:RUNNER_TEMP "zerocode-msi-install.log"
  $ExitCode = Invoke-Process "msiexec.exe" @("/i", $Package, "/qn", "/norestart", "/L*V", $Log, "INSTALLDIR=$InstallRoot")
  Assert-InstallerExit $Label $ExitCode @(0, 1641, 3010)
}

function Install-Nsis([string]$Package, [string]$InstallRoot, [string]$Label) {
  $ExitCode = Invoke-Process $Package @("/S", "/D=$InstallRoot")
  Assert-InstallerExit $Label $ExitCode @(0)
}

$MsiRoot = Join-Path $env:RUNNER_TEMP "zerocode-msi-install"
$NsisRoot = Join-Path $env:RUNNER_TEMP "zerocode-nsis-install"
foreach ($Path in @($MsiRoot, $NsisRoot)) {
  if (Test-Path -LiteralPath $Path) { throw "install root already exists: $Path" }
}

if ($PreviousMsi) {
  Install-Msi $PreviousMsi $MsiRoot "previous MSI install"
  Assert-MsiProductState $PreviousProductCode $InstallStateDefault "previous MSI after install"
  Assert-MsiProductState $CurrentProductCode $InstallStateUnknown "current MSI before upgrade"
  Assert-Installed $MsiRoot $PreviousVersion $true 1 $ExpectedSigner ""
  [IO.File]::WriteAllBytes($Legacy, [Convert]::FromBase64String($AfterBase64))
  Install-Msi $Msi $MsiRoot "MSI upgrade"
  Assert-MsiProductState $CurrentProductCode $InstallStateDefault "current MSI after upgrade"
  Assert-MsiProductState $PreviousProductCode $InstallStateUnknown "previous MSI after upgrade"
  Assert-Installed $MsiRoot $Version $false 2 $ExpectedSigner $ReleaseBinary
} else {
  Install-Msi $Msi $MsiRoot "MSI install"
  Assert-MsiProductState $CurrentProductCode $InstallStateDefault "current MSI after install"
  Assert-Installed $MsiRoot $Version $true 2 $ExpectedSigner $ReleaseBinary
  [IO.File]::WriteAllBytes($Legacy, [Convert]::FromBase64String($AfterBase64))
}
$MsiExit = Invoke-Process "msiexec.exe" @("/x", $Msi, "/qn", "/norestart")
Assert-InstallerExit "MSI uninstall" $MsiExit @(0, 1641, 3010)
Assert-MsiProductState $CurrentProductCode $InstallStateUnknown "current MSI after uninstall"

if ($PreviousNsis) {
  Install-Nsis $PreviousNsis $NsisRoot "previous NSIS install"
  Assert-Installed $NsisRoot $PreviousVersion $false 1 $ExpectedSigner ""
  Install-Nsis $Nsis $NsisRoot "NSIS upgrade"
} else {
  Install-Nsis $Nsis $NsisRoot "NSIS install"
}
Assert-Installed $NsisRoot $Version $false 2 $ExpectedSigner $ReleaseBinary

# Tauri's stock NSIS uninstaller removes LocalAppData/<bundle-id>. Do not run it
# here: the ephemeral runner is discarded, while deleting the state would make
# this a destructive uninstall test rather than a persistence/upgrade smoke.
if ($Stage) {
  if (Test-Path -LiteralPath $Stage) { throw "artifact staging path already exists: $Stage" }
  New-Item -ItemType Directory -Path $Stage | Out-Null
  Copy-Item -LiteralPath $Msi -Destination $Stage
  Copy-Item -LiteralPath $Nsis -Destination $Stage
  & node (Join-Path $Root "scripts/package-stage-manifest.mjs") write windows $Stage
  if ($LASTEXITCODE -ne 0) { throw "current Windows package manifest creation failed" }
}

if ($PreviousMsi) {
  Write-Host "checked Windows $PreviousVersion -> $Version MSI/NSIS upgrade, launch, terminate, relaunch, identity, and state retention"
} else {
  Write-Host "checked Windows MSI/NSIS install, launch, terminate, relaunch, identity, and legacy migration"
}

<#
.SYNOPSIS
  The Windows Computer Use acceptance gate: build, functional fixture tests,
  the measured performance baseline, and a report folder a person can read.

.DESCRIPTION
  Runs on a Windows host with the repository's Rust toolchain. Nothing here
  touches the person's apps: every functional test drives a disposable
  fixture window the test process creates and closes itself. The report
  says what was native-tested (this run), fixture-tested, and skipped —
  a test that needs the fixture in the foreground steps aside on a desktop
  that refuses it and says so in its output.

  Produces under -Out (default: $env:RUNNER_TEMP or the system temp):
    computer-use-windows-acceptance.json  — environment + gate outcomes
    perf-windows.json                     — p50/p95 per axis, N=20 (provider)
    perf-doors-windows.json               — p50/p95 of the CLI door launch
                                            (.cmd → powershell → --help), N=20
    cargo-test.log                        — the raw test output

  The verdict is "passed" only when every gate passed AND no foreground
  test stepped aside; foreground skips make it "passed-partial", because an
  axis that did not run is not an axis that passed.

.PARAMETER Out
  Report folder. Created if missing; refused if it already holds a report.
.PARAMETER SkipBuild
  Reuse an existing target directory without a separate build step.
#>
param(
  [string]$Out,
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $IsWindows) {
  throw "computer-use-windows-acceptance.ps1 runs on Windows only; on another host it can only be read"
}

$Root = Split-Path -Parent $PSScriptRoot
if (-not $Out) {
  $Out = Join-Path ($env:RUNNER_TEMP ?? [IO.Path]::GetTempPath()) "zerocode-computer-use-acceptance"
}
if (Test-Path -LiteralPath (Join-Path $Out "computer-use-windows-acceptance.json")) {
  throw "report folder already holds a report: $Out"
}
New-Item -ItemType Directory -Path $Out -Force | Out-Null

function Invoke-Logged([string]$Label, [string]$File, [string[]]$Arguments, [string]$Log) {
  Write-Host "== $Label"
  $Info = [System.Diagnostics.ProcessStartInfo]::new()
  $Info.FileName = $File
  $Info.UseShellExecute = $false
  $Info.RedirectStandardOutput = $true
  $Info.RedirectStandardError = $true
  $Info.WorkingDirectory = $Root
  foreach ($Argument in $Arguments) { $Info.ArgumentList.Add($Argument) }
  $Process = [System.Diagnostics.Process]::Start($Info)
  $StdOut = $Process.StandardOutput.ReadToEndAsync()
  $StdErr = $Process.StandardError.ReadToEndAsync()
  $Process.WaitForExit()
  $Text = $StdOut.Result + "`n" + $StdErr.Result
  Add-Content -LiteralPath $Log -Value ("### $Label`n" + $Text)
  return @{ ExitCode = $Process.ExitCode; Text = $Text }
}

$Log = Join-Path $Out "cargo-test.log"
$Perf = Join-Path $Out "perf-windows.json"
$DoorPerf = Join-Path $Out "perf-doors-windows.json"
$Environment = @{
  osVersion       = [System.Environment]::OSVersion.VersionString
  machine         = $env:COMPUTERNAME
  processorCount  = [System.Environment]::ProcessorCount
  dpiAwareness    = "the provider thread and the fixture test thread set PER_MONITOR_AWARE_V2 themselves (see perf-windows.json environment.dpi)"
  executionPolicy = (& powershell.exe -NoProfile -Command "Get-ExecutionPolicy" 2>$null | Out-String).Trim()
  monitors        = @(Get-CimInstance -ClassName Win32_DesktopMonitor | ForEach-Object { "$($_.ScreenWidth)x$($_.ScreenHeight)" })
  interactive     = [System.Environment]::UserInteractive
  githubActions   = ($env:GITHUB_ACTIONS -eq "true")
  runnerEnv       = $env:RUNNER_ENVIRONMENT
  commit          = (& git -C $Root rev-parse HEAD).Trim()
  rustc           = (& rustc --version).Trim()
}

$Gates = [ordered]@{}

if (-not $SkipBuild) {
  $Build = Invoke-Logged "cargo build (shell, Computer Use tests)" "cargo" @("test", "-p", "zerocode-shell", "--no-run") $Log
  $Gates["build"] = @{ passed = ($Build.ExitCode -eq 0); exitCode = $Build.ExitCode }
  if ($Build.ExitCode -ne 0) {
    $Report = @{ environment = $Environment; gates = $Gates; verdict = "build failed"; evidenceClass = "native-attempted" }
    $Report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $Out "computer-use-windows-acceptance.json")
    throw "the shell crate did not build; see $Log"
  }
}

# The platform-neutral halves and the core contract, on this host.
$Core = Invoke-Logged "core contract tests" "cargo" @("test", "-p", "zerocode-core", "computer_use", "--", "--nocapture") $Log
$Gates["core.contract"] = @{ passed = ($Core.ExitCode -eq 0); exitCode = $Core.ExitCode }

$Neutral = Invoke-Logged "shell platform-neutral tests" "cargo" @("test", "-p", "zerocode-shell", "computer_use::", "--", "--skip", "computer_use::windows::tests", "--nocapture") $Log
$Gates["shell.neutral"] = @{ passed = ($Neutral.ExitCode -eq 0); exitCode = $Neutral.ExitCode }

# The agent doors as a PowerShell host really invokes them: the bare name
# under the default (Restricted) execution policy must resolve to the .cmd
# wrapper, never to a .ps1 the policy refuses. This is the M5 gate.
$Doors = Invoke-Logged "door scripts under Restricted policy" "cargo" @("test", "-p", "zerocode-shell", "hooks::tests::the_bare_door_names_run_under_a_restricted_execution_policy", "--", "--nocapture") $Log
$Gates["doors.restricted"] = @{ passed = ($Doors.ExitCode -eq 0); exitCode = $Doors.ExitCode }

# The functional gate against the fixture window — serial, one thread.
$Functional = Invoke-Logged "windows fixture tests" "cargo" @("test", "-p", "zerocode-shell", "computer_use::windows::tests", "--", "--test-threads=1", "--nocapture") $Log
$Skipped = @([regex]::Matches($Functional.Text, "skipping ([^:]+):") | ForEach-Object { $_.Groups[1].Value })
$Gates["windows.functional"] = @{
  passed   = ($Functional.ExitCode -eq 0)
  exitCode = $Functional.ExitCode
  skipped  = $Skipped
  partial  = ($Skipped.Count -gt 0)
  note     = if ($Skipped.Count -gt 0) { "tests needing the fixture in the foreground stepped aside; foreground-dependent axes are not proved by this run" } else { "" }
}

# The measured baseline.
$env:ZEROCODE_COMPUTER_PERF = "1"
$env:ZEROCODE_COMPUTER_PERF_OUT = $Perf
try {
  $Measure = Invoke-Logged "windows perf baseline (N=20)" "cargo" @("test", "-p", "zerocode-shell", "computer_use::windows::tests::perf_baseline", "--", "--test-threads=1", "--nocapture") $Log
} finally {
  Remove-Item Env:ZEROCODE_COMPUTER_PERF -ErrorAction SilentlyContinue
  Remove-Item Env:ZEROCODE_COMPUTER_PERF_OUT -ErrorAction SilentlyContinue
}
$PerfSkipped = @([regex]::Matches($Measure.Text, "skipping ([^:]+):") | ForEach-Object { $_.Groups[1].Value })
$Gates["windows.perf"] = @{
  passed   = ($Measure.ExitCode -eq 0 -and (Test-Path -LiteralPath $Perf))
  exitCode = $Measure.ExitCode
  skipped  = $PerfSkipped
  partial  = ($PerfSkipped.Count -gt 0)
  report   = if (Test-Path -LiteralPath $Perf) { Get-Content -LiteralPath $Perf -Raw | ConvertFrom-Json } else { $null }
}

# The complete CLI path's launch cost: cmd → .cmd wrapper → powershell.exe →
# the door script's own `--help` (answered locally, no bridge), so the
# in-process provider numbers above are read together with what an agent's
# tool call pays before it reaches the window at all.
$env:ZEROCODE_COMPUTER_PERF = "1"
$env:ZEROCODE_COMPUTER_PERF_OUT = $DoorPerf
try {
  $DoorMeasure = Invoke-Logged "door launch overhead (N=20)" "cargo" @("test", "-p", "zerocode-shell", "hooks::tests::door_launch_overhead", "--", "--nocapture") $Log
} finally {
  Remove-Item Env:ZEROCODE_COMPUTER_PERF -ErrorAction SilentlyContinue
  Remove-Item Env:ZEROCODE_COMPUTER_PERF_OUT -ErrorAction SilentlyContinue
}
$Gates["doors.launch"] = @{
  passed   = ($DoorMeasure.ExitCode -eq 0 -and (Test-Path -LiteralPath $DoorPerf))
  exitCode = $DoorMeasure.ExitCode
  report   = if (Test-Path -LiteralPath $DoorPerf) { Get-Content -LiteralPath $DoorPerf -Raw | ConvertFrom-Json } else { $null }
}

$AllPassed = ($Gates.Values | Where-Object { -not $_.passed }).Count -eq 0
$AnyPartial = ($Gates.Values | Where-Object { $_.ContainsKey("partial") -and $_.partial }).Count -gt 0
$AllSkipped = @($Skipped) + @($PerfSkipped)
$Report = @{
  environment   = $Environment
  gates         = $Gates
  verdict       = if (-not $AllPassed) { "failed" } elseif ($AnyPartial) { "passed-partial" } else { "passed" }
  evidenceClass = if ($AnyPartial) { "native-tested (fixture window, this host; foreground axes skipped — partial)" } else { "native-tested (fixture window, this host)" }
  unproved      = @(
    "elevated target refusal (needs an elevated app; not launched by this gate)",
    "high-DPI mixed monitors (only what this host has: see environment.monitors and perf-windows.json environment.dpi)",
    "real third-party apps (the fixture is standard Win32 controls)",
    "a blocked input queue (BlockInput needs UIAccess; the refusal wording is compiled, not run)"
  ) + $AllSkipped
}
$Report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Out "computer-use-windows-acceptance.json")
Write-Host "report: $Out"
if (-not $AllPassed) { throw "a Computer Use gate failed; see $Log" }

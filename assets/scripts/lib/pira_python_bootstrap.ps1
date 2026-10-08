# Shared Python 3 discovery/bootstrap helpers for PIRA PowerShell wrappers.

function Test-PiraPythonCandidate {
    param(
        [Parameter(Mandatory = $true)][string]$File,
        [string[]]$PrefixArgs = @(),
        [int]$MinimumMinor = 0
    )
    $testArgs = @($PrefixArgs + @("-c", "import sys; raise SystemExit(0 if sys.version_info[0] == 3 and sys.version_info >= (3, $MinimumMinor) else 1)"))
    try {
        & $File @testArgs *> $null
        return ($LASTEXITCODE -eq 0)
    } catch {
        return $false
    }
}

function Find-PiraPython3 {
    param([int]$MinimumMinor = 0)
    # Audio keeps Python 3; configuration explicitly requests minor version 11.
    $candidates = @(
        @{ File = "py"; PrefixArgs = @("-3") },
        @{ File = "python3"; PrefixArgs = @() },
        @{ File = "python"; PrefixArgs = @() }
    )
    # PIRA: version-only fallbacks cover 3.11-3.14; extend for newer names if needed.
    foreach ($minor in 14, 13, 12, 11) {
        $candidates += @{ File = "python3.$minor"; PrefixArgs = @() }
        $candidates += @{ File = "py"; PrefixArgs = @("-3.$minor") }
    }
    foreach ($candidate in $candidates) {
        if (-not (Get-Command $candidate.File -ErrorAction SilentlyContinue)) { continue }
        if (Test-PiraPythonCandidate -File $candidate.File -PrefixArgs $candidate.PrefixArgs -MinimumMinor $MinimumMinor) {
            return @{ File = $candidate.File; PrefixArgs = $candidate.PrefixArgs }
        }
    }
    return $null
}

function Test-PiraAssumeYes {
    param([string[]]$Args = @())
    if ($env:PIRA_SETUP_ASSUME_YES -eq "1") { return $true }
    return @($Args) -contains "--yes"
}

function Install-PiraPythonHint {
    param([string[]]$Args = @())
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if (-not $winget) {
        Write-Error "Python 3.11+ was not found and winget is unavailable. Install Python 3.11+ from https://www.python.org/downloads/ or Microsoft Store, then rerun this setup wrapper."
    }

    Write-Host "Python 3.11+ was not found. It can be installed with winget:"
    Write-Host "  winget install --id Python.Python.3.14 --source winget"
    $install = $false
    if (Test-PiraAssumeYes -Args $Args) {
        $install = $true
    } else {
        $answer = Read-Host "Install Python 3.11+ now with winget? [y/N]"
        $install = @("y", "yes") -contains $answer.Trim().ToLowerInvariant()
    }
    if (-not $install) {
        Write-Error "Python 3.11+ is required."
    }

    & winget install --id Python.Python.3.14 --source winget --accept-package-agreements --accept-source-agreements | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Error "winget failed to install Python 3.11+. Install Python 3.11+ manually, then rerun this setup wrapper."
    }
}

function Require-PiraPython3 {
    $python = Find-PiraPython3
    if (-not $python) {
        Write-Error "Python 3 is required. Run assets/scripts/setup_pira.ps1 first, or install Python 3 and retry."
    }
    return $python
}

function Bootstrap-PiraPython3 {
    param([string[]]$Args = @())
    $python = Find-PiraPython3 -MinimumMinor 11
    if (-not $python) {
        Install-PiraPythonHint -Args $Args
        $python = Find-PiraPython3 -MinimumMinor 11
    }
    if (-not $python) {
        Write-Error "Python 3.11+ is required for PIRA configuration. Install it, reopen your terminal to refresh PATH, and rerun this setup wrapper."
    }
    return $python
}

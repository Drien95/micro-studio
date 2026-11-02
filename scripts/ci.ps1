# CI locale (Windows / PowerShell) : les mêmes vérifications que GitHub, en
# quelques secondes une fois le projet compilé.
#
#   .\scripts\ci.ps1          vérifications rapides : format, clippy, tests
#   .\scripts\ci.ps1 -Full    + documentation, dépendances (cargo-deny), couverture
#
# GitHub reste la vérification finale (Windows, Linux et macOS, MSRV).
param([switch]$Full)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$started = Get-Date

function Step([string]$Name, [scriptblock]$Cmd) {
    Write-Host "==> $Name" -ForegroundColor Cyan
    $t = Get-Date
    & $Cmd
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ÉCHEC : $Name" -ForegroundColor Red
        exit $LASTEXITCODE
    }
    Write-Host ("    ok ({0:N0} s)" -f ((Get-Date) - $t).TotalSeconds) -ForegroundColor Green
}

function Has([string]$Sub) {
    cargo $Sub --version *> $null
    return $LASTEXITCODE -eq 0
}

Step 'Format (rustfmt)' { cargo fmt --all --check }
Step 'Analyse statique (clippy)' { cargo clippy --all-targets --locked -- -D warnings }
Step 'Tests' { cargo test --locked --all-targets }

if ($Full) {
    $env:RUSTDOCFLAGS = '-D warnings'
    Step 'Documentation' { cargo doc --no-deps --locked }
    Remove-Item Env:RUSTDOCFLAGS
    if (Has 'deny') {
        Step 'Dépendances (cargo-deny)' { cargo deny check }
    } else {
        Write-Host '    cargo-deny absent, étape ignorée (installer : cargo install --locked cargo-deny)' -ForegroundColor Yellow
    }
    if (Has 'llvm-cov') {
        Step 'Couverture (seuil 85 %)' {
            cargo llvm-cov --locked --all-targets --summary-only --ignore-filename-regex 'src[\\/]main\.rs' --fail-under-lines 85
        }
    } else {
        Write-Host '    cargo-llvm-cov absent, étape ignorée (installer : cargo install --locked cargo-llvm-cov)' -ForegroundColor Yellow
    }
}

Write-Host ("Tout est vert en {0:N0} s." -f ((Get-Date) - $started).TotalSeconds) -ForegroundColor Green

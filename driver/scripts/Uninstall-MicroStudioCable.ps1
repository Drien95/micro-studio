#Requires -Version 5.1
#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Désinstalle complètement le pilote « Micro Studio Cable ».

.DESCRIPTION
    Supprime le(s) périphérique(s) ROOT\MicroStudioCable puis retire le pilote
    du magasin de pilotes Windows. Le mode test n'est pas désactivé : pour le
    faire, lance « bcdedit /set testsigning off » puis redémarre.
#>
[CmdletBinding(SupportsShouldProcess)]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$HardwareId = 'ROOT\MicroStudioCable'

$devices = @(Get-PnpDevice -PresentOnly:$false -ErrorAction SilentlyContinue |
        Where-Object { $_.HardwareID -contains $HardwareId })
foreach ($device in $devices) {
    if ($PSCmdlet.ShouldProcess($device.InstanceId, 'Supprimer le périphérique')) {
        & pnputil.exe /remove-device $device.InstanceId
        if ($LASTEXITCODE -ne 0) { throw "pnputil /remove-device a échoué ($LASTEXITCODE)." }
    }
}

$drivers = @(Get-WindowsDriver -Online -All |
        Where-Object { $_.OriginalFileName -like '*\MicroStudioCable.inf' })
foreach ($driver in $drivers) {
    if ($PSCmdlet.ShouldProcess($driver.Driver, 'Retirer le pilote du magasin')) {
        & pnputil.exe /delete-driver $driver.Driver /uninstall /force
        if ($LASTEXITCODE -ne 0) { throw "pnputil /delete-driver a échoué ($LASTEXITCODE)." }
    }
}

Write-Host "Micro Studio Cable désinstallé ($($devices.Count) périphérique(s), $($drivers.Count) pilote(s))." -ForegroundColor Green

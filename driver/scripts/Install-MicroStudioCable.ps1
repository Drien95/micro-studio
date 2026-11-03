#Requires -Version 5.1
#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Installe le pilote « Micro Studio Cable » (micro virtuel) en MODE TEST.

.DESCRIPTION
    Prototype : le pilote n'est signé qu'avec un certificat de test généré par
    le WDK. Windows ne l'accepte donc que si le « mode test » est activé, ce
    qui exige que le Secure Boot soit désactivé dans le BIOS/UEFI.

    Étapes :
      1. vérifie que le mode test est actif (sinon propose de l'activer) ;
      2. fait confiance au certificat de test livré avec le pilote ;
      3. crée le périphérique virtuel ROOT\MicroStudioCable s'il n'existe pas ;
      4. installe ou met à jour le pilote sur ce périphérique.

.PARAMETER PackagePath
    Dossier contenant MicroStudioCable.inf, .sys, .cat et package.cer
    (l'artefact « micro-studio-cable-x64 » de la CI, décompressé).

.EXAMPLE
    .\Install-MicroStudioCable.ps1 -PackagePath C:\Temp\micro-studio-cable-x64
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [Parameter(Mandatory)]
    [ValidateScript({ Test-Path -LiteralPath $_ -PathType Container })]
    [string] $PackagePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$HardwareId = 'ROOT\MicroStudioCable'
$MediaClassGuid = [Guid]'4d36e96c-e325-11ce-bfc1-08002be10318'

function Test-TestSigningEnabled {
    $output = & bcdedit.exe /enum '{current}' 2>&1 | Out-String
    return ($output -match '(?im)^\s*testsigning\s+(Yes|Oui)\s*$')
}

function Get-CableDevice {
    Get-PnpDevice -PresentOnly:$false -ErrorAction SilentlyContinue |
        Where-Object { $_.HardwareID -contains $HardwareId }
}

function Import-TestCertificate([string] $CertPath) {
    foreach ($store in 'Root', 'TrustedPublisher') {
        Import-Certificate -FilePath $CertPath -CertStoreLocation "Cert:\LocalMachine\$store" | Out-Null
    }
}

# Création d'un périphérique « racine » (équivalent de « devcon install »).
$setupApi = @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class MsCableSetup
{
    [StructLayout(LayoutKind.Sequential)]
    struct SP_DEVINFO_DATA { public int cbSize; public Guid ClassGuid; public int DevInst; public IntPtr Reserved; }

    const int DICD_GENERATE_ID = 0x1;
    const int SPDRP_HARDWAREID = 0x1;
    const int DIF_REGISTERDEVICE = 0x19;
    const int INSTALLFLAG_FORCE = 0x1;

    [DllImport("setupapi.dll", SetLastError = true)]
    static extern IntPtr SetupDiCreateDeviceInfoList(ref Guid classGuid, IntPtr hwnd);
    [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool SetupDiCreateDeviceInfoW(IntPtr set, string name, ref Guid classGuid, string desc, IntPtr hwnd, int flags, ref SP_DEVINFO_DATA data);
    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiSetDeviceRegistryPropertyW(IntPtr set, ref SP_DEVINFO_DATA data, int property, byte[] buffer, int size);
    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiCallClassInstaller(int function, IntPtr set, ref SP_DEVINFO_DATA data);
    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);
    [DllImport("newdev.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool UpdateDriverForPlugAndPlayDevicesW(IntPtr hwnd, string hardwareId, string infPath, int flags, out bool reboot);

    public static void CreateRootDevice(Guid classGuid, string className, string hardwareId)
    {
        IntPtr set = SetupDiCreateDeviceInfoList(ref classGuid, IntPtr.Zero);
        if (set == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            var data = new SP_DEVINFO_DATA { cbSize = Marshal.SizeOf(typeof(SP_DEVINFO_DATA)) };
            if (!SetupDiCreateDeviceInfoW(set, className, ref classGuid, null, IntPtr.Zero, DICD_GENERATE_ID, ref data))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            byte[] ids = System.Text.Encoding.Unicode.GetBytes(hardwareId + "\0\0");
            if (!SetupDiSetDeviceRegistryPropertyW(set, ref data, SPDRP_HARDWAREID, ids, ids.Length))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (!SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set, ref data))
                throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        finally { SetupDiDestroyDeviceInfoList(set); }
    }

    public static bool InstallDriver(string hardwareId, string infPath)
    {
        bool reboot;
        if (!UpdateDriverForPlugAndPlayDevicesW(IntPtr.Zero, hardwareId, infPath, INSTALLFLAG_FORCE, out reboot))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        return reboot;
    }
}
'@

# ---------------------------------------------------------------------------
$inf = Join-Path $PackagePath 'MicroStudioCable.inf'
$cer = Join-Path $PackagePath 'package.cer'
foreach ($file in $inf, (Join-Path $PackagePath 'MicroStudioCable.sys'), (Join-Path $PackagePath 'MicroStudioCable.cat'), $cer) {
    if (-not (Test-Path -LiteralPath $file)) { throw "Fichier manquant : $file" }
}

if (-not (Test-TestSigningEnabled)) {
    Write-Warning "Le mode test de Windows n'est pas actif : le pilote serait refusé."
    Write-Warning 'Il faut d''abord désactiver le Secure Boot dans le BIOS/UEFI.'
    if ($PSCmdlet.ShouldProcess('Windows', 'Activer le mode test (bcdedit /set testsigning on)')) {
        & bcdedit.exe /set testsigning on
        if ($LASTEXITCODE -ne 0) { throw 'bcdedit a échoué (Secure Boot encore actif ?).' }
        Write-Host 'Mode test activé. Redémarre le PC puis relance ce script.' -ForegroundColor Yellow
    }
    return
}

if ($PSCmdlet.ShouldProcess($cer, 'Faire confiance au certificat de test')) {
    Import-TestCertificate $cer
}

Add-Type -TypeDefinition $setupApi -Language CSharp
if (-not (Get-CableDevice)) {
    if ($PSCmdlet.ShouldProcess($HardwareId, 'Créer le périphérique virtuel')) {
        [MsCableSetup]::CreateRootDevice($MediaClassGuid, 'MEDIA', $HardwareId)
    }
}
if ($PSCmdlet.ShouldProcess($inf, 'Installer le pilote')) {
    $reboot = [MsCableSetup]::InstallDriver($HardwareId, (Resolve-Path -LiteralPath $inf).Path)
    Write-Host 'Micro Studio Cable installé : « Micro Studio Sortie » et « Micro Studio Micro » sont disponibles.' -ForegroundColor Green
    if ($reboot) { Write-Host 'Windows demande un redémarrage pour terminer.' -ForegroundColor Yellow }
}

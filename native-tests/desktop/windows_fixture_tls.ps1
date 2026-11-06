<#
.SYNOPSIS
Creates the W3 fixture site's TLS files and trusts their CA on this machine,
or removes that trust again (-Remove).

.DESCRIPTION
windows_browser_process.py serves its local fixture pages at
https://127.0.0.1:<port>. Chromium (CEF) on Windows verifies that certificate
against the machine's trust store, and the App passes the browser no switch
that could relax it, so this script:

- creates a CA ("Takosumi W3 fixture CA (disposable)", valid for two days) and
  a leaf for the IP address 127.0.0.1 (serverAuth, valid for one day) signed
  by it, both RSA 2048 / SHA-256;
- writes cert.pem (leaf, then CA), key.pem (the leaf's PKCS#8 key), ca.pem and
  ca-thumbprint.txt into the new directory -Out;
- adds the CA's public certificate to LocalMachine\Root, which needs an
  elevated PowerShell 7 (the windows-latest runner is one). The CA's private
  key is never written anywhere.

-Remove takes the CA recorded in -Out\ca-thumbprint.txt out of
LocalMachine\Root. Meant for disposable machines such as the CI runner; on
another machine, run -Remove when done.
#>
param(
    [Parameter(Mandatory = $true)][string]$Out,
    [switch]$Remove
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$x509 = 'System.Security.Cryptography.X509Certificates'
$thumbprintFile = Join-Path $Out 'ca-thumbprint.txt'

function Open-RootStore {
    $store = New-Object "$x509.X509Store" ('Root', 'LocalMachine')
    $store.Open('ReadWrite')
    return $store
}

if ($Remove) {
    $thumbprint = (Get-Content -LiteralPath $thumbprintFile -Raw).Trim()
    if ($thumbprint -notmatch '^[0-9A-F]{40}$') { throw "Unexpected CA thumbprint in $thumbprintFile" }
    $store = Open-RootStore
    try {
        $found = $store.Certificates.Find('FindByThumbprint', $thumbprint, $false)
        foreach ($certificate in $found) { $store.Remove($certificate) }
        Write-Output "Removed $($found.Count) fixture CA certificate(s) $thumbprint from LocalMachine\Root"
    } finally {
        $store.Close()
    }
    return
}

if (Test-Path -LiteralPath $Out) { throw "-Out must name a new directory: $Out" }
New-Item -ItemType Directory -Path $Out | Out-Null

$sha256 = [System.Security.Cryptography.HashAlgorithmName]::SHA256
$pkcs1 = [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
$now = [DateTimeOffset]::UtcNow

$caKey = [System.Security.Cryptography.RSA]::Create(2048)
$caRequest = New-Object "$x509.CertificateRequest" ('CN=Takosumi W3 fixture CA (disposable)', $caKey, $sha256, $pkcs1)
$caRequest.CertificateExtensions.Add((New-Object "$x509.X509BasicConstraintsExtension" ($true, $true, 0, $true)))
$caRequest.CertificateExtensions.Add((New-Object "$x509.X509KeyUsageExtension" (
    [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]'KeyCertSign, CrlSign', $true)))
$caRequest.CertificateExtensions.Add((New-Object "$x509.X509SubjectKeyIdentifierExtension" ($caRequest.PublicKey, $false)))
$ca = $caRequest.CreateSelfSigned($now.AddHours(-1), $now.AddDays(2))

$leafKey = [System.Security.Cryptography.RSA]::Create(2048)
$leafRequest = New-Object "$x509.CertificateRequest" ('CN=127.0.0.1', $leafKey, $sha256, $pkcs1)
$leafRequest.CertificateExtensions.Add((New-Object "$x509.X509BasicConstraintsExtension" ($false, $false, 0, $true)))
$leafRequest.CertificateExtensions.Add((New-Object "$x509.X509KeyUsageExtension" (
    [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]'DigitalSignature, KeyEncipherment', $true)))
$usages = New-Object System.Security.Cryptography.OidCollection
[void]$usages.Add((New-Object System.Security.Cryptography.Oid ('1.3.6.1.5.5.7.3.1')))
$leafRequest.CertificateExtensions.Add((New-Object "$x509.X509EnhancedKeyUsageExtension" ($usages, $false)))
$names = New-Object "$x509.SubjectAlternativeNameBuilder"
$names.AddIpAddress([System.Net.IPAddress]::Parse('127.0.0.1'))
$leafRequest.CertificateExtensions.Add($names.Build($true))
$leafRequest.CertificateExtensions.Add((New-Object "$x509.X509SubjectKeyIdentifierExtension" ($leafRequest.PublicKey, $false)))
$leafRequest.CertificateExtensions.Add(
    [System.Security.Cryptography.X509Certificates.X509AuthorityKeyIdentifierExtension]::CreateFromCertificate($ca, $true, $false))
$serial = [System.Security.Cryptography.RandomNumberGenerator]::GetBytes(16)
$serial[0] = $serial[0] -band 0x7F
$leaf = $leafRequest.Create($ca, $now.AddHours(-1), $now.AddDays(1), $serial)

[System.IO.File]::WriteAllText((Join-Path $Out 'cert.pem'), $leaf.ExportCertificatePem() + "`n" + $ca.ExportCertificatePem() + "`n")
[System.IO.File]::WriteAllText((Join-Path $Out 'key.pem'), $leafKey.ExportPkcs8PrivateKeyPem() + "`n")
[System.IO.File]::WriteAllText((Join-Path $Out 'ca.pem'), $ca.ExportCertificatePem() + "`n")

$public = New-Object "$x509.X509Certificate2" (, $ca.RawData)
$store = Open-RootStore
try {
    $store.Add($public)
} finally {
    $store.Close()
}
[System.IO.File]::WriteAllText($thumbprintFile, $public.Thumbprint + "`n")
Write-Output "Trusted fixture CA $($public.Thumbprint) in LocalMachine\Root; TLS files in $Out"

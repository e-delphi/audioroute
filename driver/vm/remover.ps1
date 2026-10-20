# Remove o driver de teste. RODE SÓ NA VM, como administrador:
#   powershell -ExecutionPolicy Bypass -File .\remover.ps1
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$hwid = 'ROOT\SimpleAudioSample'

.\devcon.exe remove $hwid
# Tira o pacote do repositório de drivers do Windows (oemNN.inf).
pnputil /enum-drivers | Select-String -Context 1, 0 'simpleaudiosample.inf' | ForEach-Object {
  $published = ($_.Context.PreContext[0] -split ':\s*')[1]
  if ($published) { pnputil /delete-driver $published /uninstall /force | Out-Null }
}
Write-Host 'Driver removido. Para desligar o modo de teste: bcdedit /set testsigning off (e reinicie).'

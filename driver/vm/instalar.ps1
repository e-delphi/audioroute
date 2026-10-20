# Instala o driver de teste. RODE SÓ NA VM, como administrador:
#   powershell -ExecutionPolicy Bypass -File .\instalar.ps1
#
# 1ª vez: liga o modo de teste do Windows (aceita drivers com assinatura de
# teste) e pede para reiniciar. Depois de reiniciar, rode de novo.
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$hwid = 'ROOT\SimpleAudioSample'

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Rode este script como administrador.'
}

# Modo de teste: precisa do Secure Boot desligado nas configurações da VM.
if (-not ((bcdedit /enum '{current}') -match 'testsigning\s+Yes')) {
  bcdedit /set testsigning on | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw 'Não deu para ligar o modo de teste. Desligue o Secure Boot nas configurações da VM (Hyper-V > Segurança).'
  }
  Write-Host 'Modo de teste ligado. Reinicie a VM e rode este script de novo.' -ForegroundColor Yellow
  return
}

# O certificado de teste precisa ser confiável para o Windows aceitar o .cat.
# certutil em vez de Import-Certificate: funciona também nas versões
# reduzidas do Windows (tiny11), onde o Import-Certificate dá acesso negado.
$cer = Join-Path $PSScriptRoot 'audioroute-teste.cer'
foreach ($store in 'Root', 'TrustedPublisher') {
  certutil -f -addstore $store $cer | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "certutil não conseguiu pôr o certificado em $store ($LASTEXITCODE)" }
}

$inf = Get-ChildItem *.inf | Select-Object -First 1
$installed = (.\devcon.exe hwids $hwid) -match [regex]::Escape($hwid)
if ($installed) {
  .\devcon.exe update $inf.FullName $hwid
} else {
  .\devcon.exe install $inf.FullName $hwid
}
if ($LASTEXITCODE -ne 0) { throw "devcon terminou com erro ($LASTEXITCODE)" }
Write-Host 'Driver instalado. Veja em Configurações > Sistema > Som.' -ForegroundColor Green

# Copia driver\out para dentro da VM, sem rede, pelo serviço de convidado do
# Hyper-V. Rode NO COMPUTADOR PRINCIPAL, como administrador:
#   .\copiar-para-vm.ps1                 (VM "vm", destino C:\audioroute-driver)
#   .\copiar-para-vm.ps1 -Vm outra-vm
param(
  [string]$Vm = 'vm',
  [string]$Destino = 'C:\audioroute-driver',
  [string]$Origem = (Join-Path $PSScriptRoot '..\out')
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Origem)) { throw "Não achei $Origem. Rode o driver\build.bat antes." }

# "Interface de Serviço de Convidado" (o nome muda com o idioma; o Id não).
$guestId = '6C09BB55-D683-4DA0-8931-C9BF705F6480'
$service = Get-VMIntegrationService -VMName $Vm | Where-Object { $_.Id -like "*$guestId" }
if (-not $service.Enabled) {
  Enable-VMIntegrationService -VMName $Vm -Name $service.Name
  Write-Host 'Serviço de convidado ativado.'
}

if ((Get-VM $Vm).State -ne 'Running') {
  Start-VM $Vm
  Write-Host 'VM ligada; esperando o Windows dela subir...'
}

# O Windows da VM precisa estar de pé com o serviço respondendo.
$limite = (Get-Date).AddMinutes(4)
while ((Get-VMIntegrationService -VMName $Vm | Where-Object { $_.Id -like "*$guestId" }).PrimaryStatusDescription -ne 'OK') {
  if ((Get-Date) -gt $limite) {
    throw 'O serviço de convidado não respondeu. Esta versão do Windows na VM pode não ter os serviços de integração do Hyper-V.'
  }
  Start-Sleep -Seconds 3
}

foreach ($f in Get-ChildItem $Origem -File) {
  Copy-VMFile -VMName $Vm -SourcePath $f.FullName -DestinationPath (Join-Path $Destino $f.Name) -FileSource Host -CreateFullPath -Force
  Write-Host "copiado: $($f.Name)"
}
Write-Host "Pronto: os arquivos estão em $Destino dentro da VM." -ForegroundColor Green

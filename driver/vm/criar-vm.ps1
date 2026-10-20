# Cria a VM de testes do driver no Hyper-V. Rode NO COMPUTADOR PRINCIPAL,
# num PowerShell como administrador.
#
# 1) Criar (com a ISO do Windows 11 baixada do site da Microsoft):
#      .\criar-vm.ps1 -Iso "E:\ISOs\Win11.iso"
#    Instale o Windows normalmente na janela que abrir.
#
# 2) Depois que o Windows estiver instalado, desligue a VM e rode:
#      .\criar-vm.ps1 -DesligarSecureBoot
#    (o Windows 11 exige Secure Boot para INSTALAR; o driver de teste
#    exige Secure Boot desligado para CARREGAR.)
param(
  [string]$Iso,
  [string]$Pasta = 'E:\Hyper-V',
  [string]$Nome = 'audioroute-teste',
  [switch]$DesligarSecureBoot
)
$ErrorActionPreference = 'Stop'

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Rode este script como administrador.'
}

if ($DesligarSecureBoot) {
  if ((Get-VM $Nome).State -ne 'Off') { throw "Desligue a VM '$Nome' antes (Desligar, não Pausar)." }
  Set-VMFirmware $Nome -EnableSecureBoot Off
  Write-Host "Secure Boot desligado em '$Nome'. Pode ligar a VM e seguir com o instalar.ps1." -ForegroundColor Green
  return
}

if (-not $Iso -or -not (Test-Path $Iso)) { throw 'Informe a ISO do Windows 11 com -Iso "caminho\arquivo.iso".' }
if (Get-VM $Nome -ErrorAction SilentlyContinue) { throw "Já existe uma VM chamada '$Nome'." }

$vhd = Join-Path $Pasta "$Nome\$Nome.vhdx"
# Geração 2 (UEFI), 8 GB de memória dinâmica, disco dinâmico de 64 GB
# (ocupa só o que for usado, uns 20 GB depois de instalar).
New-VM -Name $Nome -Generation 2 -MemoryStartupBytes 8GB -Path $Pasta -NewVHDPath $vhd -NewVHDSizeBytes 64GB -SwitchName 'Default Switch' | Out-Null
Set-VMMemory $Nome -DynamicMemoryEnabled $true -MinimumBytes 2GB -MaximumBytes 8GB
Set-VMProcessor $Nome -Count 4
Set-VM $Nome -AutomaticCheckpointsEnabled $false

# Windows 11: TPM virtual e Secure Boot (modelo Microsoft Windows) para instalar.
Set-VMKeyProtector $Nome -NewLocalKeyProtector
Enable-VMTPM $Nome
Set-VMFirmware $Nome -EnableSecureBoot On -SecureBootTemplate MicrosoftWindows

# Inicia pela ISO.
$dvd = Add-VMDvdDrive $Nome -Path $Iso -Passthru
Set-VMFirmware $Nome -FirstBootDevice $dvd

Start-VM $Nome
vmconnect.exe localhost $Nome
Write-Host "VM '$Nome' criada em $Pasta. Na janela, aperte uma tecla para iniciar pelo DVD e instale o Windows." -ForegroundColor Green

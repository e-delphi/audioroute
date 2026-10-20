# Ponte para rodar comandos DENTRO da VM a partir do computador principal,
# pelo PowerShell Direct do Hyper-V (sem rede).
#
# Rode no computador principal, como administrador, e deixe a janela aberta:
#   powershell -ExecutionPolicy Bypass -File .\ponte.ps1
#
# Ela pede o usuário e a senha da conta da VM (janela do Windows; a senha fica
# só na memória desta janela) e depois fica olhando a pasta de pedidos: cada
# arquivo .ps1 que aparecer lá roda dentro da VM, e a saída vai para um .txt
# com o mesmo nome. Fechar a janela encerra a ponte.
param(
  [string]$Vm = 'vm',
  [string]$Pasta = (Join-Path $env:LOCALAPPDATA 'audioroute\ponte-vm')
)
$ErrorActionPreference = 'Stop'

New-Item -ItemType Directory -Force $Pasta | Out-Null
$Host.UI.RawUI.WindowTitle = "Ponte para a VM '$Vm' (feche para encerrar)"

$cred = Get-Credential -Message "Usuário e senha da conta do Windows na VM '$Vm'"
$session = $null

function Abrir-Sessao {
  if ($script:session -and $script:session.State -eq 'Opened') { return $script:session }
  if ((Get-VM $Vm).State -ne 'Running') { Start-VM $Vm; Start-Sleep -Seconds 20 }
  $script:session = New-PSSession -VMName $Vm -Credential $cred
  return $script:session
}

try {
  $s = Abrir-Sessao
  $quem = Invoke-Command -Session $s { "$env:COMPUTERNAME\$env:USERNAME" }
  Write-Host "Conectado na VM como $quem. Pedidos em: $Pasta" -ForegroundColor Green
} catch {
  Write-Host "Não conectou na VM: $_" -ForegroundColor Red
  Write-Host 'Confira usuário e senha (contas sem senha não funcionam no PowerShell Direct).'
  Read-Host 'Enter para fechar'
  return
}

while ($true) {
  foreach ($pedido in Get-ChildItem $Pasta -Filter *.ps1 | Sort-Object LastWriteTime) {
    $saida = [IO.Path]::ChangeExtension($pedido.FullName, '.txt')
    Write-Host "$(Get-Date -Format HH:mm:ss) rodando $($pedido.Name)"
    try {
      $s = Abrir-Sessao
      $codigo = [ScriptBlock]::Create((Get-Content $pedido.FullName -Raw -Encoding UTF8))
      $r = Invoke-Command -Session $s -ScriptBlock $codigo 2>&1 | Out-String -Width 220
      ($r + "`n[fim]") | Out-File -Encoding utf8 $saida
    } catch {
      ("erro: $_`n[fim]") | Out-File -Encoding utf8 $saida
    }
    Remove-Item $pedido.FullName
  }
  Start-Sleep -Milliseconds 500
}

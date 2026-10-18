# Gera ..\icon.ico a partir de icon.svg: o Edge (invisível) desenha cada
# tamanho e o mkico.py junta tudo num .ico. Rode depois de editar o SVG.
$here = $PSScriptRoot
$edge = "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
$tmp = Join-Path ([IO.Path]::GetTempPath()) 'audioroute-icon'
New-Item -ItemType Directory -Force $tmp | Out-Null
$svg = (Join-Path $here 'icon.svg').Replace('\', '/')
foreach ($n in 16, 24, 32, 48, 64, 128, 256) {
  $html = Join-Path $tmp "r$n.html"
  $shot = Join-Path $tmp "i$n.png"
  "<!doctype html><html><body style='margin:0;background:transparent'><img src='file:///$svg' width='$n' height='$n' style='display:block'></body></html>" | Set-Content -Encoding utf8 $html
  # Start-Process: o Edge escreve no stderr, o que pararia o script.
  Start-Process $edge -Wait -WindowStyle Hidden -ArgumentList @(
    '--headless=new', '--disable-gpu', '--hide-scrollbars', '--default-background-color=00000000',
    '--force-device-scale-factor=1', "--window-size=$n,$n", "--screenshot=`"$shot`"", "file:///$($html.Replace('\', '/'))"
  )
}
python (Join-Path $here 'mkico.py') $tmp (Join-Path $here '..\icon.ico')
Copy-Item (Join-Path $tmp 'i256.png') (Join-Path $here 'icon-256.png')

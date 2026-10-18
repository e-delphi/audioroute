@echo off
REM ---------------------------------------------------------------------
REM  build.bat - compila o audioroute e poe o exe em release\audioroute.exe
REM
REM  A interface (app\ui: HTML, CSS e JS puros) vai embutida no exe; sem build web.
REM ---------------------------------------------------------------------
setlocal
cd /d "%~dp0"

cargo build --release -p audioroute-app
if errorlevel 1 (
  echo [build] ERRO: cargo build falhou.
  exit /b 1
)

if not exist release mkdir release
copy /y "target\release\audioroute.exe" "release\audioroute.exe" >nul
if errorlevel 1 (
  echo [build] ERRO: nao consegui copiar o exe ^(o audioroute esta aberto? Use Sair no icone da bandeja^).
  exit /b 1
)
echo [build] OK: release\audioroute.exe

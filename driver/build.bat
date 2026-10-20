@echo off
REM ---------------------------------------------------------------------
REM  build.bat - compila o driver e monta driver\out\ para copiar na VM
REM
REM  Uso:  build.bat            Debug (padrao, com verificacoes)
REM        build.bat Release
REM
REM  driver\out\ fica com: o pacote (.inf, .sys, .cat), o certificado de
REM  teste (.cer), o devcon.exe do WDK e os scripts de instalar/remover.
REM ---------------------------------------------------------------------
setlocal
cd /d "%~dp0"

set "CONFIG=Debug"
if /i "%~1"=="Release" set "CONFIG=Release"

set "MSBUILD="
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -find MSBuild\**\Bin\amd64\MSBuild.exe`) do set "MSBUILD=%%i"
if not defined MSBUILD (
  echo [driver] ERRO: MSBuild nao encontrado. Instale o Visual Studio Build Tools com C++.
  exit /b 1
)

REM Pacote antigo fora: senao sobram arquivos de versoes anteriores em out\.
if exist "x64\%CONFIG%\package" rmdir /s /q "x64\%CONFIG%\package"

"%MSBUILD%" audioroute-driver.sln /p:Configuration=%CONFIG% /p:Platform=x64 /m /nologo /v:minimal
if errorlevel 1 (
  echo [driver] ERRO: o build falhou.
  exit /b 1
)

set "DEVCON="
for /d %%v in ("%ProgramFiles(x86)%\Windows Kits\10\Tools\*") do if exist "%%v\x64\devcon.exe" set "DEVCON=%%v\x64\devcon.exe"

if exist out rmdir /s /q out
mkdir out
copy /y "x64\%CONFIG%\package\*" out\ >nul
copy /y "x64\%CONFIG%\package.cer" out\audioroute-teste.cer >nul
copy /y vm\instalar.ps1 out\ >nul
copy /y vm\remover.ps1 out\ >nul
if defined DEVCON (copy /y "%DEVCON%" out\ >nul) else (echo [driver] AVISO: devcon.exe nao encontrado no WDK.)
echo [driver] OK: driver\out\ (%CONFIG%) - copie a pasta para a VM e rode instalar.ps1 como administrador

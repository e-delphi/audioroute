# audioroute-driver

Driver de áudio do Windows que cria um **alto-falante virtual** e um **microfone virtual**:

- **Alto-falante "audioroute"**: os programas tocam nele (pode ser o padrão do Windows); o audioroute
  recebe esse áudio como uma entrada no canvas, sem o som sair em lugar nenhum.
- **Microfone "audioroute"**: o audioroute manda para ele qualquer mistura do canvas; Discord, OBS,
  Zoom etc. o enxergam como um microfone comum.

Hoje isso só é possível com drivers de terceiros (VB-CABLE, Steam Streaming). Com o nosso, o app
controla nomes, quantidade e latência, e o áudio não passa por um "cabo" extra.

## Como funciona

```
 programas ──► [alto-falante virtual] ──► buffer do driver ──► audioroute (entrada no canvas)
 audioroute (saída no canvas) ──► buffer do driver ──► [microfone virtual] ──► Discord, OBS...
```

- Driver de kernel **WaveRT/PortCls** em C++, partindo do exemplo oficial da Microsoft
  (`SimpleAudioSample`, licença MS-PL), o mesmo modelo de drivers de áudio virtuais conhecidos.
- O áudio entre o driver e o app passa por memória compartilhada (anel), com um controle por IOCTL
  para o audioroute abrir, ler e escrever. Formato fixo: 48 kHz, estéreo, f32 — o mesmo do motor.
- Sem o app aberto, o alto-falante descarta o som e o microfone entrega silêncio.

## Etapas

| | Etapa | Resultado |
|---|---|---|
| 0 ✅ | Ambiente: WDK, bibliotecas Spectre do C++, máquina de teste | compila e instala um driver de exemplo |
| 1 ✅ | Exemplo da Microsoft compilando sem mudanças | aparece um alto-falante e um microfone de teste |
| 2 | Nomes, IDs e INF próprios ("audioroute") | dispositivos com a nossa identidade |
| 3 | Cabo interno: o que toca no alto-falante sai no microfone | já substitui o VB-CABLE |
| 4 | Interface com o app (memória compartilhada + IOCTL) | os virtuais aparecem no canvas do audioroute |
| 5 | Vários pares, nomes configuráveis pelo app | "Microfone para o Discord", "Saída do jogo"… |
| 6 | Assinatura para distribuir (certificado EV + atestação da Microsoft) | instala em qualquer Windows sem modo de teste |

## Testes: por que numa máquina virtual

Driver de kernel com erro **trava o Windows inteiro (tela azul)**. Além disso, até a etapa 6 o
driver não tem assinatura aceita pelo Windows, então só carrega com o **modo de teste** ligado
(`bcdedit /set testsigning on`), que exige **Secure Boot desligado**. Por isso os testes vão numa
VM do Hyper-V; o computador principal não muda.

## Testar na VM

1. `build.bat` (Debug) monta `out\`.
2. No computador principal, como administrador: `vm\copiar-para-vm.ps1` (copia `out\` para
   `C:\audioroute-driver` na VM, sem rede).
3. Na VM, como administrador: `C:\audioroute-driver\instalar.ps1`. Na 1ª vez liga o modo de teste e
   pede para reiniciar; na 2ª instala. `remover.ps1` desinstala.

`vm\criar-vm.ps1` cria uma VM do zero (Windows 11, Geração 2, TPM) e desliga o Secure Boot depois.

## Requisitos para compilar

- Visual Studio 2022 (Build Tools servem) com C++ e as bibliotecas com mitigação Spectre
- Windows SDK e **WDK** 10.0.26100 (mesma versão)

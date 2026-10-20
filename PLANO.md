# audioroute — Plano

Roteador de áudio com **latência de estúdio**: cada computador (e futuramente Android) roda o app
como um **nó**, compartilhando seus microfones e alto-falantes. Qualquer entrada pode ser ligada a
qualquer saída, num canvas com cabos virtuais, com mixagem, ganho, mute, atraso e compensação de
drift. Compilar: `build.bat` → `release\audioroute.exe`.

## Histórico das decisões

- **Go → Rust (out/2026).** Começou em Go + miniaudio + Wails. Com o objetivo de latência de
  estúdio, foi refeito em Rust + Tauri com WASAPI direta. No headset GK71 (exclusive 5 ms,
  ida e volta acústica): Go+miniaudio ~57 ms; **Go com WASAPI direta ~35 ms; Rust ~36 ms**. A
  diferença vinha do miniaudio, não da linguagem. Escolhido Rust pelo COM tipado e por não ter
  runtime/GC no caminho do áudio. Versão Go removida.
- **Benchmark de mixagem** (Go, Delphi 64, Bun/TS): todos abaixo de 0,1% do orçamento de CPU;
  mixagem nunca é o gargalo.
- **Exclusive 5 ms por padrão:** o mínimo anunciado pelo driver do GK71 (3 ms) derrubou o
  dispositivo; 5 ms foi estável.
- **Cabos virtuais:** "Steam Streaming Speakers" + loopback serve de alto-falante virtual
  (~12 ms, estável). O par "Steam Streaming Microphone" passa áudio, mas é instável (34 ms ± 21 ms
  em shared; perde áudio em exclusive com buffer curto). Para microfone virtual sério: VB-CABLE ou
  driver próprio (projeto à parte).

## Decisões atuais

| Tema | Decisão |
|---|---|
| Linguagem | Rust (workspace Cargo); interface em HTML, CSS e JS puros (sem build) num WebView2 com janela Win32 feita à mão (sem Tauri, como no NoOBS): interface embutida no exe, ponte por `postMessage`, ícone na bandeja |
| Áudio | WASAPI direta pelo crate oficial `windows`: exclusive, shared de baixa latência (IAudioClient3) e shared convertido (loopback) |
| Formato interno | f32 intercalado, 48 kHz, estéreo |
| Tempo real | Thread de áudio sem lock, sem alocação e sem liberação de memória: comandos por filas lock-free (`rtrb`), lixo devolvido à thread de controle |
| Identificação | **GUID determinístico** (UUID v5 de MachineGuid + papel + ID do endpoint); religação por nome quando o Windows recria o endpoint |
| Som do Windows | Nó fixo "Windows" no canvas com um cabo não removível até a saída padrão; arrastar a ponta troca o padrão do sistema (`IPolicyConfig`, sem driver, sem latência extra). Se precisar de várias saídas ou exclusive: passar por loopback de um dispositivo sem uso. Driver próprio fica só para microfone virtual |
| Topologia (rede) | Mestre fixo de config e de relógio + participantes |
| Rede | Só LAN · descoberta UDP multicast · controle TCP (HTTP/WebSocket) · áudio UDP com timestamp e jitter buffer · PCM 16 bits, depois Opus |

## Estrutura

```
crates/core      tipos, identidade, config, motor (rotas, mix, drift, reamostrador)
crates/wasapi    backend Windows: enumeração, streams, medidor do sistema, sondagem
crates/control   fachada usada pela interface
crates/net       rede: descoberta, controle TCP, áudio UDP; NetBackend junta local + remoto
crates/cli       audioroute-cli --list / --probe / --measure / config.json
app/ui           interface: HTML, CSS e JS puros, embutidos no exe
app/host         janela Win32 + WebView2, bandeja, ponte JSON (exe audioroute)
release/         audioroute.exe (app final, sem instalador)
driver/          alto-falante e microfone virtuais (WDK, C++); plano em driver/README.md
```

## config.json

```json
{
  "bufferMs": 30,
  "devices": [
    { "id": "…", "name": "Fones de ouvido (GK71)", "description": "", "delayMs": 0,
      "mode": "exclusive", "periodMs": 5 }
  ],
  "routes": [ { "from": "…", "to": "…", "gain": 0.8, "mute": false } ]
}
```

`mode` (padrão shared) e `periodMs` (padrão do modo) são omitidos quando não usados.

## Etapas

1. ✅ Identidade e `--list` (GUIDs iguais aos da versão Go)
2. ✅ Config com serde e validação
3. ✅ Streams WASAPI e motor sem alocação na thread de áudio
4. ✅ Drift (controlador PI) e reamostrador
5. ✅ App desktop com canvas: modo e período por dispositivo, medidores do sistema, religação automática
6. ✅ Descoberta: o controlador se anuncia por UDP (multicast 239.255.77.77 e broadcast, porta 47800); o participante conecta sozinho
7. ✅ Controle por TCP (porta 47801, uma linha JSON por mensagem): o participante informa os dispositivos; o controlador pede para capturar/tocar
8. Relógio comum com o mestre — por enquanto cada cabo compensa o drift pelo nível do buffer (suficiente para 1 saída por cabo; para tocar em várias salas exatamente juntas ainda falta)
9. ✅ Áudio por UDP: PCM 16 bits, 5 ms por pacote, posição do frame no cabeçalho (perda vira silêncio, atraso é descartado); o buffer do cabo faz de jitter buffer (+20 ms de folga)
10. Opus, autenticação e criptografia
11. Android

## Riscos conhecidos

- **Drivers USB** nem sempre aguentam o período mínimo que anunciam → período configurável, 5 ms padrão
- **Endpoint recriado pelo Windows** muda o GUID → religação por nome (feita)
- **Exclusive** tira o dispositivo dos outros programas → cabo virtual para o som do Windows
- **Latência de rede**: LAN cabeada 30–50 ms; Wi-Fi 60–150 ms (se picotar, aumentar `bufferMs`)
- **Firewall do Windows** pergunta na 1ª vez que o app usa a rede: precisa permitir em redes privadas, nos dois computadores
- **Sem autenticação ainda** (etapa 10): qualquer audioroute da mesma rede local consegue participar

/*++

Module Name:

    loopback.h

Abstract:

    Anéis de áudio do audioroute, um conjunto por par (48 kHz, estéreo,
    inteiro de 32 bits):

    - anel do alto-falante N: o alto-falante N escreve; o microfone N (cabo
      interno) e o app leem;
    - anel do microfone N: o app escreve; o microfone N lê.

    O microfone N lê do anel do microfone N depois que o app escreveu nele
    (com o app conectado); senão, do anel do alto-falante N (cabo interno).

    Tudo anda pelo mesmo relógio (o timer do sistema), então não há drift
    entre quem escreve e quem lê: só uma folga pequena para os ticks não se
    atropelarem.

--*/

#ifndef _AUDIOROUTE_LOOPBACK_H_
#define _AUDIOROUTE_LOOPBACK_H_

// Posição de quem lê um anel.
typedef struct _LOOPBACK_READER
{
    ULONGLONG   Position;   // próximo frame a ler (contagem desde o início)
    PVOID       Source;     // anel de onde lê (se mudar, ressincroniza)
    BOOLEAN     Started;    // já sincronizou com quem escreve
    BOOLEAN     Priming;    // esperando juntar a folga antes de entregar som
} LOOPBACK_READER, *PLOOPBACK_READER;

// Alto-falante do par → anel. Data: PCM 16 bits, estéreo.
VOID LoopbackWriteRender16(_In_ ULONG Pair, _In_reads_bytes_(Bytes) const BYTE* Data, _In_ ULONG Bytes);

// Microfone do par ← anel do microfone (app) ou do alto-falante (cabo
// interno). Data: PCM 32 bits, estéreo. Sem som: silêncio.
VOID LoopbackReadCapture32(_In_ ULONG Pair, _Inout_ PLOOPBACK_READER Reader, _Out_writes_bytes_(Bytes) BYTE* Data, _In_ ULONG Bytes);

// App (via \\.\audioroute, ver control.cpp). Pair fora da faixa: ignorado.
VOID LoopbackAppConnected(_In_ BOOLEAN Connected);
VOID LoopbackAppReadSpeaker(_In_ ULONG Pair, _Out_writes_bytes_(Bytes) BYTE* Data, _In_ ULONG Bytes);
VOID LoopbackAppWriteMic(_In_ ULONG Pair, _In_reads_bytes_(Bytes) const BYTE* Data, _In_ ULONG Bytes);

#endif // _AUDIOROUTE_LOOPBACK_H_

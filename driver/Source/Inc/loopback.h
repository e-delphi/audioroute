/*++

Module Name:

    loopback.h

Abstract:

    Cabo interno do audioroute: o que toca no alto-falante virtual sai no
    microfone virtual.

    O alto-falante escreve num anel de memória (48 kHz, estéreo, inteiro de
    32 bits); cada stream do microfone lê dele com a sua própria posição.
    Os dois andam pelo mesmo relógio (o timer do sistema), então não há
    drift entre eles: só é preciso uma folga pequena para os ticks não se
    atropelarem.

--*/

#ifndef _AUDIOROUTE_LOOPBACK_H_
#define _AUDIOROUTE_LOOPBACK_H_

// Posição de leitura de um stream do microfone.
typedef struct _LOOPBACK_READER
{
    ULONGLONG   Position;   // próximo frame a ler (contagem desde o início)
    BOOLEAN     Started;    // já sincronizou com o alto-falante
    BOOLEAN     Priming;    // esperando juntar a folga antes de tocar
} LOOPBACK_READER, *PLOOPBACK_READER;

// Alto-falante → anel. Data: PCM 16 bits, estéreo.
VOID LoopbackWriteRender16(_In_reads_bytes_(Bytes) const BYTE* Data, _In_ ULONG Bytes);

// Anel → microfone. Data: PCM 32 bits, estéreo. Sem som no anel: silêncio.
VOID LoopbackReadCapture32(_Inout_ PLOOPBACK_READER Reader, _Out_writes_bytes_(Bytes) BYTE* Data, _In_ ULONG Bytes);

#endif // _AUDIOROUTE_LOOPBACK_H_

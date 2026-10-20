/*++

Module Name:

    loopback.cpp

Abstract:

    Cabo interno do audioroute (ver loopback.h).

    Roda em DISPATCH_LEVEL (chamado pelo timer dos streams): sem código
    paginável, sem alocação; o anel é estático, em memória não paginada.

--*/

#include <definitions.h>
#include "loopback.h"

// ~0,68 s de áudio. Potência de 2 para o índice ser só uma máscara.
#define LOOPBACK_FRAMES     32768
#define LOOPBACK_MASK       (LOOPBACK_FRAMES - 1)
#define LOOPBACK_CHANNELS   2
// Folga entre o alto-falante e o microfone: 10 ms a 48 kHz.
#define LOOPBACK_LATENCY    480

static INT32        g_Ring[LOOPBACK_FRAMES * LOOPBACK_CHANNELS];
static ULONGLONG    g_Written;      // frames escritos desde que o driver carregou
static KSPIN_LOCK   g_Lock;         // zero é um spin lock válido (KeInitializeSpinLock só zera)

#pragma code_seg()
VOID LoopbackWriteRender16(_In_reads_bytes_(Bytes) const BYTE* Data, _In_ ULONG Bytes)
{
    const INT16* samples = (const INT16*)Data;
    ULONG frames = Bytes / (sizeof(INT16) * LOOPBACK_CHANNELS);
    KIRQL irql;

    KeAcquireSpinLock(&g_Lock, &irql);
    for (ULONG i = 0; i < frames; i++)
    {
        ULONG slot = (ULONG)((g_Written + i) & LOOPBACK_MASK) * LOOPBACK_CHANNELS;
        // 16 → 32 bits: mesma escala, só desloca.
        g_Ring[slot]     = (INT32)samples[2 * i] * 65536;
        g_Ring[slot + 1] = (INT32)samples[2 * i + 1] * 65536;
    }
    g_Written += frames;
    KeReleaseSpinLock(&g_Lock, irql);
}

#pragma code_seg()
VOID LoopbackReadCapture32(_Inout_ PLOOPBACK_READER Reader, _Out_writes_bytes_(Bytes) BYTE* Data, _In_ ULONG Bytes)
{
    INT32* out = (INT32*)Data;
    ULONG frames = Bytes / (sizeof(INT32) * LOOPBACK_CHANNELS);
    KIRQL irql;

    KeAcquireSpinLock(&g_Lock, &irql);
    ULONGLONG written = g_Written;

    // Primeira leitura, ou o leitor ficou para trás mais do que o anel guarda:
    // recomeça do ponto atual do alto-falante.
    if (!Reader->Started || Reader->Position > written || written - Reader->Position > LOOPBACK_FRAMES - frames)
    {
        Reader->Position = written;
        Reader->Started = TRUE;
        Reader->Priming = TRUE;
    }
    // Só começa a entregar som depois de juntar a folga.
    if (Reader->Priming && written - Reader->Position >= LOOPBACK_LATENCY)
    {
        Reader->Priming = FALSE;
    }

    for (ULONG i = 0; i < frames; i++)
    {
        if (!Reader->Priming && Reader->Position < written)
        {
            ULONG slot = (ULONG)(Reader->Position & LOOPBACK_MASK) * LOOPBACK_CHANNELS;
            out[2 * i]     = g_Ring[slot];
            out[2 * i + 1] = g_Ring[slot + 1];
            Reader->Position++;
        }
        else
        {
            // Nada tocando no alto-falante (ou acabou a folga): silêncio, e
            // espera juntar a folga de novo antes de voltar a tocar.
            out[2 * i]     = 0;
            out[2 * i + 1] = 0;
            Reader->Priming = TRUE;
        }
    }
    KeReleaseSpinLock(&g_Lock, irql);
}

/*++

Module Name:

    loopback.cpp

Abstract:

    Anéis de áudio do audioroute (ver loopback.h).

    Roda em DISPATCH_LEVEL (chamado pelo timer dos streams): sem código
    paginável, sem alocação; os anéis são estáticos, em memória não paginada.

--*/

#include <definitions.h>
#include "loopback.h"

// ~0,68 s de áudio por anel. Potência de 2 para o índice ser só uma máscara.
#define LOOPBACK_FRAMES     32768
#define LOOPBACK_MASK       (LOOPBACK_FRAMES - 1)
#define LOOPBACK_CHANNELS   2
// Folga entre quem escreve e quem lê: 10 ms a 48 kHz.
#define LOOPBACK_LATENCY    480

typedef struct _RING
{
    INT32       Data[LOOPBACK_FRAMES * LOOPBACK_CHANNELS];
    ULONGLONG   Written;    // frames escritos desde que o driver carregou
    KSPIN_LOCK  Lock;       // zero é um spin lock válido (KeInitializeSpinLock só zera)
} RING, *PRING;

static RING             g_Speaker;          // alto-falante virtual → (microfone, app)
static RING             g_Mic;              // app → microfone virtual
static volatile LONG    g_AppConnected;     // app com \\.\audioroute aberto
static LOOPBACK_READER  g_AppReader;        // posição do app no anel do alto-falante

#pragma code_seg()
static VOID RingWrite32(_Inout_ PRING Ring, _In_reads_(Frames * LOOPBACK_CHANNELS) const INT32* Samples, _In_ ULONG Frames)
{
    KIRQL irql;
    KeAcquireSpinLock(&Ring->Lock, &irql);
    for (ULONG i = 0; i < Frames; i++)
    {
        ULONG slot = (ULONG)((Ring->Written + i) & LOOPBACK_MASK) * LOOPBACK_CHANNELS;
        Ring->Data[slot]     = Samples[2 * i];
        Ring->Data[slot + 1] = Samples[2 * i + 1];
    }
    Ring->Written += Frames;
    KeReleaseSpinLock(&Ring->Lock, irql);
}

#pragma code_seg()
static VOID RingRead32(_Inout_ PRING Ring, _Inout_ PLOOPBACK_READER Reader, _Out_writes_(Frames * LOOPBACK_CHANNELS) INT32* Out, _In_ ULONG Frames)
{
    KIRQL irql;
    KeAcquireSpinLock(&Ring->Lock, &irql);
    ULONGLONG written = Ring->Written;

    // Primeira leitura, troca de anel, ou ficou para trás mais do que o anel
    // guarda: recomeça do ponto atual de quem escreve.
    if (!Reader->Started || Reader->Source != Ring || Reader->Position > written ||
        written - Reader->Position > LOOPBACK_FRAMES - Frames)
    {
        Reader->Position = written;
        Reader->Source = Ring;
        Reader->Started = TRUE;
        Reader->Priming = TRUE;
    }
    // Só começa a entregar som depois de juntar a folga.
    if (Reader->Priming && written - Reader->Position >= LOOPBACK_LATENCY)
    {
        Reader->Priming = FALSE;
    }

    for (ULONG i = 0; i < Frames; i++)
    {
        if (!Reader->Priming && Reader->Position < written)
        {
            ULONG slot = (ULONG)(Reader->Position & LOOPBACK_MASK) * LOOPBACK_CHANNELS;
            Out[2 * i]     = Ring->Data[slot];
            Out[2 * i + 1] = Ring->Data[slot + 1];
            Reader->Position++;
        }
        else
        {
            // Quem escreve parou (ou acabou a folga): silêncio, e espera
            // juntar a folga de novo antes de voltar a entregar som.
            Out[2 * i]     = 0;
            Out[2 * i + 1] = 0;
            Reader->Priming = TRUE;
        }
    }
    KeReleaseSpinLock(&Ring->Lock, irql);
}

#pragma code_seg()
VOID LoopbackWriteRender16(_In_reads_bytes_(Bytes) const BYTE* Data, _In_ ULONG Bytes)
{
    const INT16* in = (const INT16*)Data;
    ULONG frames = Bytes / (sizeof(INT16) * LOOPBACK_CHANNELS);
    INT32 chunk[256 * LOOPBACK_CHANNELS];

    // 16 → 32 bits (mesma escala), em blocos para não usar muita pilha.
    while (frames > 0)
    {
        ULONG n = min(frames, 256UL);
        for (ULONG i = 0; i < n * LOOPBACK_CHANNELS; i++)
        {
            chunk[i] = (INT32)in[i] * 65536;
        }
        RingWrite32(&g_Speaker, chunk, n);
        in += n * LOOPBACK_CHANNELS;
        frames -= n;
    }
}

#pragma code_seg()
VOID LoopbackReadCapture32(_Inout_ PLOOPBACK_READER Reader, _Out_writes_bytes_(Bytes) BYTE* Data, _In_ ULONG Bytes)
{
    PRING source = g_AppConnected ? &g_Mic : &g_Speaker;
    RingRead32(source, Reader, (INT32*)Data, Bytes / (sizeof(INT32) * LOOPBACK_CHANNELS));
}

#pragma code_seg()
VOID LoopbackAppConnected(_In_ BOOLEAN Connected)
{
    KIRQL irql;
    KeAcquireSpinLock(&g_Speaker.Lock, &irql);
    g_AppReader.Started = FALSE;    // o app começa do ponto atual
    KeReleaseSpinLock(&g_Speaker.Lock, irql);
    InterlockedExchange(&g_AppConnected, Connected ? 1 : 0);
}

#pragma code_seg()
VOID LoopbackAppReadSpeaker(_Out_writes_bytes_(Bytes) BYTE* Data, _In_ ULONG Bytes)
{
    RingRead32(&g_Speaker, &g_AppReader, (INT32*)Data, Bytes / (sizeof(INT32) * LOOPBACK_CHANNELS));
}

#pragma code_seg()
VOID LoopbackAppWriteMic(_In_reads_bytes_(Bytes) const BYTE* Data, _In_ ULONG Bytes)
{
    RingWrite32(&g_Mic, (const INT32*)Data, Bytes / (sizeof(INT32) * LOOPBACK_CHANNELS));
}

/*++

Module Name:

    loopback.cpp

Abstract:

    Anéis de áudio do audioroute, um conjunto por par (ver loopback.h).

    Roda em DISPATCH_LEVEL (chamado pelo timer dos streams): sem código
    paginável, sem alocação; os anéis são estáticos, em memória não paginada
    (AUDIOROUTE_PAIRS × 2 anéis de 128 KB).

--*/

#include <definitions.h>
#include "loopback.h"
#include "audioroute_ioctl.h"

// ~0,34 s de áudio por anel. Potência de 2 para o índice ser só uma máscara.
#define LOOPBACK_FRAMES     16384
#define LOOPBACK_MASK       (LOOPBACK_FRAMES - 1)
#define LOOPBACK_CHANNELS   2
// Folga entre quem escreve e quem lê: 4 ms a 48 kHz. Os anéis andam de
// 1 em 1 ms (timer dos streams) e o app troca blocos de 2 ms.
#define LOOPBACK_LATENCY    192

typedef struct _RING
{
    INT32       Data[LOOPBACK_FRAMES * LOOPBACK_CHANNELS];
    ULONGLONG   Written;    // frames escritos desde que o driver carregou
    KSPIN_LOCK  Lock;       // zero é um spin lock válido (KeInitializeSpinLock só zera)
} RING, *PRING;

typedef struct _PAIR
{
    RING            Speaker;    // alto-falante → (microfone, app)
    RING            Mic;        // app → microfone
    LOOPBACK_READER AppReader;  // posição do app no anel do alto-falante
    volatile LONG   AppMic;     // o app já escreveu no microfone deste par
} PAIR, *PPAIR;

static PAIR             g_Pairs[AUDIOROUTE_PAIRS];
static volatile LONG    g_AppConnected;     // app com \\.\audioroute aberto

#pragma code_seg()
static PPAIR GetPair(_In_ ULONG Pair)
{
    return Pair < AUDIOROUTE_PAIRS ? &g_Pairs[Pair] : NULL;
}

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
VOID LoopbackWriteRender16(_In_ ULONG Pair, _In_reads_bytes_(Bytes) const BYTE* Data, _In_ ULONG Bytes)
{
    PPAIR pair = GetPair(Pair);
    if (pair == NULL)
    {
        return;
    }
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
        RingWrite32(&pair->Speaker, chunk, n);
        in += n * LOOPBACK_CHANNELS;
        frames -= n;
    }
}

#pragma code_seg()
VOID LoopbackReadCapture32(_In_ ULONG Pair, _Inout_ PLOOPBACK_READER Reader, _Out_writes_bytes_(Bytes) BYTE* Data, _In_ ULONG Bytes)
{
    PPAIR pair = GetPair(Pair);
    if (pair == NULL)
    {
        RtlZeroMemory(Data, Bytes);
        return;
    }
    PRING source = (g_AppConnected && pair->AppMic) ? &pair->Mic : &pair->Speaker;
    RingRead32(source, Reader, (INT32*)Data, Bytes / (sizeof(INT32) * LOOPBACK_CHANNELS));
}

#pragma code_seg()
VOID LoopbackAppConnected(_In_ BOOLEAN Connected)
{
    for (ULONG i = 0; i < AUDIOROUTE_PAIRS; i++)
    {
        KIRQL irql;
        KeAcquireSpinLock(&g_Pairs[i].Speaker.Lock, &irql);
        g_Pairs[i].AppReader.Started = FALSE;   // o app começa do ponto atual
        KeReleaseSpinLock(&g_Pairs[i].Speaker.Lock, irql);
        InterlockedExchange(&g_Pairs[i].AppMic, 0);
    }
    InterlockedExchange(&g_AppConnected, Connected ? 1 : 0);
}

#pragma code_seg()
VOID LoopbackAppReadSpeaker(_In_ ULONG Pair, _Out_writes_bytes_(Bytes) BYTE* Data, _In_ ULONG Bytes)
{
    PPAIR pair = GetPair(Pair);
    if (pair == NULL)
    {
        RtlZeroMemory(Data, Bytes);
        return;
    }
    RingRead32(&pair->Speaker, &pair->AppReader, (INT32*)Data, Bytes / (sizeof(INT32) * LOOPBACK_CHANNELS));
}

#pragma code_seg()
VOID LoopbackAppWriteMic(_In_ ULONG Pair, _In_reads_bytes_(Bytes) const BYTE* Data, _In_ ULONG Bytes)
{
    PPAIR pair = GetPair(Pair);
    if (pair == NULL)
    {
        return;
    }
    RingWrite32(&pair->Mic, (const INT32*)Data, Bytes / (sizeof(INT32) * LOOPBACK_CHANNELS));
    InterlockedExchange(&pair->AppMic, 1);
}

/*++

Module Name:

    audioroute_ioctl.h

Abstract:

    Contrato entre o driver e o app audioroute (user mode). O app abre
    \\.\audioroute e troca áudio com os pares virtuais (alto-falante N e
    microfone N, N = 0..AUDIOROUTE_PAIRS-1):

    - READ_SPEAKER: o que os programas tocaram no alto-falante do par;
    - WRITE_MIC: o que deve sair no microfone do par.

    Formato fixo nos dois sentidos: 48 kHz, estéreo, PCM inteiro de 32 bits
    intercalado (8 bytes por frame).

    O microfone de um par toca o que o app escreve nele a partir da primeira
    escrita; antes disso (e depois que o app fecha), toca o que o
    alto-falante do mesmo par recebe (cabo interno).

    Mudou algo aqui: suba AUDIOROUTE_IOCTL_VERSION e ajuste o app
    (crates/virtual/src/driver.rs).

--*/

#ifndef _AUDIOROUTE_IOCTL_H_
#define _AUDIOROUTE_IOCTL_H_

#define AUDIOROUTE_IOCTL_VERSION        2

#define AUDIOROUTE_PAIRS                4
#define AUDIOROUTE_SAMPLE_RATE          48000
#define AUDIOROUTE_CHANNELS             2
#define AUDIOROUTE_BITS_PER_SAMPLE      32

// Tipo de dispositivo próprio (faixa 0x8000+ é livre para terceiros).
#define FILE_DEVICE_AUDIOROUTE          0x8A55

// Saída: AUDIOROUTE_INFO.                                     = 0x8A552000
#define IOCTL_AUDIOROUTE_GET_INFO \
    CTL_CODE(FILE_DEVICE_AUDIOROUTE, 0x800, METHOD_BUFFERED, FILE_ANY_ACCESS)

// Entrada: ULONG com o par. Saída: frames do alto-falante do par (o tamanho
// pedido; silêncio se nada toca).                            = 0x8A556004
#define IOCTL_AUDIOROUTE_READ_SPEAKER \
    CTL_CODE(FILE_DEVICE_AUDIOROUTE, 0x801, METHOD_BUFFERED, FILE_READ_ACCESS)

// Entrada: AUDIOROUTE_MIC_HEADER seguido dos frames para o microfone do par.
//                                                             = 0x8A55A008
#define IOCTL_AUDIOROUTE_WRITE_MIC \
    CTL_CODE(FILE_DEVICE_AUDIOROUTE, 0x802, METHOD_BUFFERED, FILE_WRITE_ACCESS)

typedef struct _AUDIOROUTE_INFO
{
    ULONG   Version;        // AUDIOROUTE_IOCTL_VERSION
    ULONG   SampleRate;     // 48000
    ULONG   Channels;       // 2
    ULONG   BitsPerSample;  // 32
    ULONG   Pairs;          // AUDIOROUTE_PAIRS
} AUDIOROUTE_INFO, *PAUDIOROUTE_INFO;

typedef struct _AUDIOROUTE_MIC_HEADER
{
    ULONG   Pair;
    ULONG   Reserved;       // 0 (alinha os frames em 8 bytes)
} AUDIOROUTE_MIC_HEADER, *PAUDIOROUTE_MIC_HEADER;

#endif // _AUDIOROUTE_IOCTL_H_

/*++

Module Name:

    audioroute_ioctl.h

Abstract:

    Contrato entre o driver e o app audioroute (user mode). O app abre
    \\.\audioroute e troca áudio com os dispositivos virtuais:

    - READ_SPEAKER: o que os programas tocaram no "Alto-falantes (audioroute)";
    - WRITE_MIC: o que deve sair no "Microfone (audioroute)".

    Formato fixo nos dois sentidos: 48 kHz, estéreo, PCM inteiro de 32 bits
    intercalado (8 bytes por frame). Com o app conectado, o microfone toca o
    que ele escreve; sem o app, o microfone volta ao cabo interno (o que toca
    no alto-falante).

    Mudou algo aqui: suba AUDIOROUTE_IOCTL_VERSION e ajuste o app
    (crates/virtual).

--*/

#ifndef _AUDIOROUTE_IOCTL_H_
#define _AUDIOROUTE_IOCTL_H_

#define AUDIOROUTE_IOCTL_VERSION        1

#define AUDIOROUTE_SAMPLE_RATE          48000
#define AUDIOROUTE_CHANNELS             2
#define AUDIOROUTE_BITS_PER_SAMPLE      32

// Tipo de dispositivo próprio (faixa 0x8000+ é livre para terceiros).
#define FILE_DEVICE_AUDIOROUTE          0x8A55

// Saída: AUDIOROUTE_INFO.                                     = 0x8A552000
#define IOCTL_AUDIOROUTE_GET_INFO \
    CTL_CODE(FILE_DEVICE_AUDIOROUTE, 0x800, METHOD_BUFFERED, FILE_ANY_ACCESS)

// Saída: frames do alto-falante (o tamanho pedido; silêncio se nada toca).
//                                                             = 0x8A556004
#define IOCTL_AUDIOROUTE_READ_SPEAKER \
    CTL_CODE(FILE_DEVICE_AUDIOROUTE, 0x801, METHOD_BUFFERED, FILE_READ_ACCESS)

// Entrada: frames para o microfone.                           = 0x8A55A008
#define IOCTL_AUDIOROUTE_WRITE_MIC \
    CTL_CODE(FILE_DEVICE_AUDIOROUTE, 0x802, METHOD_BUFFERED, FILE_WRITE_ACCESS)

typedef struct _AUDIOROUTE_INFO
{
    ULONG   Version;        // AUDIOROUTE_IOCTL_VERSION
    ULONG   SampleRate;     // 48000
    ULONG   Channels;       // 2
    ULONG   BitsPerSample;  // 32
} AUDIOROUTE_INFO, *PAUDIOROUTE_INFO;

#endif // _AUDIOROUTE_IOCTL_H_

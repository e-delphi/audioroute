/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Module Name:

    minipairs.h

Abstract:

    Local audio endpoint filter definitions. 
--*/

#ifndef _SIMPLEAUDIOSAMPLE_MINIPAIRS_H_
#define _SIMPLEAUDIOSAMPLE_MINIPAIRS_H_

#include "speakertopo.h"
#include "speakertoptable.h"
#include "speakerwavtable.h"

#include "micarraytopo.h"
#include "micarray1toptable.h"
#include "micarraywavtable.h"


NTSTATUS
CreateMiniportWaveRTSimpleAudioSample
( 
    _Out_       PUNKNOWN *,
    _In_        REFCLSID,
    _In_opt_    PUNKNOWN,
    _In_        POOL_FLAGS,
    _In_        PUNKNOWN,
    _In_opt_    PVOID,
    _In_        PENDPOINT_MINIPAIR
);

NTSTATUS
CreateMiniportTopologySimpleAudioSample
( 
    _Out_       PUNKNOWN *,
    _In_        REFCLSID,
    _In_opt_    PUNKNOWN,
    _In_        POOL_FLAGS,
    _In_        PUNKNOWN,
    _In_opt_    PVOID,
    _In_        PENDPOINT_MINIPAIR
);

//
// Render miniports.
//

/*********************************************************************
* Topology/Wave bridge connection for speaker (internal)             *
*                                                                    *
*              +------+                +------+                      *
*              | Wave |                | Topo |                      *
*              |      |                |      |                      *
* System   --->|0    1|--------------->|0    1|---> Line Out         *
*              |      |                |      |                      *
*              +------+                +------+                      *
*********************************************************************/
static
PHYSICALCONNECTIONTABLE SpeakerTopologyPhysicalConnections[] =
{
    {
        KSPIN_TOPO_WAVEOUT_SOURCE,  // TopologyIn
        KSPIN_WAVE_RENDER3_SOURCE,   // WaveOut
        CONNECTIONTYPE_WAVE_OUTPUT
    }
};

// audioroute: um par por número. Os nomes batem com KSNAME_* do audioroute.inx.
#define AUDIOROUTE_SPEAKER_PAIR(n, topo, wave)                                            \
static                                                                          \
ENDPOINT_MINIPAIR SpeakerMiniports##n =                                         \
{                                                                               \
    eSpeakerDevice,                                                             \
    topo,                                                                       \
    NULL,                                                                       \
    CreateMiniportTopologySimpleAudioSample,                                    \
    &SpeakerTopoMiniportFilterDescriptor,                                       \
    0, NULL,                                                                    \
    wave,                                                                       \
    NULL,                                                                       \
    CreateMiniportWaveRTSimpleAudioSample,                                      \
    &SpeakerWaveMiniportFilterDescriptor,                                       \
    0,                                                                          \
    NULL,                                                                       \
    SPEAKER_DEVICE_MAX_CHANNELS,                                                \
    SpeakerPinDeviceFormatsAndModes,                                            \
    SIZEOF_ARRAY(SpeakerPinDeviceFormatsAndModes),                              \
    SpeakerTopologyPhysicalConnections,                                         \
    SIZEOF_ARRAY(SpeakerTopologyPhysicalConnections),                           \
    ENDPOINT_NO_FLAGS,                                                          \
    (n) - 1,                                                                    \
};

AUDIOROUTE_SPEAKER_PAIR(1, L"TopologySpeaker1", L"WaveSpeaker1")
AUDIOROUTE_SPEAKER_PAIR(2, L"TopologySpeaker2", L"WaveSpeaker2")
AUDIOROUTE_SPEAKER_PAIR(3, L"TopologySpeaker3", L"WaveSpeaker3")
AUDIOROUTE_SPEAKER_PAIR(4, L"TopologySpeaker4", L"WaveSpeaker4")

//
// Capture miniports.
//

/*********************************************************************
* Topology/Wave bridge connection for mic array  1 (front)           *
*                                                                    *
*              +------+    +------+                                  *
*              | Topo |    | Wave |                                  *
*              |      |    |      |                                  *
*  Mic in  --->|0    1|===>|0    1|---> Capture Host Pin             *
*              |      |    |      |                                  *
*              +------+    +------+                                  *
*********************************************************************/
static
PHYSICALCONNECTIONTABLE MicArray1TopologyPhysicalConnections[] =
{
    {
        KSPIN_TOPO_BRIDGE,          // TopologyOut
        KSPIN_WAVE_BRIDGE,          // WaveIn
        CONNECTIONTYPE_TOPOLOGY_OUTPUT
    }
};

#define AUDIOROUTE_MIC_PAIR(n, topo, wave)                                                \
static                                                                          \
ENDPOINT_MINIPAIR MicMiniports##n =                                             \
{                                                                               \
    eMicArrayDevice1,                                                           \
    topo,                                                                       \
    NULL,                                                                       \
    CreateMicArrayMiniportTopology,                                             \
    &MicArray1TopoMiniportFilterDescriptor,                                     \
    0, NULL,                                                                    \
    wave,                                                                       \
    NULL,                                                                       \
    CreateMiniportWaveRTSimpleAudioSample,                                      \
    &MicArrayWaveMiniportFilterDescriptor,                                      \
    0,                                                                          \
    NULL,                                                                       \
    MICARRAY_DEVICE_MAX_CHANNELS,                                               \
    MicArrayPinDeviceFormatsAndModes,                                           \
    SIZEOF_ARRAY(MicArrayPinDeviceFormatsAndModes),                             \
    MicArray1TopologyPhysicalConnections,                                       \
    SIZEOF_ARRAY(MicArray1TopologyPhysicalConnections),                         \
    ENDPOINT_NO_FLAGS,                                                          \
    (n) - 1,                                                                    \
};

AUDIOROUTE_MIC_PAIR(1, L"TopologyMic1", L"WaveMic1")
AUDIOROUTE_MIC_PAIR(2, L"TopologyMic2", L"WaveMic2")
AUDIOROUTE_MIC_PAIR(3, L"TopologyMic3", L"WaveMic3")
AUDIOROUTE_MIC_PAIR(4, L"TopologyMic4", L"WaveMic4")


//=============================================================================
//
// Render miniport pairs. NOTE: the split of render and capture is arbitrary and
// unnessary, this array could contain capture endpoints.
//
static
PENDPOINT_MINIPAIR  g_RenderEndpoints[] = 
{
    &SpeakerMiniports1,
    &SpeakerMiniports2,
    &SpeakerMiniports3,
    &SpeakerMiniports4,
};

#define g_cRenderEndpoints  (SIZEOF_ARRAY(g_RenderEndpoints))

//=============================================================================
//
// Capture miniport pairs. NOTE: the split of render and capture is arbitrary and
// unnessary, this array could contain render endpoints.
//
static
PENDPOINT_MINIPAIR  g_CaptureEndpoints[] =
{
    &MicMiniports1,
    &MicMiniports2,
    &MicMiniports3,
    &MicMiniports4,
};

#define g_cCaptureEndpoints (SIZEOF_ARRAY(g_CaptureEndpoints))

//=============================================================================
//
// Total miniports = # endpoints * 2 (topology + wave).
//
#define g_MaxMiniports  ((g_cRenderEndpoints + g_cCaptureEndpoints) * 2)

#endif // _SIMPLEAUDIOSAMPLE_MINIPAIRS_H_

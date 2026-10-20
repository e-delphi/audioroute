/*++

Module Name:

    control.cpp

Abstract:

    Dispositivo de controle \\.\audioroute (ver control.h e
    audioroute_ioctl.h).

    O PortCls é dono das rotinas de despacho do driver. Aqui elas são
    trocadas por ControlDispatch, que atende o dispositivo de controle e
    repassa todo o resto (os filtros de áudio) às rotinas originais.

    Um app por vez: abrir de novo enquanto já está aberto dá
    STATUS_DEVICE_BUSY.

--*/

// O DPF usa do { } while (0), como no resto do exemplo.
#pragma warning (disable : 4127)

#include <definitions.h>
#include <wdmsec.h>
#include "loopback.h"
#include "control.h"
#include "audioroute_ioctl.h"

// Classe do dispositivo de controle (exigida pelo IoCreateDeviceSecure).
// {c1bdd805-c50f-475a-b9f4-1e552abb805a}
static const GUID GUID_AUDIOROUTE_CONTROL =
    { 0xc1bdd805, 0xc50f, 0x475a, { 0xb9, 0xf4, 0x1e, 0x55, 0x2a, 0xbb, 0x80, 0x5a } };

DECLARE_CONST_UNICODE_STRING(g_ControlName, L"\\Device\\audioroute");
DECLARE_CONST_UNICODE_STRING(g_ControlLink, L"\\DosDevices\\audioroute");
// Sistema e administradores: tudo. Usuários logados: ler e escrever.
DECLARE_CONST_UNICODE_STRING(g_ControlSddl, L"D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)");

static PDEVICE_OBJECT   g_ControlDevice;
static volatile LONG    g_ControlOpen;
static PDRIVER_DISPATCH g_PcCreate;
static PDRIVER_DISPATCH g_PcClose;
static PDRIVER_DISPATCH g_PcCleanup;
static PDRIVER_DISPATCH g_PcDeviceControl;

#pragma code_seg()
static NTSTATUS Complete(_Inout_ PIRP Irp, _In_ NTSTATUS Status, _In_ ULONG_PTR Information)
{
    Irp->IoStatus.Status = Status;
    Irp->IoStatus.Information = Information;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return Status;
}

#pragma code_seg()
static NTSTATUS DeviceControl(_Inout_ PIRP Irp, _In_ PIO_STACK_LOCATION Stack)
{
    PVOID buffer = Irp->AssociatedIrp.SystemBuffer;
    ULONG inLen = Stack->Parameters.DeviceIoControl.InputBufferLength;
    ULONG outLen = Stack->Parameters.DeviceIoControl.OutputBufferLength;
    const ULONG frameBytes = AUDIOROUTE_CHANNELS * (AUDIOROUTE_BITS_PER_SAMPLE / 8);

    switch (Stack->Parameters.DeviceIoControl.IoControlCode)
    {
    case IOCTL_AUDIOROUTE_GET_INFO:
    {
        if (outLen < sizeof(AUDIOROUTE_INFO) || buffer == NULL)
        {
            return Complete(Irp, STATUS_BUFFER_TOO_SMALL, 0);
        }
        PAUDIOROUTE_INFO info = (PAUDIOROUTE_INFO)buffer;
        info->Version = AUDIOROUTE_IOCTL_VERSION;
        info->SampleRate = AUDIOROUTE_SAMPLE_RATE;
        info->Channels = AUDIOROUTE_CHANNELS;
        info->BitsPerSample = AUDIOROUTE_BITS_PER_SAMPLE;
        return Complete(Irp, STATUS_SUCCESS, sizeof(AUDIOROUTE_INFO));
    }

    case IOCTL_AUDIOROUTE_READ_SPEAKER:
    {
        ULONG bytes = outLen - outLen % frameBytes;
        if (bytes > 0 && buffer != NULL)
        {
            LoopbackAppReadSpeaker((BYTE*)buffer, bytes);
        }
        return Complete(Irp, STATUS_SUCCESS, bytes);
    }

    case IOCTL_AUDIOROUTE_WRITE_MIC:
    {
        ULONG bytes = inLen - inLen % frameBytes;
        if (bytes > 0 && buffer != NULL)
        {
            LoopbackAppWriteMic((const BYTE*)buffer, bytes);
        }
        return Complete(Irp, STATUS_SUCCESS, 0);
    }

    default:
        return Complete(Irp, STATUS_INVALID_DEVICE_REQUEST, 0);
    }
}

_Dispatch_type_(IRP_MJ_CREATE)
_Dispatch_type_(IRP_MJ_CLOSE)
_Dispatch_type_(IRP_MJ_CLEANUP)
_Dispatch_type_(IRP_MJ_DEVICE_CONTROL)
static DRIVER_DISPATCH ControlDispatch;

#pragma code_seg()
static NTSTATUS ControlDispatch(_In_ PDEVICE_OBJECT DeviceObject, _Inout_ PIRP Irp)
{
    PIO_STACK_LOCATION stack = IoGetCurrentIrpStackLocation(Irp);

    // Não é o dispositivo de controle: é um filtro de áudio, do PortCls.
    // Pelo tipo, não pelo ponteiro: o CLOSE do app pode chegar depois do
    // ControlDelete (o objeto só some quando o último handle fecha).
    if (DeviceObject->DeviceType != FILE_DEVICE_AUDIOROUTE)
    {
        switch (stack->MajorFunction)
        {
        case IRP_MJ_CREATE:         return g_PcCreate(DeviceObject, Irp);
        case IRP_MJ_CLOSE:          return g_PcClose(DeviceObject, Irp);
        case IRP_MJ_CLEANUP:        return g_PcCleanup(DeviceObject, Irp);
        default:                    return g_PcDeviceControl(DeviceObject, Irp);
        }
    }

    switch (stack->MajorFunction)
    {
    case IRP_MJ_CREATE:
        if (InterlockedCompareExchange(&g_ControlOpen, 1, 0) != 0)
        {
            return Complete(Irp, STATUS_DEVICE_BUSY, 0);
        }
        LoopbackAppConnected(TRUE);
        return Complete(Irp, STATUS_SUCCESS, 0);

    case IRP_MJ_CLEANUP:
        return Complete(Irp, STATUS_SUCCESS, 0);

    case IRP_MJ_CLOSE:
        LoopbackAppConnected(FALSE);
        InterlockedExchange(&g_ControlOpen, 0);
        return Complete(Irp, STATUS_SUCCESS, 0);

    case IRP_MJ_DEVICE_CONTROL:
        return DeviceControl(Irp, stack);

    default:
        return Complete(Irp, STATUS_INVALID_DEVICE_REQUEST, 0);
    }
}

#pragma code_seg("PAGE")
VOID ControlHookDispatch(_In_ PDRIVER_OBJECT DriverObject)
{
    PAGED_CODE();
    g_PcCreate = DriverObject->MajorFunction[IRP_MJ_CREATE];
    g_PcClose = DriverObject->MajorFunction[IRP_MJ_CLOSE];
    g_PcCleanup = DriverObject->MajorFunction[IRP_MJ_CLEANUP];
    g_PcDeviceControl = DriverObject->MajorFunction[IRP_MJ_DEVICE_CONTROL];
    DriverObject->MajorFunction[IRP_MJ_CREATE] = ControlDispatch;
    DriverObject->MajorFunction[IRP_MJ_CLOSE] = ControlDispatch;
    DriverObject->MajorFunction[IRP_MJ_CLEANUP] = ControlDispatch;
    DriverObject->MajorFunction[IRP_MJ_DEVICE_CONTROL] = ControlDispatch;
}

#pragma code_seg("PAGE")
NTSTATUS ControlCreate(_In_ PDRIVER_OBJECT DriverObject)
{
    PAGED_CODE();
    if (g_ControlDevice != NULL)
    {
        return STATUS_SUCCESS;
    }

    PDEVICE_OBJECT device = NULL;
    NTSTATUS status = IoCreateDeviceSecure(DriverObject, 0, (PUNICODE_STRING)&g_ControlName,
                                           FILE_DEVICE_AUDIOROUTE, FILE_DEVICE_SECURE_OPEN, FALSE,
                                           &g_ControlSddl, &GUID_AUDIOROUTE_CONTROL, &device);
    if (!NT_SUCCESS(status))
    {
        DPF(D_ERROR, ("audioroute: IoCreateDeviceSecure falhou, 0x%x", status));
        return status;
    }

    status = IoCreateSymbolicLink((PUNICODE_STRING)&g_ControlLink, (PUNICODE_STRING)&g_ControlName);
    if (!NT_SUCCESS(status))
    {
        DPF(D_ERROR, ("audioroute: IoCreateSymbolicLink falhou, 0x%x", status));
        IoDeleteDevice(device);
        return status;
    }

    device->Flags |= DO_BUFFERED_IO;
    device->Flags &= ~DO_DEVICE_INITIALIZING;
    g_ControlDevice = device;
    return STATUS_SUCCESS;
}

#pragma code_seg("PAGE")
VOID ControlDelete()
{
    PAGED_CODE();
    if (g_ControlDevice == NULL)
    {
        return;
    }
    IoDeleteSymbolicLink((PUNICODE_STRING)&g_ControlLink);
    PDEVICE_OBJECT device = g_ControlDevice;
    g_ControlDevice = NULL;
    IoDeleteDevice(device);
}

/*++

Module Name:

    control.h

Abstract:

    Dispositivo de controle \\.\audioroute: o canal entre o driver e o app
    (contrato em audioroute_ioctl.h).

--*/

#ifndef _AUDIOROUTE_CONTROL_H_
#define _AUDIOROUTE_CONTROL_H_

// DriverEntry, depois do PcInitializeAdapterDriver: passa a receber CREATE,
// CLOSE, CLEANUP e DEVICE_CONTROL; o que não for do controle segue para o PortCls.
VOID ControlHookDispatch(_In_ PDRIVER_OBJECT DriverObject);

// AddDevice: cria \Device\audioroute e o link \\.\audioroute (uma vez).
NTSTATUS ControlCreate(_In_ PDRIVER_OBJECT DriverObject);

// IRP_MN_REMOVE_DEVICE: remove o dispositivo de controle.
VOID ControlDelete();

#endif // _AUDIOROUTE_CONTROL_H_

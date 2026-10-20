//! Streams WASAPI: uma thread por dispositivo, acordada pelo evento do
//! próprio dispositivo, com prioridade MMCSS "Pro Audio".
//!
//! Três jeitos de abrir:
//! - **Exclusive**: o menor período do hardware, no formato nativo dele.
//! - **Shared de baixa latência** (IAudioClient3): o menor período que o
//!   motor de áudio do Windows oferece para o dispositivo.
//! - **Shared padrão**: período do Windows (10 ms) com conversão automática
//!   de formato; usado no loopback e quando os outros não estão disponíveis.
//!
//! Todo objeto COM é criado e usado dentro da thread do stream.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use audioroute_core::audio::{
    CHANNELS, CaptureCallback, DEFAULT_EXCLUSIVE_PERIOD_FRAMES, RenderCallback, SAMPLE_RATE, ShareMode, Stream,
    StreamError, StreamInfo, StreamRequest,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_SHAREMODE_EXCLUSIVE,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient,
    IAudioClient, IAudioClient3, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, SetEvent,
    WaitForMultipleObjects,
};
use windows::core::{HSTRING, w};

use crate::com::ComGuard;
use crate::format::{NativeFormat, SampleType};

/// 100 ns: a unidade de tempo da WASAPI.
const REFTIMES_PER_SEC: i64 = 10_000_000;

pub(crate) enum Callback {
    Capture(CaptureCallback),
    Render(RenderCallback),
}

pub(crate) struct Request {
    pub endpoint_id: String,
    /// Loopback: captura o que toca no endpoint de saída.
    pub loopback: bool,
    pub stream: StreamRequest,
}

/// HANDLE não é Send no crate `windows`; um evento pode ser sinalizado de
/// qualquer thread, então é seguro movê-lo.
#[derive(Clone, Copy)]
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

pub struct WasapiStream {
    info: StreamInfo,
    stop: Arc<AtomicBool>,
    /// Fica `false` se o loop de áudio parar por erro (dispositivo caiu).
    alive: Arc<AtomicBool>,
    wake: SendHandle,
    thread: Option<JoinHandle<()>>,
}

impl Stream for WasapiStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for WasapiStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        unsafe {
            let _ = SetEvent(self.wake.0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            let _ = CloseHandle(self.wake.0);
        }
    }
}

/// Abre o stream numa thread nova e espera ela informar se deu certo.
pub(crate) fn open(request: Request, callback: Callback) -> Result<WasapiStream, StreamError> {
    let wake = SendHandle(unsafe { CreateEventW(None, false, false, None)? });
    let stop = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));
    let (tx, rx) = mpsc::sync_channel(1);

    let (thread_stop, thread_alive) = (stop.clone(), alive.clone());
    let name = if matches!(callback, Callback::Capture(_)) { "audio-capture" } else { "audio-render" };
    let thread = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || run(request, callback, thread_stop, thread_alive, wake, tx))?;

    match rx.recv() {
        Ok(Ok(info)) => Ok(WasapiStream { info, stop, alive, wake, thread: Some(thread) }),
        Ok(Err(e)) => {
            let _ = thread.join();
            unsafe {
                let _ = CloseHandle(wake.0);
            }
            Err(e)
        }
        Err(_) => Err("a thread de áudio terminou sem responder".into()),
    }
}

/// Tudo que a thread precisa depois de configurar o cliente.
struct Setup {
    client: IAudioClient,
    event: HANDLE,
    format: NativeFormat,
    buffer_frames: u32,
    exclusive: bool,
    info: StreamInfo,
}

fn run(
    request: Request,
    mut callback: Callback,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    wake: SendHandle,
    tx: mpsc::SyncSender<Result<StreamInfo, StreamError>>,
) {
    let _com = match ComGuard::new() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Err(e.into()));
            return;
        }
    };
    let is_capture = matches!(callback, Callback::Capture(_));
    let setup = match setup(&request, is_capture) {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Err(e));
            return;
        }
    };
    let _ = tx.send(Ok(setup.info.clone()));

    // MMCSS: o Windows passa a tratar esta thread como áudio profissional.
    let mut task_index = 0u32;
    let mmcss = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index) }.ok();

    let result = match &mut callback {
        Callback::Capture(cb) => capture_loop(&setup, cb, &stop, wake),
        Callback::Render(cb) => render_loop(&setup, cb, &stop, wake),
    };
    if let Err(e) = result {
        // Ex.: dispositivo desconectado ou driver reiniciado. O motor vê
        // is_alive() == false, fecha o stream e tenta reabrir depois.
        eprintln!("[audio] stream encerrado: {}", e.message());
        alive.store(false, Ordering::SeqCst);
    }

    unsafe {
        let _ = setup.client.Stop();
        if let Some(h) = mmcss {
            let _ = AvRevertMmThreadCharacteristics(h);
        }
        let _ = CloseHandle(setup.event);
    }
}

/// Espera o evento do dispositivo ou o pedido de parada.
/// Retorna `false` quando é hora de parar.
fn wait(setup: &Setup, stop: &AtomicBool, wake: SendHandle) -> bool {
    loop {
        // Timeout: o loopback não recebe eventos enquanto nada toca.
        let r = unsafe { WaitForMultipleObjects(&[setup.event, wake.0], false, 2000) };
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        if r == WAIT_OBJECT_0 {
            return true;
        }
    }
}

fn capture_loop(
    setup: &Setup,
    cb: &mut CaptureCallback,
    stop: &AtomicBool,
    wake: SendHandle,
) -> windows::core::Result<()> {
    let capture: IAudioCaptureClient = unsafe { setup.client.GetService()? };
    let mut scratch = vec![0f32; setup.buffer_frames as usize * CHANNELS];
    unsafe { setup.client.Start()? };

    while wait(setup, stop, wake) {
        loop {
            let packet = unsafe { capture.GetNextPacketSize()? };
            if packet == 0 {
                break;
            }
            let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
            unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)? };
            let n = (frames as usize).min(setup.buffer_frames as usize);
            let out = &mut scratch[..n * CHANNELS];
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                out.fill(0.0);
            } else {
                let bytes = unsafe { std::slice::from_raw_parts(data, n * setup.format.frame_bytes()) };
                setup.format.to_internal(bytes, n, out);
            }
            cb(out);
            unsafe { capture.ReleaseBuffer(frames)? };
        }
    }
    Ok(())
}

fn render_loop(
    setup: &Setup,
    cb: &mut RenderCallback,
    stop: &AtomicBool,
    wake: SendHandle,
) -> windows::core::Result<()> {
    let render: IAudioRenderClient = unsafe { setup.client.GetService()? };
    let mut scratch = vec![0f32; setup.buffer_frames as usize * CHANNELS];
    let frame_bytes = setup.format.frame_bytes();

    // Começa com o buffer em silêncio, para não tocar lixo nem estalar.
    unsafe {
        render.GetBuffer(setup.buffer_frames)?;
        render.ReleaseBuffer(setup.buffer_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
        setup.client.Start()?;
    }

    while wait(setup, stop, wake) {
        // Exclusive (evento): preenche o buffer inteiro a cada evento.
        // Shared: preenche só o espaço livre.
        let frames = if setup.exclusive {
            setup.buffer_frames
        } else {
            setup.buffer_frames - unsafe { setup.client.GetCurrentPadding()? }
        };
        if frames == 0 {
            continue;
        }
        let out = &mut scratch[..frames as usize * CHANNELS];
        cb(out);
        unsafe {
            let data = render.GetBuffer(frames)?;
            let bytes = std::slice::from_raw_parts_mut(data, frames as usize * frame_bytes);
            setup.format.from_internal(out, frames as usize, bytes);
            render.ReleaseBuffer(frames, 0)?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------ configuração

pub(crate) fn device_for_probe(endpoint_id: &str) -> windows::core::Result<IMMDevice> {
    device(endpoint_id)
}

fn device(endpoint_id: &str) -> windows::core::Result<IMMDevice> {
    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    unsafe { enumerator.GetDevice(&HSTRING::from(endpoint_id)) }
}

fn setup(request: &Request, is_capture: bool) -> Result<Setup, StreamError> {
    let device = device(&request.endpoint_id)?;
    let event = unsafe { CreateEventW(None, false, false, None)? };
    let wanted = request.stream.period_frames;
    let result = if request.loopback {
        setup_shared_converted(&device, event, true)
    } else if request.stream.mode == ShareMode::Exclusive {
        setup_exclusive(&device, event, is_capture, wanted.unwrap_or(DEFAULT_EXCLUSIVE_PERIOD_FRAMES))
    } else {
        setup_shared_low_latency(&device, event, wanted)
            .or_else(|_| setup_shared_converted(&device, event, false))
    };
    if result.is_err() {
        unsafe {
            let _ = CloseHandle(event);
        }
    }
    result
}

/// Formato de mistura do Windows para o dispositivo (o que o shared usa).
fn mix_format(client: &IAudioClient) -> windows::core::Result<(NativeFormat, Vec<u8>)> {
    unsafe {
        let ptr = client.GetMixFormat()?;
        let size = std::mem::size_of::<WAVEFORMATEX>() + (*ptr).cbSize as usize;
        let raw = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
        let parsed = NativeFormat::from_waveformat(ptr);
        CoTaskMemFree(Some(ptr as *const _));
        parsed.map(|f| (f, raw)).ok_or_else(|| windows::core::Error::empty())
    }
}

/// Shared pelo IAudioClient3, no período pedido (ou no menor que o motor de
/// áudio do Windows oferece). Só serve se o formato de mistura já for 48 kHz.
fn setup_shared_low_latency(device: &IMMDevice, event: HANDLE, wanted: Option<u32>) -> Result<Setup, StreamError> {
    let client3: IAudioClient3 = unsafe { device.Activate(CLSCTX_ALL, None)? };
    let client: IAudioClient = windows::core::Interface::cast(&client3)?;
    let (format, raw) = mix_format(&client)?;
    if format.rate != SAMPLE_RATE {
        return Err(format!("formato de mistura em {} Hz", format.rate).into());
    }
    let wf = raw.as_ptr() as *const WAVEFORMATEX;
    let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
    unsafe { client3.GetSharedModeEnginePeriod(wf, &mut default, &mut fundamental, &mut min, &mut max)? };
    // O período precisa ser min + k·fundamental, sem passar de max.
    let period = match wanted {
        Some(w) if w > min && fundamental > 0 => (min + (w - min) / fundamental * fundamental).min(max),
        _ => min,
    };
    unsafe {
        client3.InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, period, wf, None)?;
        client.SetEventHandle(event)?;
    }
    let label = if period < default { "shared baixa latência" } else { "shared" };
    finish(client, event, format, period, false, format!("{label} ({})", format.describe()))
}

/// Shared padrão, com conversão automática para 48 kHz f32 estéreo.
fn setup_shared_converted(device: &IMMDevice, event: HANDLE, loopback: bool) -> Result<Setup, StreamError> {
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };
    let format = NativeFormat::new(SAMPLE_RATE, CHANNELS as u16, SampleType::F32);
    let ext = format.to_extensible();
    let mut flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if loopback {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    let mut default = 0i64;
    unsafe {
        client.GetDevicePeriod(Some(&mut default), None)?;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, &ext.Format, None)?;
        client.SetEventHandle(event)?;
    }
    let period = (default * SAMPLE_RATE as i64 / REFTIMES_PER_SEC) as u32;
    let label = if loopback { "shared (loopback)" } else { "shared padrão" };
    finish(client, event, format, period, false, format!("{label} (convertido)"))
}

/// Exclusive: o dispositivo inteiro para nós, no período pedido (nunca
/// abaixo do mínimo do hardware).
fn setup_exclusive(
    device: &IMMDevice,
    event: HANDLE,
    is_capture: bool,
    wanted_frames: u32,
) -> Result<Setup, StreamError> {
    let probe: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };
    let format = exclusive_format(&probe, is_capture).ok_or("nenhum formato exclusivo a 48 kHz aceito")?;
    let ext = format.to_extensible();
    let mut min = 0i64;
    unsafe { probe.GetDevicePeriod(None, Some(&mut min))? };
    drop(probe);

    let wanted = wanted_frames as i64 * REFTIMES_PER_SEC / SAMPLE_RATE as i64;
    let mut period = wanted.max(min);
    for _ in 0..2 {
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };
        let r = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_EXCLUSIVE,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                period,
                period,
                &ext.Format,
                None,
            )
        };
        match r {
            Ok(()) => {
                unsafe { client.SetEventHandle(event)? };
                let frames = unsafe { client.GetBufferSize()? };
                return finish(client, event, format, frames, true, format!("exclusive ({})", format.describe()));
            }
            // O driver quer um buffer de outro tamanho: refaz com o que ele sugere.
            Err(e) if e.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => {
                let frames = unsafe { client.GetBufferSize()? } as i64;
                period = (REFTIMES_PER_SEC * frames + format.rate as i64 / 2) / format.rate as i64;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err("o driver recusou todos os tamanhos de buffer".into())
}

/// Procura um formato a 48 kHz que o dispositivo aceite em exclusive.
pub(crate) fn exclusive_format(client: &IAudioClient, is_capture: bool) -> Option<NativeFormat> {
    let mix_channels = mix_format(client).map(|(f, _)| f.channels).unwrap_or(2);
    let mut channel_options = vec![mix_channels, 2];
    if is_capture {
        channel_options.push(1);
    }
    channel_options.dedup();
    let samples = [
        (SampleType::F32, 32),
        (SampleType::I32, 32),
        (SampleType::I32, 24),
        (SampleType::I24, 24),
        (SampleType::I16, 16),
    ];
    for &channels in &channel_options {
        for &(sample, valid_bits) in &samples {
            let mut format = NativeFormat::new(SAMPLE_RATE, channels, sample);
            format.valid_bits = valid_bits;
            let ext = format.to_extensible();
            let hr = unsafe { client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, &ext.Format, None) };
            if hr.is_ok() && hr.0 == 0 {
                return Some(format);
            }
        }
    }
    None
}

fn finish(
    client: IAudioClient,
    event: HANDLE,
    format: NativeFormat,
    period_frames: u32,
    exclusive: bool,
    native_format: String,
) -> Result<Setup, StreamError> {
    let buffer_frames = unsafe { client.GetBufferSize()? };
    let latency = unsafe { client.GetStreamLatency() }.unwrap_or(0);
    let latency_frames = (latency * SAMPLE_RATE as i64 / REFTIMES_PER_SEC) as u32;
    let info = StreamInfo {
        mode: if exclusive { ShareMode::Exclusive } else { ShareMode::Shared },
        period_frames: period_frames.max(1),
        max_callback_frames: buffer_frames,
        device_latency_frames: latency_frames + period_frames,
        native_format,
        // Hardware: os callbacks seguem o relógio da placa, com jitter.
        steady_clock: false,
    };
    Ok(Setup { client, event, format, buffer_frames, exclusive, info })
}

//! Acesso ao driver pelo dispositivo de controle `\\.\audioroute`.
//!
//! Espelha `driver/Source/Inc/audioroute_ioctl.h`: mudou lá, muda aqui.

use std::io;

use windows::Win32::Foundation::{CloseHandle, ERROR_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE, OPEN_EXISTING};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::w;

pub const IOCTL_VERSION: u32 = 2;
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
pub const BITS_PER_SAMPLE: u32 = 32;
/// Pares alto-falante/microfone do driver (AUDIOROUTE_PAIRS).
pub const MAX_PAIRS: u32 = 4;

const IOCTL_GET_INFO: u32 = 0x8A55_2000;
const IOCTL_READ_SPEAKER: u32 = 0x8A55_6004;
const IOCTL_WRITE_MIC: u32 = 0x8A55_A008;

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub version: u32,
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: u32,
    /// Pares alto-falante/microfone que o driver tem.
    pub pairs: u32,
}

/// Por que o driver não está disponível.
#[derive(Debug)]
pub enum OpenError {
    /// Driver não instalado (ou o dispositivo desligado).
    NotInstalled,
    /// Outro programa (outra instância do audioroute) está usando.
    Busy,
    /// Versão do driver diferente da que o app conhece.
    Version(Info),
    Other(io::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NotInstalled => f.write_str("driver do audioroute não instalado"),
            OpenError::Busy => f.write_str("o driver do audioroute já está em uso por outro programa"),
            OpenError::Version(i) => {
                write!(f, "driver do audioroute em outra versão ({}; o app usa {IOCTL_VERSION})", i.version)
            }
            OpenError::Other(e) => write!(f, "driver do audioroute: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Handle aberto para o driver. Um por vez no sistema inteiro.
pub struct Driver {
    handle: HANDLE,
}

// O handle é usado de várias threads (leitura do alto-falante e escrita do
// microfone); o Windows serializa as chamadas no mesmo handle.
unsafe impl Send for Driver {}
unsafe impl Sync for Driver {}

impl Driver {
    pub fn open() -> Result<Self, OpenError> {
        let handle = unsafe {
            CreateFileW(
                w!(r"\\.\audioroute"),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|e| {
            if e.code() == ERROR_BUSY.to_hresult() {
                OpenError::Busy
            } else if e.code().0 as u32 & 0xFFFF == 2 || e.code().0 as u32 & 0xFFFF == 3 {
                OpenError::NotInstalled // arquivo/caminho não encontrado
            } else {
                OpenError::Other(io::Error::from_raw_os_error((e.code().0 as u32 & 0xFFFF) as i32))
            }
        })?;
        let driver = Driver { handle };
        let info = driver.info().map_err(OpenError::Other)?;
        if info.version != IOCTL_VERSION
            || info.sample_rate != SAMPLE_RATE
            || info.channels != CHANNELS
            || info.bits_per_sample != BITS_PER_SAMPLE
            || info.pairs != MAX_PAIRS
        {
            return Err(OpenError::Version(info));
        }
        Ok(driver)
    }

    fn ioctl(&self, code: u32, input: Option<&[u8]>, output: Option<&mut [u8]>) -> io::Result<u32> {
        let mut returned = 0u32;
        let (in_ptr, in_len) = input.map_or((std::ptr::null(), 0), |b| (b.as_ptr().cast(), b.len() as u32));
        let (out_ptr, out_len) = match output {
            Some(b) => (b.as_mut_ptr().cast(), b.len() as u32),
            None => (std::ptr::null_mut(), 0),
        };
        unsafe {
            DeviceIoControl(
                self.handle,
                code,
                Some(in_ptr),
                in_len,
                Some(out_ptr),
                out_len,
                Some(&mut returned),
                None,
            )
        }
        .map_err(|e| io::Error::from_raw_os_error((e.code().0 as u32 & 0xFFFF) as i32))?;
        Ok(returned)
    }

    pub fn info(&self) -> io::Result<Info> {
        let mut info = Info::default();
        let bytes = unsafe {
            std::slice::from_raw_parts_mut((&mut info as *mut Info).cast::<u8>(), size_of::<Info>())
        };
        self.ioctl(IOCTL_GET_INFO, None, Some(bytes))?;
        Ok(info)
    }

    /// Preenche `out` (estéreo intercalado) com o que tocou no alto-falante
    /// do par. Sem som tocando, vem silêncio.
    pub fn read_speaker(&self, pair: u32, out: &mut [i32]) -> io::Result<()> {
        let bytes = unsafe { std::slice::from_raw_parts_mut(out.as_mut_ptr().cast::<u8>(), size_of_val(out)) };
        self.ioctl(IOCTL_READ_SPEAKER, Some(&pair.to_le_bytes()), Some(bytes))?;
        Ok(())
    }

    /// Manda `data` (estéreo intercalado) para o microfone do par. `scratch`
    /// é reaproveitado entre chamadas (cabeçalho + amostras), sem alocar.
    pub fn write_mic(&self, pair: u32, data: &[i32], scratch: &mut Vec<u8>) -> io::Result<()> {
        scratch.clear();
        scratch.extend_from_slice(&pair.to_le_bytes());
        scratch.extend_from_slice(&0u32.to_le_bytes()); // reservado
        for s in data {
            scratch.extend_from_slice(&s.to_le_bytes());
        }
        self.ioctl(IOCTL_WRITE_MIC, Some(scratch), None)?;
        Ok(())
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

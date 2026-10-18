//! Identificador único da instalação do Windows.

use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
};
use windows::core::w;

/// Lê o `MachineGuid` do registro (visão 64 bits).
pub fn machine_id() -> windows::core::Result<String> {
    let key = w!("SOFTWARE\\Microsoft\\Cryptography");
    let value = w!("MachineGuid");
    let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY;

    // Primeira chamada descobre o tamanho; a segunda lê.
    let mut size = 0u32;
    unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key, value, flags, None, None, Some(&mut size)) }.ok()?;
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            value,
            flags,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()?;

    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(String::from_utf16_lossy(&buf[..len]))
}

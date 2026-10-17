use std::{io, ptr, slice};

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = input_blob(data)?;
    let mut output = empty_blob();
    // SAFETY: `input` borrows `data` for the duration of the call; `output` is OS-allocated and freed in `take_output`.
    let ok = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    take_output(ok, output)
}

pub fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = input_blob(data)?;
    let mut output = empty_blob();
    // SAFETY: same contract as `protect`.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    take_output(ok, output)
}

fn input_blob(data: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
    let len = u32::try_from(data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "data too large for DPAPI"))?;
    Ok(CRYPT_INTEGER_BLOB {
        cbData: len,
        pbData: data.as_ptr().cast_mut(),
    })
}

fn empty_blob() -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    }
}

fn take_output(ok: i32, output: CRYPT_INTEGER_BLOB) -> io::Result<Vec<u8>> {
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if output.pbData.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: on success DPAPI returns `cbData` writable bytes at `pbData`, allocated with LocalAlloc.
    let buffer = unsafe { slice::from_raw_parts_mut(output.pbData, output.cbData as usize) };
    let bytes = buffer.to_vec();
    // Decrypted secrets must not linger in the freed OS buffer.
    buffer.fill(0);
    // SAFETY: `pbData` came from LocalAlloc and is not used after this.
    unsafe { LocalFree(output.pbData.cast()) };
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_current_user_key() {
        let secret = b"SAPISID=test-only-value";
        let sealed = protect(secret).unwrap();
        assert_ne!(sealed.as_slice(), secret);
        assert_eq!(unprotect(&sealed).unwrap(), secret);
    }

    #[test]
    fn rejects_tampered_data() {
        let mut sealed = protect(b"value").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        assert!(unprotect(&sealed).is_err());
    }
}

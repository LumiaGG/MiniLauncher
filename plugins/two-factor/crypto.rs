//! Rust wrappers around Windows CNG HMAC and current-user DPAPI.
use std::{
    ptr::{null, null_mut},
    sync::OnceLock,
};
use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};

pub fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    if unsafe {
        BCryptGenRandom(
            null_mut(),
            bytes.as_mut_ptr(),
            16,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err("生成账号标识失败".into());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn protect(bytes: &[u8], decrypt: bool) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err("账号库加密/解密失败（仅原 Windows 用户可解密）".into());
    }
    // DPAPI owns this allocation; copy before releasing it with LocalFree.
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

pub fn code(
    secret: &[u8],
    algorithm: &str,
    digits: u32,
    period: u64,
    time: u64,
) -> Result<String, String> {
    if !(6..=8).contains(&digits) || period == 0 {
        return Err("验证码参数无效".into());
    }
    static SHA1_HANDLE: OnceLock<usize> = OnceLock::new();
    static SHA256_HANDLE: OnceLock<usize> = OnceLock::new();
    static SHA512_HANDLE: OnceLock<usize> = OnceLock::new();
    let (slot, name, size) = match algorithm {
        "SHA1" => (&SHA1_HANDLE, BCRYPT_SHA1_ALGORITHM, 20),
        "SHA256" => (&SHA256_HANDLE, BCRYPT_SHA256_ALGORITHM, 32),
        "SHA512" => (&SHA512_HANDLE, BCRYPT_SHA512_ALGORITHM, 64),
        _ => return Err("仅支持 SHA1 / SHA256 / SHA512".into()),
    };
    let provider = if let Some(&handle) = slot.get() {
        handle as BCRYPT_ALG_HANDLE
    } else {
        let mut provider = null_mut();
        if unsafe {
            BCryptOpenAlgorithmProvider(&mut provider, name, null(), BCRYPT_ALG_HANDLE_HMAC_FLAG)
        } < 0
        {
            return Err("无法初始化 HMAC".into());
        }
        let handle = provider as usize;
        if slot.set(handle).is_err() {
            unsafe {
                BCryptCloseAlgorithmProvider(provider, 0);
            }
        }
        *slot.get().unwrap() as BCRYPT_ALG_HANDLE
    };
    let counter = (time / period).to_be_bytes();
    let mut hash = vec![0u8; size];
    let status = unsafe {
        BCryptHash(
            provider,
            secret.as_ptr(),
            secret.len() as u32,
            counter.as_ptr(),
            8,
            hash.as_mut_ptr(),
            size as u32,
        )
    };
    if status < 0 {
        return Err("生成验证码失败".into());
    }
    let offset = (hash[size - 1] & 15) as usize;
    let number = u32::from_be_bytes(hash[offset..offset + 4].try_into().unwrap()) & 0x7fff_ffff;
    Ok(format!(
        "{:0width$}",
        number % 10u32.pow(digits),
        width = digits as usize
    ))
}

pub fn base32(text: &str) -> Result<Vec<u8>, String> {
    let text: String = text
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != '-')
        .collect();
    let raw = text.trim_end_matches('=');
    if raw.is_empty()
        || raw.len() > 4096
        || !matches!(raw.len() % 8, 0 | 2 | 4 | 5 | 7)
        || (text.len() != raw.len() && (!text.len().is_multiple_of(8) || text.len() - raw.len() > 6))
    {
        return Err("密钥不是有效的 Base32".into());
    }
    let mut result = Vec::new();
    let (mut bits, mut buffer) = (0u32, 0u32);
    for ch in raw.bytes() {
        let value = match ch.to_ascii_uppercase() {
            b'A'..=b'Z' => ch.to_ascii_uppercase() - b'A',
            b'2'..=b'7' => ch - b'2' + 26,
            _ => return Err("密钥不是有效的 Base32".into()),
        };
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            result.push((buffer >> bits) as u8);
        }
        buffer &= (1 << bits) - 1;
    }
    if buffer != 0 {
        return Err("Base32 尾部编码无效".into());
    }
    Ok(result)
}

use std::fmt::Write as _;
use std::path::Path;
use std::ptr::null;

use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider, BCryptHash,
    BCryptOpenAlgorithmProvider,
};

pub(crate) fn hash_bytes(bytes: &[u8]) -> Result<[u8; 32], String> {
    let mut algorithm: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
    let status =
        unsafe { BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_SHA256_ALGORITHM, null(), 0) };
    if status < 0 {
        return Err(format!(
            "could not open the SHA-256 provider (NTSTATUS {status:#x})"
        ));
    }

    let mut digest = [0u8; 32];
    let status = unsafe {
        BCryptHash(
            algorithm,
            null(),
            0,
            bytes.as_ptr(),
            bytes.len() as u32,
            digest.as_mut_ptr(),
            digest.len() as u32,
        )
    };
    unsafe { BCryptCloseAlgorithmProvider(algorithm, 0) };
    if status < 0 {
        return Err(format!("could not hash the data (NTSTATUS {status:#x})"));
    }
    Ok(digest)
}

pub(crate) fn hash_file(path: &Path) -> Result<[u8; 32], String> {
    let bytes = std::fs::read(path).map_err(|error| format!("could not read {path:?}: {error}"))?;
    hash_bytes(&bytes)
}

pub(crate) fn to_hex(hash: &[u8; 32]) -> String {
    let mut text = String::with_capacity(64);
    for byte in hash {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    #[test]
    fn hashes_known_vectors() {
        assert_eq!(
            super::to_hex(&super::hash_bytes(b"abc").unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            super::to_hex(&super::hash_bytes(b"").unwrap()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hashes_a_file() {
        let path = std::env::temp_dir().join(format!("hiyodyna-sha-{}.bin", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            super::hash_file(&path).unwrap(),
            super::hash_bytes(b"abc").unwrap()
        );
        let _ = std::fs::remove_file(path);
    }
}

//! Proteção da chave em disco.
//!
//! - Windows: DPAPI (`CryptProtectData`) com escopo do usuário atual — só o
//!   mesmo usuário, na mesma máquina, consegue desproteger.
//! - Outros sistemas (apenas desenvolvimento/testes): sem proteção.

use crate::IdentityError;

pub trait Protector: Send + Sync {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, IdentityError>;
    fn unprotect(&self, sealed: &[u8]) -> Result<Vec<u8>, IdentityError>;
}

/// Sem proteção. Usado em testes e em plataformas sem suporte.
pub struct PlainProtector;

impl Protector for PlainProtector {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, IdentityError> {
        Ok(plain.to_vec())
    }
    fn unprotect(&self, sealed: &[u8]) -> Result<Vec<u8>, IdentityError> {
        Ok(sealed.to_vec())
    }
}

/// Proteção nativa do sistema (DPAPI no Windows).
pub struct SystemProtector;

#[cfg(not(windows))]
impl Protector for SystemProtector {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, IdentityError> {
        PlainProtector.protect(plain)
    }
    fn unprotect(&self, sealed: &[u8]) -> Result<Vec<u8>, IdentityError> {
        PlainProtector.unprotect(sealed)
    }
}

#[cfg(windows)]
mod dpapi {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows::core::PCWSTR;

    use super::{IdentityError, Protector, SystemProtector};

    /// Entropia adicional: amarra o blob a este app.
    const ENTROPY: &[u8] = b"kr-desk/device-key/v1";

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    /// Copia o resultado alocado pelo Windows e libera com `LocalFree`.
    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: a DPAPI devolveu `cbData` bytes válidos em `pbData`.
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        // SAFETY: o buffer foi alocado pela DPAPI com LocalAlloc.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        }
        v
    }

    impl Protector for SystemProtector {
        fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, IdentityError> {
            let input = blob(plain);
            let entropy = blob(ENTROPY);
            let mut out = CRYPT_INTEGER_BLOB::default();
            // SAFETY: ponteiros válidos durante a chamada; `out` recebe memória da DPAPI.
            unsafe {
                CryptProtectData(
                    &input,
                    PCWSTR::null(),
                    Some(&entropy as *const CRYPT_INTEGER_BLOB),
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
            }
            .map_err(|e| IdentityError::Protect(e.to_string()))?;
            Ok(take(out))
        }

        fn unprotect(&self, sealed: &[u8]) -> Result<Vec<u8>, IdentityError> {
            let input = blob(sealed);
            let entropy = blob(ENTROPY);
            let mut out = CRYPT_INTEGER_BLOB::default();
            // SAFETY: idem `protect`.
            unsafe {
                CryptUnprotectData(
                    &input,
                    None,
                    Some(&entropy as *const CRYPT_INTEGER_BLOB),
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
            }
            .map_err(|e| IdentityError::Protect(e.to_string()))?;
            Ok(take(out))
        }
    }
}

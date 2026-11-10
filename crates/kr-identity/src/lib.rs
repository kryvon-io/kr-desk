//! Identidade do dispositivo: um par de chaves Ed25519 gerado na primeira
//! execução e guardado em disco **protegido pelo sistema operacional**
//! (DPAPI no Windows, com escopo do usuário). O ID público de 9 dígitos é
//! atribuído pelo servidor e fica vinculado à chave pública.

mod session;

pub use session::{
    KnownPeers, SessionAuthError, Trust, verification_code, verify_peer, verify_session_message,
};

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signer, SigningKey};
use zeroize::Zeroizing;

mod protect;

pub use protect::{PlainProtector, Protector, SystemProtector};

/// Nome do arquivo da chave dentro do diretório de dados do app.
pub const KEY_FILE: &str = "device-key.bin";

/// Cabeçalho do arquivo (versão do formato), antes do conteúdo protegido.
const MAGIC: &[u8; 8] = b"KRDKEY01";

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("falha de E/S em {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("arquivo de chave inválido ou corrompido")]
    Corrupt,
    #[error("falha ao proteger/desproteger a chave: {0}")]
    Protect(String),
    #[error("falha ao gerar bytes aleatórios: {0}")]
    Random(String),
}

/// Par de chaves do dispositivo. A chave privada nunca sai deste tipo.
pub struct DeviceIdentity {
    key: SigningKey,
}

impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("public_key", &self.public_key_b64())
            .finish_non_exhaustive()
    }
}

impl DeviceIdentity {
    /// Gera uma identidade nova com o RNG do sistema.
    pub fn generate() -> Result<Self, IdentityError> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut_slice()).map_err(|e| IdentityError::Random(e.to_string()))?;
        Ok(Self::from_seed(&seed))
    }

    /// Reconstrói a identidade a partir da semente (32 bytes).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(seed),
        }
    }

    /// Chave pública em base64url sem padding (formato do protocolo).
    pub fn public_key_b64(&self) -> String {
        B64.encode(self.key.verifying_key().as_bytes())
    }

    /// Assina o desafio de login (`AUTH_CONTEXT + nonce`), em base64url.
    pub fn sign_auth(&self, nonce: &str) -> String {
        let sig = self.key.sign(&kr_proto::auth::auth_message(nonce));
        B64.encode(sig.to_bytes())
    }

    /// Carrega a identidade de `dir`, ou cria e salva uma nova se não existir.
    pub fn load_or_create(dir: &Path, protector: &dyn Protector) -> Result<Self, IdentityError> {
        let path = dir.join(KEY_FILE);
        match fs::read(&path) {
            Ok(bytes) => Self::decode(&bytes, protector),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let identity = Self::generate()?;
                identity.save(dir, protector)?;
                Ok(identity)
            }
            Err(source) => Err(IdentityError::Io { path, source }),
        }
    }

    /// Grava a chave protegida em `dir` (escrita atômica via arquivo temporário).
    pub fn save(&self, dir: &Path, protector: &dyn Protector) -> Result<(), IdentityError> {
        let io_err = |path: &Path| {
            let path = path.to_path_buf();
            move |source| IdentityError::Io { path, source }
        };
        fs::create_dir_all(dir).map_err(io_err(dir))?;

        let seed = Zeroizing::new(self.key.to_bytes());
        let sealed = protector.protect(seed.as_slice())?;
        let mut out = Vec::with_capacity(MAGIC.len() + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&sealed);

        let path = dir.join(KEY_FILE);
        let tmp = dir.join(format!("{KEY_FILE}.tmp"));
        fs::write(&tmp, &out).map_err(io_err(&tmp))?;
        fs::rename(&tmp, &path).map_err(io_err(&path))?;
        Ok(())
    }

    fn decode(bytes: &[u8], protector: &dyn Protector) -> Result<Self, IdentityError> {
        let sealed = bytes.strip_prefix(MAGIC).ok_or(IdentityError::Corrupt)?;
        let seed = Zeroizing::new(protector.unprotect(sealed)?);
        let seed: &[u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::Corrupt)?;
        Ok(Self::from_seed(seed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kr-identity-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Mesmo vetor usado em kr-proto e no servidor (fixtures/auth-vector.json).
    #[test]
    fn signs_shared_vector() {
        let text = include_str!("../../kr-proto/fixtures/auth-vector.json");
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        let seed: [u8; 32] = (1..=32).collect::<Vec<u8>>().try_into().unwrap();
        let id = DeviceIdentity::from_seed(&seed);
        assert_eq!(id.public_key_b64(), v["public_key"].as_str().unwrap());
        assert_eq!(
            id.sign_auth(v["nonce"].as_str().unwrap()),
            v["signature"].as_str().unwrap()
        );
    }

    #[test]
    fn load_or_create_persists_the_same_key() {
        let dir = temp_dir("persist");
        let p = PlainProtector;
        let a = DeviceIdentity::load_or_create(&dir, &p).unwrap();
        let b = DeviceIdentity::load_or_create(&dir, &p).unwrap();
        assert_eq!(a.public_key_b64(), b.public_key_b64());
        assert!(!dir.join(format!("{KEY_FILE}.tmp")).exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_corrupt_file() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(KEY_FILE), b"lixo").unwrap();
        let err = DeviceIdentity::load_or_create(&dir, &PlainProtector).unwrap_err();
        assert!(matches!(err, IdentityError::Corrupt));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn generated_keys_are_unique_and_43_chars() {
        let a = DeviceIdentity::generate().unwrap();
        let b = DeviceIdentity::generate().unwrap();
        assert_ne!(a.public_key_b64(), b.public_key_b64());
        assert_eq!(a.public_key_b64().len(), 43);
        assert_eq!(a.sign_auth("n").len(), 86);
    }

    #[test]
    fn system_protector_roundtrip() {
        let p = SystemProtector;
        let sealed = p.protect(b"segredo de teste").unwrap();
        assert_eq!(
            p.unprotect(&sealed).unwrap().as_slice(),
            b"segredo de teste"
        );
    }

    #[test]
    fn debug_does_not_leak_private_key() {
        let id = DeviceIdentity::from_seed(&[7u8; 32]);
        let dbg = format!("{id:?}");
        assert!(dbg.contains("public_key"));
        assert!(!dbg.contains("[7, 7"));
    }
}

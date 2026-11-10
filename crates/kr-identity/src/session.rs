//! Assinatura e verificação da sessão P2P, código de verificação e
//! dispositivos conhecidos (TOFU — confiança no primeiro uso).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use kr_proto::session::{Role, SESSION_CONTEXT, sas_input, session_message};
use sha2::{Digest, Sha256};

use crate::DeviceIdentity;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionAuthError {
    #[error("chave pública inválida")]
    BadKey,
    #[error("assinatura inválida")]
    BadSignature,
    #[error("mensagem fora do formato de sessão")]
    BadMessage,
    #[error("a chave não é a que o servidor informou para este computador")]
    KeyMismatch,
}

fn is_session_message(message: &str) -> bool {
    message
        .strip_prefix(SESSION_CONTEXT)
        .is_some_and(|rest| rest.starts_with('\n'))
}

impl DeviceIdentity {
    /// Assina os fingerprints que este lado vê (`local`, `remote`).
    pub fn sign_session(&self, request_id: &str, role: Role, local: &str, remote: &str) -> String {
        let message = session_message(request_id, role, local, remote);
        B64.encode(self.key.sign(message.as_bytes()).to_bytes())
    }

    /// Assina uma mensagem de sessão já montada (viewer: montada pela
    /// interface). Recusa qualquer coisa fora do contexto de sessão, para que
    /// a chave não possa ser usada para assinar outros dados (ex.: login).
    pub fn sign_session_message(&self, message: &str) -> Result<String, SessionAuthError> {
        if !is_session_message(message) {
            return Err(SessionAuthError::BadMessage);
        }
        Ok(B64.encode(self.key.sign(message.as_bytes()).to_bytes()))
    }
}

/// Verifica a assinatura `sig` (base64url) de `message` com a chave `pk`.
pub fn verify_session_message(pk: &str, sig: &str, message: &str) -> Result<(), SessionAuthError> {
    if !is_session_message(message) {
        return Err(SessionAuthError::BadMessage);
    }
    let key: [u8; 32] = B64
        .decode(pk)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(SessionAuthError::BadKey)?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| SessionAuthError::BadKey)?;
    let sig: [u8; 64] = B64
        .decode(sig)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(SessionAuthError::BadSignature)?;
    key.verify_strict(message.as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|_| SessionAuthError::BadSignature)
}

/// Verifica a prova do outro lado: ele assinou o fingerprint dele
/// (`peer_fp`, como **nós** o vemos) e o nosso (`local_fp`).
pub fn verify_peer(
    pk: &str,
    sig: &str,
    request_id: &str,
    peer_role: Role,
    peer_fp: &str,
    local_fp: &str,
) -> Result<(), SessionAuthError> {
    verify_session_message(
        pk,
        sig,
        &session_message(request_id, peer_role, peer_fp, local_fp),
    )
}

/// Código de verificação de 6 dígitos (igual nos dois lados sem MITM).
pub fn verification_code(request_id: &str, fp_a: &str, fp_b: &str) -> String {
    let digest = Sha256::digest(sas_input(request_id, fp_a, fp_b).as_bytes());
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{n:06}")
}

/// Resultado da checagem TOFU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Primeira vez: a chave foi memorizada.
    New,
    /// Mesma chave das conexões anteriores.
    Known,
    /// A chave mudou desde a última conexão (reinstalação ou ataque).
    Changed,
}

/// Chaves públicas dos computadores já conectados, por ID.
#[derive(Debug)]
pub struct KnownPeers {
    path: PathBuf,
    peers: BTreeMap<String, String>,
}

impl KnownPeers {
    pub const FILE_NAME: &str = "known-peers.json";

    /// Carrega de `dir` (arquivo ausente ou ilegível = lista vazia).
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(Self::FILE_NAME);
        let peers = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self { path, peers }
    }

    /// Compara `pk` com a chave memorizada para `peer_id` e memoriza na
    /// primeira vez. Não substitui uma chave diferente (use [`Self::forget`]).
    pub fn check(&mut self, peer_id: &str, pk: &str) -> Trust {
        match self.peers.get(peer_id) {
            Some(known) if known == pk => Trust::Known,
            Some(_) => Trust::Changed,
            None => {
                self.peers.insert(peer_id.to_owned(), pk.to_owned());
                self.save();
                Trust::New
            }
        }
    }

    /// Esquece a chave de `peer_id` (o usuário confirmou a troca).
    pub fn forget(&mut self, peer_id: &str) {
        if self.peers.remove(peer_id).is_some() {
            self.save();
        }
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.peers) {
            let tmp = self.path.with_extension("tmp");
            if fs::write(&tmp, text).is_ok() {
                let _ = fs::rename(&tmp, &self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector() -> serde_json::Value {
        let text = include_str!("../../kr-proto/fixtures/session-vector.json");
        serde_json::from_str(text).unwrap()
    }

    fn seed_identity() -> DeviceIdentity {
        let seed: [u8; 32] = (1..=32).collect::<Vec<u8>>().try_into().unwrap();
        DeviceIdentity::from_seed(&seed)
    }

    #[test]
    fn assinatura_e_codigo_batem_com_o_vetor() {
        let v = vector();
        let s = |k: &str| v[k].as_str().unwrap().to_owned();
        let id = seed_identity();
        let sig = id.sign_session(&s("request_id"), Role::Host, &s("host_fp"), &s("viewer_fp"));
        assert_eq!(id.public_key_b64(), s("host_public_key"));
        assert_eq!(sig, s("host_signature"));
        assert_eq!(
            verification_code(&s("request_id"), &s("host_fp"), &s("viewer_fp")),
            s("sas")
        );
        // O viewer verifica com o fingerprint do host como ele o vê.
        verify_peer(
            &s("host_public_key"),
            &sig,
            &s("request_id"),
            Role::Host,
            &s("host_fp"),
            &s("viewer_fp"),
        )
        .unwrap();
    }

    #[test]
    fn intermediario_e_detectado() {
        let v = vector();
        let s = |k: &str| v[k].as_str().unwrap().to_owned();
        let id = seed_identity();
        let sig = id.sign_session(&s("request_id"), Role::Host, &s("host_fp"), &s("viewer_fp"));
        // Com um MITM, o viewer vê o fingerprint do intermediário.
        let mitm = "sha-256 AA:AA";
        assert_eq!(
            verify_peer(
                &id.public_key_b64(),
                &sig,
                &s("request_id"),
                Role::Host,
                mitm,
                &s("viewer_fp")
            ),
            Err(SessionAuthError::BadSignature)
        );
        // Papel trocado também não vale.
        assert!(
            verify_peer(
                &id.public_key_b64(),
                &sig,
                &s("request_id"),
                Role::Viewer,
                &s("host_fp"),
                &s("viewer_fp")
            )
            .is_err()
        );
        // E o código de verificação muda.
        assert_ne!(
            verification_code(&s("request_id"), mitm, &s("viewer_fp")),
            verification_code(&s("request_id"), &s("host_fp"), &s("viewer_fp"))
        );
    }

    #[test]
    fn so_assina_mensagens_de_sessao() {
        let id = seed_identity();
        assert_eq!(
            id.sign_session_message("kr-desk/auth/v1:nonce"),
            Err(SessionAuthError::BadMessage)
        );
        assert_eq!(
            id.sign_session_message("kr-desk/session/v1x\n"),
            Err(SessionAuthError::BadMessage)
        );
        let msg = session_message("r", Role::Viewer, "a", "b");
        let sig = id.sign_session_message(&msg).unwrap();
        verify_session_message(&id.public_key_b64(), &sig, &msg).unwrap();
        assert_eq!(
            verify_session_message("curta", &sig, &msg),
            Err(SessionAuthError::BadKey)
        );
    }

    #[test]
    fn tofu_memoriza_detecta_troca_e_esquece() {
        let dir = std::env::temp_dir().join(format!("kr-known-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut k = KnownPeers::load(&dir);
        assert_eq!(k.check("123456789", "A"), Trust::New);
        assert_eq!(k.check("123456789", "A"), Trust::Known);
        // Persistiu em disco.
        let mut k = KnownPeers::load(&dir);
        assert_eq!(k.check("123456789", "B"), Trust::Changed);
        assert_eq!(k.check("123456789", "A"), Trust::Known);
        k.forget("123456789");
        assert_eq!(k.check("123456789", "B"), Trust::New);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "gera os valores do vetor"]
    fn imprime_vetor() {
        let v = vector();
        let s = |k: &str| v[k].as_str().unwrap().to_owned();
        let id = seed_identity();
        println!("pk={}", id.public_key_b64());
        println!(
            "sig={}",
            id.sign_session(&s("request_id"), Role::Host, &s("host_fp"), &s("viewer_fp"))
        );
        println!(
            "sas={}",
            verification_code(&s("request_id"), &s("host_fp"), &s("viewer_fp"))
        );
    }
}

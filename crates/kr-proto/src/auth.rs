//! Formato da prova de identidade usada no `auth`.
//!
//! O cliente assina `AUTH_CONTEXT || nonce` (bytes UTF-8) com a chave Ed25519
//! do dispositivo. O prefixo de contexto impede que uma assinatura feita para
//! outro propósito seja reaproveitada como login.

/// Prefixo de domínio da mensagem assinada no login.
pub const AUTH_CONTEXT: &str = "kr-desk/auth/v1:";

/// Bytes exatos que devem ser assinados para o `nonce` recebido no `hello`.
pub fn auth_message(nonce: &str) -> Vec<u8> {
    let mut msg = Vec::with_capacity(AUTH_CONTEXT.len() + nonce.len());
    msg.extend_from_slice(AUTH_CONTEXT.as_bytes());
    msg.extend_from_slice(nonce.as_bytes());
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    use ed25519_dalek::{Signer, SigningKey, Verifier};

    /// O mesmo vetor é verificado pelo servidor (apps/server/test/auth.test.ts).
    #[test]
    fn rust_signature_matches_shared_vector() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/auth-vector.json"
        ))
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let s = |k: &str| v[k].as_str().unwrap().to_owned();

        let seed: [u8; 32] = (1..=32).collect::<Vec<u8>>().try_into().unwrap();
        assert_eq!(s("seed_hex").len(), 64);
        let key = SigningKey::from_bytes(&seed);

        assert_eq!(B64.encode(key.verifying_key().as_bytes()), s("public_key"));

        let msg = auth_message(&s("nonce"));
        let sig = key.sign(&msg);
        assert_eq!(B64.encode(sig.to_bytes()), s("signature"));
        assert!(key.verifying_key().verify(&msg, &sig).is_ok());
    }
}

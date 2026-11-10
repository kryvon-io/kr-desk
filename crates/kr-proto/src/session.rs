//! Autenticação ponta a ponta da sessão P2P.
//!
//! O servidor de sinalização repassa as ofertas SDP, então um servidor
//! malicioso poderia se colocar no meio (MITM) trocando os fingerprints DTLS.
//! Para impedir isso, cada lado assina com a chave Ed25519 do dispositivo os
//! fingerprints que **ele mesmo** vê — o seu e o do outro — e manda pelo
//! DataChannel de controle (já cifrado). Com um intermediário, os fingerprints
//! vistos por cada lado não batem e a verificação falha.
//!
//! O código de verificação (6 dígitos) é derivado dos dois fingerprints: é
//! igual nas duas telas só quando não há ninguém no meio — útil na primeira
//! conexão, antes de a chave do outro computador ser conhecida (TOFU).

/// Prefixo de domínio das mensagens assinadas na sessão. Diferente do login
/// (`kr-desk/auth/v1:`), então uma assinatura não serve para a outra.
pub const SESSION_CONTEXT: &str = "kr-desk/session/v1";
/// Prefixo da entrada do código de verificação.
pub const SAS_CONTEXT: &str = "kr-desk/sas/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Host => "host",
            Role::Viewer => "viewer",
        }
    }
}

/// Fingerprint DTLS do SDP (`a=fingerprint:sha-256 AB:CD:...`), normalizado
/// como `sha-256 AB:CD:...` (algoritmo minúsculo, hex maiúsculo).
pub fn sdp_fingerprint(sdp: &str) -> Option<String> {
    sdp.lines().find_map(|line| {
        let value = line.trim().strip_prefix("a=fingerprint:")?;
        let (algo, hex) = value.trim().split_once(' ')?;
        Some(format!(
            "{} {}",
            algo.trim().to_ascii_lowercase(),
            hex.trim().to_ascii_uppercase()
        ))
    })
}

/// Bytes assinados por `signer`: contexto, pedido, papel, fingerprint de
/// quem assina e o do outro lado, separados por `\n`.
pub fn session_message(
    request_id: &str,
    signer: Role,
    signer_fingerprint: &str,
    peer_fingerprint: &str,
) -> String {
    format!(
        "{SESSION_CONTEXT}\n{request_id}\n{}\n{signer_fingerprint}\n{peer_fingerprint}",
        signer.as_str()
    )
}

/// Entrada do hash do código de verificação (independe de qual lado calcula).
pub fn sas_input(request_id: &str, fingerprint_a: &str, fingerprint_b: &str) -> String {
    let (lo, hi) = if fingerprint_a <= fingerprint_b {
        (fingerprint_a, fingerprint_b)
    } else {
        (fingerprint_b, fingerprint_a)
    };
    format!("{SAS_CONTEXT}\n{request_id}\n{lo}\n{hi}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector() -> serde_json::Value {
        let text = include_str!("../fixtures/session-vector.json");
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn extrai_fingerprint_normalizado() {
        let sdp = "v=0\r\na=group:BUNDLE 0\r\na=fingerprint:SHA-256 ab:cd:01\r\na=setup:active\r\n";
        assert_eq!(sdp_fingerprint(sdp).as_deref(), Some("sha-256 AB:CD:01"));
        assert_eq!(sdp_fingerprint("v=0\r\n"), None);
    }

    #[test]
    fn mensagens_batem_com_o_vetor() {
        let v = vector();
        let s = |k: &str| v[k].as_str().unwrap().to_owned();
        assert_eq!(sdp_fingerprint(&s("host_sdp")).unwrap(), s("host_fp"));
        assert_eq!(sdp_fingerprint(&s("viewer_sdp")).unwrap(), s("viewer_fp"));
        assert_eq!(
            session_message(&s("request_id"), Role::Host, &s("host_fp"), &s("viewer_fp")),
            s("host_message")
        );
        assert_eq!(
            session_message(
                &s("request_id"),
                Role::Viewer,
                &s("viewer_fp"),
                &s("host_fp")
            ),
            s("viewer_message")
        );
        // Ordem dos fingerprints não importa.
        assert_eq!(
            sas_input(&s("request_id"), &s("host_fp"), &s("viewer_fp")),
            sas_input(&s("request_id"), &s("viewer_fp"), &s("host_fp"))
        );
    }
}

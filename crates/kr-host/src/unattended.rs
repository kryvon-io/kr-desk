//! Acesso não assistido: senha definida no host para entrar sem ninguém
//! aceitar o pedido (inclusive na tela de login, com o serviço).
//!
//! - A senha nunca passa pelo servidor: o viewer só avisa que tem uma
//!   (`unattended` no pedido) e a envia pelo canal P2P cifrado, depois de o
//!   host comprovar a identidade.
//! - No disco fica só um hash Argon2id com sal aleatório
//!   (`unattended.json`), na pasta de dados do host.
//! - Tentativas erradas são limitadas: depois de [`MAX_FAILURES`] em
//!   [`FAILURE_WINDOW`], tudo é recusado até a janela passar.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD as B64;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

const FILE: &str = "unattended.json";
pub const MIN_PASSWORD_CHARS: usize = 8;
pub const MAX_FAILURES: usize = 5;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Custo do Argon2id (recomendação OWASP: 19 MiB, 2 passagens).
const M_COST_KIB: u32 = 19 * 1024;
const T_COST: u32 = 2;
const HASH_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    /// Formato do arquivo.
    v: u32,
    m_kib: u32,
    t: u32,
    salt: String,
    hash: String,
}

fn derive(password: &str, salt: &[u8], m_kib: u32, t: u32) -> Result<Vec<u8>, String> {
    let params = Params::new(m_kib, t, 1, Some(HASH_LEN)).map_err(|e| e.to_string())?;
    let mut out = vec![0u8; HASH_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Comparação em tempo constante.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Senha válida? Devolve o motivo em português se não for.
pub fn validate_password(password: &str) -> Result<(), String> {
    let n = password.chars().count();
    if n < MIN_PASSWORD_CHARS {
        return Err(format!(
            "A senha precisa ter pelo menos {MIN_PASSWORD_CHARS} caracteres."
        ));
    }
    if n > kr_proto::control::MAX_PASSWORD_CHARS {
        return Err("Senha longa demais.".into());
    }
    Ok(())
}

/// Configuração de acesso não assistido de um host.
pub struct Unattended {
    path: Option<PathBuf>,
    stored: Mutex<Option<Stored>>,
    failures: Mutex<VecDeque<Instant>>,
}

impl Unattended {
    /// Lê a configuração de `dir` (sem arquivo = desativado).
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let stored = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Stored>(&bytes) {
                Ok(s) => Some(s),
                Err(err) => {
                    warn!(%err, "configuração de acesso não assistido ilegível; desativado");
                    None
                }
            },
            Err(_) => None,
        };
        Self {
            path: Some(path),
            stored: Mutex::new(stored),
            failures: Mutex::default(),
        }
    }

    /// Sem disco (testes).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            stored: Mutex::default(),
            failures: Mutex::default(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.stored.lock().expect("mutex").is_some()
    }

    fn save(&self, stored: Option<&Stored>) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let result = match stored {
            Some(s) => {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, serde_json::to_vec_pretty(s).expect("serializável"))
                    .and_then(|()| std::fs::rename(&tmp, path))
            }
            None => match std::fs::remove_file(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        result.map_err(|e| format!("Não foi possível salvar a configuração: {e}"))
    }

    /// Define (ou troca) a senha e ativa o acesso não assistido.
    pub fn set_password(&self, password: &str) -> Result<(), String> {
        validate_password(password)?;
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|e| format!("sem aleatoriedade: {e}"))?;
        let hash = derive(password, &salt, M_COST_KIB, T_COST)?;
        let stored = Stored {
            v: 1,
            m_kib: M_COST_KIB,
            t: T_COST,
            salt: B64.encode(salt),
            hash: B64.encode(hash),
        };
        self.save(Some(&stored))?;
        *self.stored.lock().expect("mutex") = Some(stored);
        self.failures.lock().expect("mutex").clear();
        info!("acesso não assistido ativado (senha definida)");
        Ok(())
    }

    /// Desativa o acesso não assistido (apaga o hash).
    pub fn disable(&self) -> Result<(), String> {
        self.save(None)?;
        *self.stored.lock().expect("mutex") = None;
        info!("acesso não assistido desativado");
        Ok(())
    }

    /// Confere a senha (lento de propósito: rode fora do runtime).
    pub fn verify(&self, password: &str) -> Result<(), String> {
        self.verify_at(password, Instant::now())
    }

    fn verify_at(&self, password: &str, now: Instant) -> Result<(), String> {
        let stored = self
            .stored
            .lock()
            .expect("mutex")
            .clone()
            .ok_or("O acesso não assistido está desativado neste computador.")?;
        {
            let mut failures = self.failures.lock().expect("mutex");
            while failures
                .front()
                .is_some_and(|t| now.duration_since(*t) > FAILURE_WINDOW)
            {
                failures.pop_front();
            }
            if failures.len() >= MAX_FAILURES {
                return Err(
                    "Muitas tentativas com senha errada. Tente de novo em alguns minutos.".into(),
                );
            }
        }
        let salt = B64.decode(&stored.salt).map_err(|e| e.to_string())?;
        let expected = B64.decode(&stored.hash).map_err(|e| e.to_string())?;
        let got = derive(password, &salt, stored.m_kib, stored.t)?;
        if same(&got, &expected) {
            self.failures.lock().expect("mutex").clear();
            Ok(())
        } else {
            self.failures.lock().expect("mutex").push_back(now);
            warn!("senha de acesso não assistido incorreta");
            Err("Senha incorreta.".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn senha_certa_passa_e_errada_nao() {
        let u = Unattended::in_memory();
        assert!(!u.enabled());
        assert!(u.verify("qualquer").is_err());
        u.set_password("senha-forte-1").unwrap();
        assert!(u.enabled());
        u.verify("senha-forte-1").unwrap();
        assert_eq!(u.verify("senha-forte-2").unwrap_err(), "Senha incorreta.");
        u.disable().unwrap();
        assert!(u.verify("senha-forte-1").is_err());
    }

    #[test]
    fn senha_curta_e_recusada() {
        let u = Unattended::in_memory();
        assert!(u.set_password("1234567").is_err());
        assert!(u.set_password(&"a".repeat(129)).is_err());
        assert!(!u.enabled());
    }

    #[test]
    fn tentativas_erradas_bloqueiam_por_um_tempo() {
        let u = Unattended::in_memory();
        u.set_password("senha-forte-1").unwrap();
        let t0 = Instant::now();
        for _ in 0..MAX_FAILURES {
            assert_eq!(u.verify_at("errada!!", t0).unwrap_err(), "Senha incorreta.");
        }
        // Bloqueado: nem a senha certa passa.
        assert!(
            u.verify_at("senha-forte-1", t0)
                .unwrap_err()
                .contains("Muitas tentativas")
        );
        // Passada a janela, volta a aceitar.
        u.verify_at(
            "senha-forte-1",
            t0 + FAILURE_WINDOW + Duration::from_secs(1),
        )
        .unwrap();
    }

    #[test]
    fn guarda_so_o_hash_no_disco() {
        let dir = std::env::temp_dir().join(format!("kr-unattended-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let u = Unattended::load(&dir);
        u.set_password("minha senha secreta").unwrap();
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(!text.contains("secreta"));
        // Outra instância (reinício) lê a mesma configuração.
        Unattended::load(&dir)
            .verify("minha senha secreta")
            .unwrap();
        u.disable().unwrap();
        assert!(!dir.join(FILE).exists());
        assert!(!Unattended::load(&dir).enabled());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

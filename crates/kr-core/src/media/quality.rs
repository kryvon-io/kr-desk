//! Controle de qualidade adaptativo do host.
//!
//! O viewer manda, a cada segundo, quantos pacotes de vídeo recebeu e
//! perdeu e o RTT. O controle segue o modelo AIMD (aumento aditivo,
//! redução multiplicativa): perda ou RTT subindo → reduz a taxa na hora;
//! rede limpa por alguns segundos → aumenta devagar. Com taxa baixa, reduz
//! também a resolução, que rende mais nitidez por bit do que 1080p borrado.

use std::collections::VecDeque;

/// Faixas de resolução máxima, da maior para a menor.
pub const LEVELS: [(u32, u32); 3] = [(1920, 1080), (1280, 720), (960, 540)];
/// Taxa mínima para usar cada faixa (desce abaixo disso).
const LEVEL_MIN_BPS: [u32; 3] = [2_500_000, 1_000_000, 0];
/// Margem para voltar a uma faixa maior (evita alternar sem parar).
const UPGRADE_MARGIN: f64 = 1.3;
/// Perda acima disso indica congestionamento.
const LOSS_CONGESTED: f64 = 0.05;
const LOSS_MILD: f64 = 0.02;
/// Relatórios limpos seguidos antes de aumentar.
const GOOD_STREAK: u32 = 3;
/// Janela (em relatórios) para o RTT de referência.
const RTT_WINDOW: usize = 30;
/// Mudança mínima de taxa para valer a pena reconfigurar o encoder.
const MIN_CHANGE: f64 = 0.05;

/// Relatório de recepção de um intervalo (diferenças, não totais).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RxReport {
    pub lost: u32,
    pub received: u32,
    pub rtt_ms: u32,
}

/// Configuração pedida ao pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityTarget {
    pub bitrate_bps: u32,
    pub max_width: u32,
    pub max_height: u32,
}

#[derive(Debug, Clone)]
pub struct QualityController {
    min_bps: u32,
    max_bps: u32,
    bitrate: f64,
    applied_bps: u32,
    level: usize,
    good: u32,
    rtts: VecDeque<u32>,
}

impl QualityController {
    pub fn new(start_bps: u32, min_bps: u32, max_bps: u32) -> Self {
        let start = start_bps.clamp(min_bps, max_bps);
        Self {
            min_bps,
            max_bps,
            bitrate: start as f64,
            applied_bps: start,
            level: level_for(start, 0),
            good: 0,
            rtts: VecDeque::new(),
        }
    }

    pub fn target(&self) -> QualityTarget {
        let (w, h) = LEVELS[self.level];
        QualityTarget {
            bitrate_bps: self.applied_bps,
            max_width: w,
            max_height: h,
        }
    }

    /// Processa um relatório. Devolve a nova configuração se ela mudou.
    pub fn on_report(&mut self, r: RxReport) -> Option<QualityTarget> {
        let total = r.lost.saturating_add(r.received);
        if total == 0 {
            return None; // tela parada: sem pacotes, sem informação
        }
        let loss = r.lost as f64 / total as f64;
        let baseline = self.rtts.iter().copied().min();
        self.rtts.push_back(r.rtt_ms);
        if self.rtts.len() > RTT_WINDOW {
            self.rtts.pop_front();
        }
        let rtt_rising = baseline.is_some_and(|b| r.rtt_ms > b.saturating_mul(2) + 40);

        if loss > LOSS_CONGESTED || rtt_rising {
            self.bitrate *= 0.7;
            self.good = 0;
        } else if loss > LOSS_MILD {
            self.bitrate *= 0.9;
            self.good = 0;
        } else {
            self.good += 1;
            if self.good >= GOOD_STREAK {
                // Aumento aditivo: ~8% da taxa máxima por passo.
                self.bitrate += self.max_bps as f64 * 0.08;
            }
        }
        self.bitrate = self.bitrate.clamp(self.min_bps as f64, self.max_bps as f64);

        let before = self.target();
        let candidate = self.bitrate.round() as u32;
        let relative = (candidate as f64 - self.applied_bps as f64).abs() / self.applied_bps as f64;
        if relative >= MIN_CHANGE || candidate == self.min_bps || candidate == self.max_bps {
            self.applied_bps = candidate;
        }
        self.level = level_for(self.applied_bps, self.level);
        let after = self.target();
        (after != before).then_some(after)
    }
}

/// Faixa de resolução para a taxa, com histerese para subir.
fn level_for(bps: u32, current: usize) -> usize {
    let mut level = current;
    // Desce enquanto a taxa não sustentar a faixa atual.
    while level + 1 < LEVELS.len() && bps < LEVEL_MIN_BPS[level] {
        level += 1;
    }
    // Sobe só com folga.
    while level > 0 && bps as f64 >= LEVEL_MIN_BPS[level - 1] as f64 * UPGRADE_MARGIN {
        level -= 1;
    }
    level
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(lost: u32, received: u32, rtt_ms: u32) -> RxReport {
        RxReport {
            lost,
            received,
            rtt_ms,
        }
    }

    #[test]
    fn perda_alta_reduz_na_hora_e_desce_resolucao() {
        let mut q = QualityController::new(5_000_000, 400_000, 8_000_000);
        assert_eq!(q.target().max_width, 1920);
        let t = q.on_report(report(10, 90, 20)).unwrap();
        assert_eq!(t.bitrate_bps, 3_500_000);
        assert_eq!(t.max_width, 1920);
        // Perdas seguidas levam a 720p e depois 540p.
        let mut last = t;
        for _ in 0..6 {
            if let Some(t) = q.on_report(report(10, 90, 20)) {
                last = t;
            }
        }
        assert_eq!((last.max_width, last.max_height), (960, 540));
        assert!(last.bitrate_bps >= 400_000);
    }

    #[test]
    fn rede_limpa_sobe_devagar_ate_o_maximo() {
        let mut q = QualityController::new(1_000_000, 400_000, 8_000_000);
        assert_eq!(q.target().max_width, 1280);
        // Dois relatórios limpos não mudam nada; o terceiro aumenta.
        assert!(q.on_report(report(0, 100, 20)).is_none());
        assert!(q.on_report(report(0, 100, 20)).is_none());
        let t = q.on_report(report(0, 100, 20)).unwrap();
        assert_eq!(t.bitrate_bps, 1_640_000);
        for _ in 0..200 {
            q.on_report(report(0, 100, 20));
        }
        assert_eq!(q.target().bitrate_bps, 8_000_000);
        assert_eq!(q.target().max_width, 1920);
    }

    #[test]
    fn rtt_subindo_reduz_sem_perda() {
        let mut q = QualityController::new(5_000_000, 400_000, 8_000_000);
        q.on_report(report(0, 100, 20));
        let t = q.on_report(report(0, 100, 200)).unwrap();
        assert_eq!(t.bitrate_bps, 3_500_000);
    }

    #[test]
    fn sem_pacotes_nao_muda_e_histerese_segura_a_faixa() {
        let mut q = QualityController::new(2_400_000, 400_000, 8_000_000);
        assert!(q.on_report(report(0, 0, 20)).is_none());
        // 2,4 Mbps está em 720p; voltar a 1080p exige 2,5 × 1,3 = 3,25 Mbps.
        assert_eq!(level_for(2_400_000, 1), 1);
        assert_eq!(level_for(3_000_000, 1), 1);
        assert_eq!(level_for(3_300_000, 1), 0);
        assert_eq!(level_for(900_000, 0), 2);
    }
}

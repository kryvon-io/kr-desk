//! Fonte de quadros sintética, para testes e para a demonstração sem DXGI.

use std::time::{Duration, Instant};

use super::{BgraFrame, CaptureError, FrameSource};

/// Gradiente com uma faixa vertical que anda a cada quadro.
pub struct TestPattern {
    frame: BgraFrame,
    interval: Duration,
    next: Instant,
    n: u32,
}

impl TestPattern {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            frame: BgraFrame::new(width, height),
            interval: Duration::from_secs(1) / fps.max(1),
            next: Instant::now(),
            n: 0,
        }
    }

    fn draw(&mut self) {
        let (w, h) = (self.frame.width, self.frame.height);
        let bar = (self.n * 8) % w;
        for y in 0..h {
            let row = &mut self.frame.data[y as usize * self.frame.stride..];
            for x in 0..w {
                let i = x as usize * 4;
                let on_bar = x >= bar && x < bar + w / 16;
                let (b, g, r) = if on_bar {
                    (255, 255, 255)
                } else {
                    ((x * 255 / w) as u8, (y * 255 / h) as u8, 96)
                };
                row[i..i + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        self.n = self.n.wrapping_add(1);
    }
}

impl FrameSource for TestPattern {
    fn capture(&mut self, timeout: Duration) -> Result<Option<&BgraFrame>, CaptureError> {
        let now = Instant::now();
        if self.next > now {
            let wait = self.next - now;
            if wait > timeout {
                std::thread::sleep(timeout);
                return Ok(None);
            }
            std::thread::sleep(wait);
        }
        self.next = Instant::now().max(self.next) + self.interval;
        self.draw();
        Ok(Some(&self.frame))
    }

    fn name(&self) -> String {
        "padrão de teste".into()
    }
}

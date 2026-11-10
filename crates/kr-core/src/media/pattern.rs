//! Fonte de quadros sintética, para testes e para a demonstração sem DXGI.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::{
    BgraFrame, CaptureError, CursorUpdate, DesktopRect, FrameSource, MonitorInfo, SourceEvent,
};

/// Gradiente com uma faixa vertical que anda a cada quadro. Simula um ou
/// mais monitores (lado a lado) e um cursor.
pub struct TestPattern {
    frame: BgraFrame,
    interval: Duration,
    next: Instant,
    n: u32,
    monitors: Vec<MonitorInfo>,
    active: u32,
    events: VecDeque<SourceEvent>,
}

impl TestPattern {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self::with_monitors(&[(width, height)], fps)
    }

    /// Vários monitores lado a lado; o primeiro é o principal.
    pub fn with_monitors(sizes: &[(u32, u32)], fps: u32) -> Self {
        let mut left = 0i32;
        let monitors: Vec<MonitorInfo> = sizes
            .iter()
            .enumerate()
            .map(|(i, &(width, height))| {
                let m = MonitorInfo {
                    index: i as u32,
                    name: format!("TESTE{}", i + 1),
                    rect: DesktopRect {
                        left,
                        top: 0,
                        width,
                        height,
                    },
                    primary: i == 0,
                };
                left += width as i32;
                m
            })
            .collect();
        let (w, h) = sizes.first().copied().unwrap_or((64, 64));
        let mut pattern = Self {
            frame: BgraFrame::new(w, h),
            interval: Duration::from_secs(1) / fps.max(1),
            next: Instant::now(),
            n: 0,
            monitors,
            active: 0,
            events: VecDeque::new(),
        };
        pattern
            .events
            .push_back(SourceEvent::Cursor(CursorUpdate::Shape {
                width: 2,
                height: 2,
                hot_x: 0,
                hot_y: 0,
                rgba: [255, 0, 0, 255].repeat(4),
            }));
        pattern.push_monitors();
        pattern
    }

    fn push_monitors(&mut self) {
        self.events.push_back(SourceEvent::Monitors {
            list: self.monitors.clone(),
            active: self.active,
        });
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

    /// Um cursor 2×2 vermelho, uma única vez.
    fn take_event(&mut self) -> Option<SourceEvent> {
        self.events.pop_front()
    }

    fn select_monitor(&mut self, index: u32) -> Result<(), CaptureError> {
        let monitor = self
            .monitors
            .iter()
            .find(|m| m.index == index)
            .ok_or_else(|| CaptureError::Transient(format!("monitor {index} não existe")))?;
        self.frame = BgraFrame::new(monitor.rect.width, monitor.rect.height);
        self.active = index;
        self.push_monitors();
        Ok(())
    }
}

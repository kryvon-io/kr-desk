//! Mede a conversão BGRA → NV12: `cargo run --release -p kr-core --example convert_bench`.
use kr_core::media::{BgraFrame, Nv12Frame, bgra_to_nv12};
use std::time::Instant;

fn main() {
    for (sw, sh, dw, dh) in [
        (1920, 1080, 1920, 1080),
        (2560, 1440, 1920, 1080),
        (3840, 2160, 1920, 1080),
    ] {
        let src = BgraFrame::new(sw, sh);
        let mut dst = Nv12Frame::new(dw, dh);
        let n = 30;
        let t = Instant::now();
        for _ in 0..n {
            bgra_to_nv12(&src, &mut dst);
        }
        println!(
            "{sw}x{sh} -> {dw}x{dh}: {:.1} ms/quadro",
            t.elapsed().as_secs_f64() * 1000.0 / n as f64
        );
    }
}

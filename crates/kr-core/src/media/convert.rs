//! Conversão BGRA → NV12 (BT.601, faixa limitada) com redimensionamento
//! bilinear opcional, feita na CPU.
//!
//! É o caminho simples do MVP. A Etapa 7 move a conversão para a GPU
//! (D3D11 Video Processor), sem copiar o quadro para a memória do sistema.

use super::{BgraFrame, Nv12Frame};

/// Maior tamanho com dimensões pares que cabe em `max_w × max_h`, mantendo a
/// proporção. Nunca aumenta a imagem.
pub fn fit_within(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let src_w = src_w.max(2);
    let src_h = src_h.max(2);
    let scale = f64::min(
        1.0,
        f64::min(max_w as f64 / src_w as f64, max_h as f64 / src_h as f64),
    );
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(src_w as f64 * scale), even(src_h as f64 * scale))
}

/// Posição de amostragem: interpola entre `i0` e `i1` com peso `w1/256`.
#[derive(Debug, Clone, Copy)]
struct Tap {
    i0: usize,
    i1: usize,
    w1: u32,
}

fn taps(src_len: u32, dst_len: u32) -> Vec<Tap> {
    let last = src_len as usize - 1;
    let ratio = src_len as f64 / dst_len as f64;
    (0..dst_len)
        .map(|d| {
            // Centro do pixel de destino projetado na origem.
            let pos = ((d as f64 + 0.5) * ratio - 0.5).clamp(0.0, last as f64);
            let i0 = pos.floor() as usize;
            Tap {
                i0,
                i1: (i0 + 1).min(last),
                w1: ((pos - i0 as f64) * 256.0).round() as u32,
            }
        })
        .collect()
}

#[inline]
fn lerp(a: u8, b: u8, w1: u32) -> u32 {
    (a as u32 * (256 - w1) + b as u32 * w1 + 128) >> 8
}

/// Amostra uma linha de destino em RGB (3 bytes por pixel).
fn sample_row(src: &BgraFrame, xs: &[Tap], ty: Tap, out: &mut [u8]) {
    let r0 = src.row(ty.i0 as u32);
    let r1 = src.row(ty.i1 as u32);
    let wy = ty.w1;
    for (x, tx) in xs.iter().enumerate() {
        let (a, b) = (tx.i0 * 4, tx.i1 * 4);
        let o = &mut out[x * 3..x * 3 + 3];
        for (c, px) in o.iter_mut().enumerate() {
            // BGRA → RGB: canal de saída c lê o canal 2 - c.
            let s = 2 - c;
            let top = lerp(r0[a + s], r0[b + s], tx.w1);
            let bottom = lerp(r1[a + s], r1[b + s], tx.w1);
            *px = ((top * (256 - wy) + bottom * wy + 128) >> 8) as u8;
        }
    }
}

#[inline]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

#[inline]
fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

/// Linhas de destino por bloco de trabalho (pares de linhas).
const MIN_PAIRS_PER_THREAD: usize = 32;
/// Threads usadas na conversão (o resto da CPU fica para o encoder).
const MAX_THREADS: usize = 4;

/// Converte `src` para `dst`, redimensionando para o tamanho de `dst`.
/// Divide o trabalho em faixas horizontais processadas em paralelo.
pub fn bgra_to_nv12(src: &BgraFrame, dst: &mut Nv12Frame) {
    assert!(src.width > 0 && src.height > 0, "quadro vazio");
    assert!(src.data.len() >= src.stride * (src.height as usize - 1) + src.width as usize * 4);
    let (dw, dh) = (dst.width as usize, dst.height as usize);
    let identity = src.width == dst.width && src.height == dst.height;
    let xs = if identity {
        Vec::new()
    } else {
        taps(src.width, dst.width)
    };
    let ys = if identity {
        Vec::new()
    } else {
        taps(src.height, dst.height)
    };
    let (y_plane, uv_plane) = dst.data.split_at_mut(dw * dh);

    let pairs = dh / 2;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, MAX_THREADS)
        .min(pairs.div_ceil(MIN_PAIRS_PER_THREAD).max(1));
    let per = pairs.div_ceil(threads);
    let y_chunks = y_plane.chunks_mut(per * 2 * dw);
    let uv_chunks = uv_plane.chunks_mut(per * dw);
    let work = |(i, (y_out, uv_out)): (usize, (&mut [u8], &mut [u8]))| {
        let first = i * per;
        let n = uv_out.len() / dw;
        if identity {
            convert_identity(src, first, n, y_out, uv_out);
        } else {
            convert_scaled(src, &xs, &ys, first, n, y_out, uv_out);
        }
    };
    if threads == 1 {
        y_chunks.zip(uv_chunks).enumerate().for_each(work);
    } else {
        std::thread::scope(|scope| {
            for item in y_chunks.zip(uv_chunks).enumerate() {
                scope.spawn(move || work(item));
            }
        });
    }
}

/// Mesmo tamanho: lê BGRA direto, sem buffer intermediário.
fn convert_identity(
    src: &BgraFrame,
    first_pair: usize,
    pairs: usize,
    y_out: &mut [u8],
    uv_out: &mut [u8],
) {
    let w = src.width as usize;
    for p in 0..pairs {
        let sy = (first_pair + p) * 2;
        let r0 = src.row(sy as u32);
        let r1 = src.row(sy as u32 + 1);
        let (ya, yb) = y_out[p * 2 * w..(p * 2 + 2) * w].split_at_mut(w);
        let uv = &mut uv_out[p * w..(p + 1) * w];
        for cx in 0..w / 2 {
            let mut sum = [0i32; 3];
            for dx in 0..2 {
                let x = cx * 2 + dx;
                let a = &r0[x * 4..x * 4 + 3];
                let b = &r1[x * 4..x * 4 + 3];
                ya[x] = luma(a[2] as i32, a[1] as i32, a[0] as i32);
                yb[x] = luma(b[2] as i32, b[1] as i32, b[0] as i32);
                sum[0] += a[2] as i32 + b[2] as i32;
                sum[1] += a[1] as i32 + b[1] as i32;
                sum[2] += a[0] as i32 + b[0] as i32;
            }
            let (u, v) = chroma((sum[0] + 2) / 4, (sum[1] + 2) / 4, (sum[2] + 2) / 4);
            uv[cx * 2] = u;
            uv[cx * 2 + 1] = v;
        }
    }
}

/// Tamanhos diferentes: amostragem bilinear para RGB e depois YUV.
fn convert_scaled(
    src: &BgraFrame,
    xs: &[Tap],
    ys: &[Tap],
    first_pair: usize,
    pairs: usize,
    y_out: &mut [u8],
    uv_out: &mut [u8],
) {
    let dw = xs.len();
    let mut rows = [vec![0u8; dw * 3], vec![0u8; dw * 3]];
    for p in 0..pairs {
        for (k, row) in rows.iter_mut().enumerate() {
            let y = (first_pair + p) * 2 + k;
            sample_row(src, xs, ys[y], row);
            let out = &mut y_out[(p * 2 + k) * dw..(p * 2 + k + 1) * dw];
            for (x, px) in out.iter_mut().enumerate() {
                let c = &row[x * 3..x * 3 + 3];
                *px = luma(c[0] as i32, c[1] as i32, c[2] as i32);
            }
        }
        let uv = &mut uv_out[p * dw..(p + 1) * dw];
        for cx in 0..dw / 2 {
            let mut sum = [0i32; 3];
            for row in &rows {
                let px = &row[cx * 6..cx * 6 + 6];
                sum[0] += px[0] as i32 + px[3] as i32;
                sum[1] += px[1] as i32 + px[4] as i32;
                sum[2] += px[2] as i32 + px[5] as i32;
            }
            let (u, v) = chroma((sum[0] + 2) / 4, (sum[1] + 2) / 4, (sum[2] + 2) / 4);
            uv[cx * 2] = u;
            uv[cx * 2 + 1] = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> BgraFrame {
        let mut f = BgraFrame::new(w, h);
        for px in f.data.chunks_exact_mut(4) {
            px.copy_from_slice(&bgra);
        }
        f
    }

    #[test]
    fn fit_within_mantem_proporcao_e_paridade() {
        assert_eq!(fit_within(1920, 1080, 1920, 1080), (1920, 1080));
        assert_eq!(fit_within(3840, 2160, 1920, 1080), (1920, 1080));
        assert_eq!(fit_within(2560, 1440, 1920, 1080), (1920, 1080));
        assert_eq!(fit_within(1366, 768, 1920, 1080), (1366, 768));
        // Ímpar é arredondado para baixo; nunca amplia.
        assert_eq!(fit_within(1025, 769, 1920, 1080), (1024, 768));
        // Ultrawide limitado pela largura.
        assert_eq!(fit_within(3440, 1440, 1920, 1080), (1920, 804));
    }

    #[test]
    fn cores_puras_bt601() {
        let cases = [
            ([0, 0, 0, 255], (16, 128, 128)),        // preto
            ([255, 255, 255, 255], (235, 128, 128)), // branco
            ([0, 0, 255, 255], (82, 90, 240)),       // vermelho
            ([0, 255, 0, 255], (144, 54, 34)),       // verde
            ([255, 0, 0, 255], (41, 240, 110)),      // azul
        ];
        for (bgra, (y, u, v)) in cases {
            let src = solid(4, 4, bgra);
            let mut dst = Nv12Frame::new(4, 4);
            bgra_to_nv12(&src, &mut dst);
            assert!(dst.y_plane().iter().all(|&p| p == y), "{bgra:?} Y");
            for uv in dst.uv_plane().chunks_exact(2) {
                assert_eq!((uv[0], uv[1]), (u, v), "{bgra:?} UV");
            }
        }
    }

    #[test]
    fn imagem_grande_em_paralelo_bate_com_a_formula() {
        let (w, h) = (640u32, 480u32);
        let mut src = BgraFrame::new(w, h);
        for (i, px) in src.data.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            px.copy_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255]);
        }
        let mut dst = Nv12Frame::new(w, h);
        bgra_to_nv12(&src, &mut dst);
        for &(x, y) in &[(0u32, 0u32), (639, 0), (100, 239), (320, 240), (639, 479)] {
            let p = &src.row(y)[x as usize * 4..];
            let expected = luma(p[2] as i32, p[1] as i32, p[0] as i32);
            assert_eq!(dst.y_plane()[(y * w + x) as usize], expected, "({x},{y})");
        }
    }

    #[test]
    fn respeita_stride_e_reduz() {
        // 8×4 com stride maior; metade esquerda preta, direita branca.
        let mut src = BgraFrame {
            width: 8,
            height: 4,
            stride: 40,
            data: vec![0xAA; 40 * 4],
        };
        for y in 0..4 {
            for x in 0..8 {
                let v = if x < 4 { 0 } else { 255 };
                let i = y * 40 + x * 4;
                src.data[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let mut dst = Nv12Frame::new(4, 2);
        bgra_to_nv12(&src, &mut dst);
        let y = dst.y_plane();
        assert_eq!(&y[..4], &[16, 16, 235, 235]);
        assert_eq!(&y[4..], &[16, 16, 235, 235]);
    }
}

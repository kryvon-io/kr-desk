//! Implementação de [`FrameSource`] com `IDXGIOutputDuplication`.
//!
//! Fluxo de cada quadro: `AcquireNextFrame` (espera uma mudança na tela) →
//! cópia da textura da área de trabalho para uma textura *staging* → `Map` →
//! cópia das linhas para um [`BgraFrame`] na memória do sistema.
//!
//! A duplicação é perdida em trocas de resolução, ao abrir o UAC/tela de
//! bloqueio (*secure desktop*) ou quando outro app entra em tela cheia
//! exclusiva. Nesses casos devolvemos [`CaptureError::Transient`] e
//! recriamos tudo na próxima chamada.

use std::collections::VecDeque;
use std::time::Duration;

use kr_core::media::{
    BgraFrame, CaptureError, CursorUpdate, DesktopRect, FrameSource, MonitorInfo, SourceEvent,
};
use tracing::{info, warn};
use windows::Win32::Foundation::{E_ACCESSDENIED, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_DENIED, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_SESSION_DISCONNECTED,
    DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME,
    IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::{HRESULT, Interface};

/// Erros que indicam "tente de novo daqui a pouco".
fn is_transient(code: HRESULT) -> bool {
    [
        DXGI_ERROR_ACCESS_LOST,
        DXGI_ERROR_ACCESS_DENIED,
        E_ACCESSDENIED,
        DXGI_ERROR_NOT_CURRENTLY_AVAILABLE,
        DXGI_ERROR_SESSION_DISCONNECTED,
        DXGI_ERROR_UNSUPPORTED,
        DXGI_ERROR_NOT_FOUND,
    ]
    .contains(&code)
}

fn map_err(context: &str, err: windows::core::Error) -> CaptureError {
    let msg = format!("{context}: {err}");
    if is_transient(err.code()) {
        CaptureError::Transient(msg)
    } else {
        CaptureError::Fatal(msg)
    }
}

/// Dispositivo D3D11 e duplicação de um monitor.
struct Session {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    output_name: String,
}

impl Session {
    /// Lista os monitores ligados à área de trabalho: o principal primeiro,
    /// depois da esquerda para a direita.
    fn enumerate() -> Result<Vec<(IDXGIAdapter1, IDXGIOutput1, MonitorInfo)>, CaptureError> {
        let mut found = Vec::new();
        // SAFETY: chamadas COM/DXGI comuns; os objetos retornados são
        // gerenciados pelos wrappers do `windows` (Release no Drop).
        unsafe {
            let factory: IDXGIFactory1 =
                CreateDXGIFactory1().map_err(|e| map_err("CreateDXGIFactory1", e))?;
            let mut adapter_index = 0;
            while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
                adapter_index += 1;
                let mut output_index = 0;
                while let Ok(output) = adapter.EnumOutputs(output_index) {
                    output_index += 1;
                    let Ok(desc) = output.GetDesc() else { continue };
                    if !desc.AttachedToDesktop.as_bool() {
                        continue;
                    }
                    let Ok(output1) = output.cast::<IDXGIOutput1>() else {
                        continue;
                    };
                    let name = String::from_utf16_lossy(&desc.DeviceName)
                        .trim_end_matches('\0')
                        .to_string();
                    let r = desc.DesktopCoordinates;
                    let info = MonitorInfo {
                        index: 0,
                        name,
                        rect: DesktopRect {
                            left: r.left,
                            top: r.top,
                            width: (r.right - r.left).max(0) as u32,
                            height: (r.bottom - r.top).max(0) as u32,
                        },
                        primary: r.left == 0 && r.top == 0,
                    };
                    found.push((adapter.clone(), output1, info));
                }
            }
        }
        found.sort_by_key(|(_, _, m)| (!m.primary, m.rect.left, m.rect.top));
        for (i, (_, _, m)) in found.iter_mut().enumerate() {
            m.index = i as u32;
        }
        Ok(found)
    }

    /// Abre o monitor `selected` (ou o principal). Devolve também a lista de
    /// monitores e o índice aberto.
    fn open_selected(selected: Option<u32>) -> Result<(Self, Vec<MonitorInfo>, u32), CaptureError> {
        let outputs = Self::enumerate()?;
        let chosen = selected
            .and_then(|i| outputs.iter().position(|(_, _, m)| m.index == i))
            .unwrap_or(0);
        let Some((adapter, output, info)) = outputs.get(chosen) else {
            return Err(CaptureError::Transient(
                "nenhum monitor ligado à área de trabalho".into(),
            ));
        };
        let session = Self::open(adapter, output, info.name.clone())?;
        let active = info.index;
        let list = outputs.into_iter().map(|(_, _, m)| m).collect();
        Ok((session, list, active))
    }

    fn open(
        adapter: &IDXGIAdapter1,
        output: &IDXGIOutput1,
        output_name: String,
    ) -> Result<Self, CaptureError> {
        let mut device = None;
        let mut context = None;
        // SAFETY: ponteiros de saída válidos durante a chamada.
        unsafe {
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| map_err("D3D11CreateDevice", e))?;
        }
        let device: ID3D11Device =
            device.ok_or_else(|| CaptureError::Fatal("D3D11 sem dispositivo".into()))?;
        let context = context.ok_or_else(|| CaptureError::Fatal("D3D11 sem contexto".into()))?;
        // SAFETY: `device` é um ID3D11Device válido.
        let duplication = unsafe { output.DuplicateOutput(&device) }
            .map_err(|e| map_err("DuplicateOutput", e))?;
        Ok(Self {
            device,
            context,
            duplication,
            staging: None,
            output_name,
        })
    }

    fn staging(&mut self, width: u32, height: u32) -> Result<ID3D11Texture2D, CaptureError> {
        if let Some((tex, w, h)) = &self.staging
            && *w == width
            && *h == height
        {
            return Ok(tex.clone());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut tex = None;
        // SAFETY: descrição válida; ponteiro de saída válido.
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex)) }
            .map_err(|e| map_err("CreateTexture2D (staging)", e))?;
        let tex = tex.ok_or_else(|| CaptureError::Fatal("textura staging nula".into()))?;
        self.staging = Some((tex.clone(), width, height));
        Ok(tex)
    }
}

/// Converte a forma do ponteiro da DXGI para RGBA.
///
/// - Colorido: BGRA com alfa.
/// - Monocromático: máscaras AND e XOR de 1 bit (altura dobrada). Pixels que
///   invertem a tela viram pretos (não há "inverter" num cursor CSS).
/// - Colorido com máscara: alfa 0xFF indica XOR com a tela; tratamos preto
///   como transparente e as demais cores como opacas.
pub(crate) fn pointer_to_rgba(
    kind: u32,
    width: u32,
    height: u32,
    pitch: u32,
    buf: &[u8],
) -> Option<(u32, u32, Vec<u8>)> {
    let (w, pitch) = (width as usize, pitch as usize);
    if kind == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
        let h = height as usize / 2;
        let bit = |row: usize, x: usize| -> Option<bool> {
            let byte = *buf.get(row * pitch + x / 8)?;
            Some(byte & (0x80 >> (x % 8)) != 0)
        };
        let mut out = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let and = bit(y, x)?;
                let xor = bit(y + h, x)?;
                out.extend_from_slice(&match (and, xor) {
                    (true, false) => [0, 0, 0, 0],
                    (false, false) => [0, 0, 0, 255],
                    (false, true) => [255, 255, 255, 255],
                    (true, true) => [0, 0, 0, 255],
                });
            }
        }
        return Some((width, h as u32, out));
    }
    let masked = kind == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 as u32;
    if !masked && kind != DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32 {
        return None;
    }
    let h = height as usize;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let i = y * pitch + x * 4;
            let px = buf.get(i..i + 4)?;
            let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
            let rgba = if masked {
                if a == 0 {
                    [r, g, b, 255]
                } else if r == 0 && g == 0 && b == 0 {
                    [0, 0, 0, 0]
                } else {
                    [r, g, b, 255]
                }
            } else {
                [r, g, b, a]
            };
            out.extend_from_slice(&rgba);
        }
    }
    Some((width, height, out))
}

/// Captura de um monitor (o principal, até que outro seja escolhido).
pub struct DxgiCapture {
    session: Option<Session>,
    /// Monitor escolhido pelo viewer (`None` = principal).
    selected: Option<u32>,
    frame: BgraFrame,
    has_frame: bool,
    cursor_visible: Option<bool>,
    events: VecDeque<SourceEvent>,
    shape_buffer: Vec<u8>,
}

impl DxgiCapture {
    /// Abre a captura do monitor principal. Deve ser criado na thread que
    /// vai capturar (objetos COM).
    pub fn primary() -> Result<Self, CaptureError> {
        let mut capture = Self {
            session: None,
            selected: None,
            frame: BgraFrame::new(2, 2),
            has_frame: false,
            cursor_visible: None,
            events: VecDeque::new(),
            shape_buffer: Vec::new(),
        };
        capture.reopen()?;
        Ok(capture)
    }

    /// (Re)abre o monitor escolhido e avisa a lista de monitores.
    fn reopen(&mut self) -> Result<(), CaptureError> {
        self.session = None;
        let (session, list, active) = Session::open_selected(self.selected)?;
        info!(monitor = %session.output_name, active, total = list.len(), "captura DXGI aberta");
        self.session = Some(session);
        self.has_frame = false;
        self.cursor_visible = None;
        self.events
            .push_back(SourceEvent::Monitors { list, active });
        Ok(())
    }

    fn session(&mut self) -> Result<&mut Session, CaptureError> {
        if self.session.is_none() {
            self.reopen()?;
        }
        Ok(self.session.as_mut().expect("aberta acima"))
    }

    /// Lê visibilidade e forma do ponteiro do quadro adquirido.
    fn read_pointer(
        &mut self,
        duplication: &IDXGIOutputDuplication,
        info: &DXGI_OUTDUPL_FRAME_INFO,
    ) {
        if info.LastMouseUpdateTime == 0 {
            return;
        }
        let visible = info.PointerPosition.Visible.as_bool();
        if self.cursor_visible != Some(visible) {
            self.cursor_visible = Some(visible);
            self.events
                .push_back(SourceEvent::Cursor(CursorUpdate::Visible(visible)));
        }
        if info.PointerShapeBufferSize == 0 {
            return;
        }
        self.shape_buffer
            .resize(info.PointerShapeBufferSize as usize, 0);
        let mut required = 0u32;
        let mut shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        // SAFETY: o buffer tem o tamanho informado pela DXGI.
        let result = unsafe {
            duplication.GetFramePointerShape(
                self.shape_buffer.len() as u32,
                self.shape_buffer.as_mut_ptr().cast(),
                &mut required,
                &mut shape,
            )
        };
        if let Err(err) = result {
            warn!(%err, "não foi possível ler a forma do cursor");
            return;
        }
        if let Some((width, height, rgba)) = pointer_to_rgba(
            shape.Type,
            shape.Width,
            shape.Height,
            shape.Pitch,
            &self.shape_buffer,
        ) {
            self.events
                .push_back(SourceEvent::Cursor(CursorUpdate::Shape {
                    width,
                    height,
                    hot_x: shape.HotSpot.x.max(0) as u32,
                    hot_y: shape.HotSpot.y.max(0) as u32,
                    rgba,
                }));
        }
    }

    fn lose(&mut self, err: CaptureError) -> CaptureError {
        if matches!(err, CaptureError::Transient(_)) {
            self.session = None;
        }
        err
    }

    /// Copia a textura adquirida para `self.frame`.
    fn read_texture(&mut self, resource: &IDXGIResource) -> Result<(), CaptureError> {
        let texture: ID3D11Texture2D = resource
            .cast()
            .map_err(|e| map_err("IDXGIResource → ID3D11Texture2D", e))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` é um destino válido.
        unsafe { texture.GetDesc(&mut desc) };
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(CaptureError::Fatal(format!(
                "formato de tela não suportado: {:?} (HDR?)",
                desc.Format
            )));
        }
        let session = self.session.as_mut().expect("sessão ativa");
        let staging = session.staging(desc.Width, desc.Height)?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: texturas do mesmo dispositivo e tamanho; `mapped` válido.
        unsafe {
            session.context.CopyResource(&staging, &texture);
            session
                .context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| map_err("Map", e))?;
        }

        let (w, h) = (desc.Width, desc.Height);
        if self.frame.width != w || self.frame.height != h {
            self.frame = BgraFrame::new(w, h);
        }
        let row_bytes = w as usize * 4;
        for y in 0..h as usize {
            // SAFETY: o driver garante `RowPitch * Height` bytes legíveis
            // enquanto o recurso está mapeado.
            let src = unsafe {
                std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    row_bytes,
                )
            };
            let start = y * self.frame.stride;
            self.frame.data[start..start + row_bytes].copy_from_slice(src);
        }
        // SAFETY: mapeado acima.
        unsafe { session.context.Unmap(&staging, 0) };
        self.has_frame = true;
        Ok(())
    }
}

impl FrameSource for DxgiCapture {
    fn capture(&mut self, timeout: Duration) -> Result<Option<&BgraFrame>, CaptureError> {
        let duplication = match self.session() {
            Ok(s) => s.duplication.clone(),
            Err(e) => return Err(self.lose(e)),
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: ponteiros de saída válidos.
        let acquired = unsafe {
            duplication.AcquireNextFrame(
                timeout.as_millis().min(u32::MAX as u128) as u32,
                &mut info,
                &mut resource,
            )
        };
        match acquired {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(e) => {
                let err = map_err("AcquireNextFrame", e);
                warn!(%err, "duplicação perdida");
                return Err(self.lose(err));
            }
        }

        self.read_pointer(&duplication, &info);
        // Só o cursor mexeu (LastPresentTime 0): a imagem não mudou.
        let changed = info.LastPresentTime != 0 || !self.has_frame;
        let result = match (&resource, changed) {
            (Some(res), true) => self.read_texture(res).map(|()| true),
            _ => Ok(false),
        };
        // SAFETY: um quadro foi adquirido acima.
        if let Err(e) = unsafe { duplication.ReleaseFrame() } {
            let err = map_err("ReleaseFrame", e);
            return Err(self.lose(err));
        }
        match result {
            Ok(true) => Ok(Some(&self.frame)),
            Ok(false) => Ok(None),
            Err(err) => Err(self.lose(err)),
        }
    }

    fn take_event(&mut self) -> Option<SourceEvent> {
        self.events.pop_front()
    }

    fn select_monitor(&mut self, index: u32) -> Result<(), CaptureError> {
        let previous = self.selected;
        self.selected = Some(index);
        if let Err(err) = self.reopen() {
            // Volta ao monitor anterior; se nem ele abrir, a próxima captura tenta de novo.
            self.selected = previous;
            let _ = self.reopen();
            return Err(err);
        }
        Ok(())
    }

    fn name(&self) -> String {
        match &self.session {
            Some(s) => format!("DXGI Desktop Duplication ({})", s.output_name),
            None => "DXGI Desktop Duplication".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_monocromatico() {
        // 2×1: AND = [1,0], XOR = [0,1] → transparente, branco.
        let buf = [0b1000_0000, 0b0100_0000];
        let (w, h, rgba) = pointer_to_rgba(
            DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32,
            2,
            2,
            1,
            &buf,
        )
        .unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, vec![0, 0, 0, 0, 255, 255, 255, 255]);
    }

    #[test]
    fn cursor_colorido_troca_bgra_por_rgba() {
        let buf = [1, 2, 3, 128];
        let (_, _, rgba) = pointer_to_rgba(
            DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32,
            1,
            1,
            4,
            &buf,
        )
        .unwrap();
        assert_eq!(rgba, vec![3, 2, 1, 128]);
    }
}

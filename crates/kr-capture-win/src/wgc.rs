//! Captura com Windows.Graphics.Capture (WGC).
//!
//! Alternativa à Desktop Duplication para onde ela falha: algumas máquinas
//! virtuais, notebooks com GPU híbrida e sessões em que a DXGI devolve
//! `DXGI_ERROR_UNSUPPORTED`. O cursor vem desenhado na própria imagem (a WGC
//! não informa a forma dele separadamente).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use kr_core::media::{BgraFrame, CaptureError, FrameSource, MonitorInfo, SourceEvent};
use tracing::{info, warn};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::Interface;

use crate::dxgi::{Session, map_err};

const PIXEL_FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
const BUFFERS: i32 = 2;

struct Wgc {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    size: SizeInt32,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    name: String,
}

impl Drop for Wgc {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

impl Wgc {
    /// Abre o monitor `selected` (ou o principal).
    fn open(selected: Option<u32>) -> Result<(Self, Vec<MonitorInfo>, u32), CaptureError> {
        // SAFETY: inicialização WinRT da thread de captura (MTA); repetir é inofensivo.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let outputs = Session::enumerate()?;
        let chosen = selected
            .and_then(|i| outputs.iter().position(|(_, _, m)| m.index == i))
            .unwrap_or(0);
        let Some((adapter, output, info)) = outputs.get(chosen) else {
            return Err(CaptureError::Transient(
                "nenhum monitor ligado à área de trabalho".into(),
            ));
        };

        let mut device = None;
        let mut context = None;
        // SAFETY: chamadas D3D11/DXGI/WinRT na thread de captura; ponteiros de
        // saída válidos durante as chamadas.
        let wgc = unsafe {
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
            .map_err(|e| map_err("D3D11CreateDevice (WGC)", e))?;
            let device: ID3D11Device =
                device.ok_or_else(|| CaptureError::Fatal("D3D11 sem dispositivo".into()))?;
            let context =
                context.ok_or_else(|| CaptureError::Fatal("D3D11 sem contexto".into()))?;
            let dxgi: IDXGIDevice = device.cast().map_err(|e| map_err("IDXGIDevice", e))?;
            let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
                .and_then(|i| i.cast())
                .map_err(|e| map_err("CreateDirect3D11DeviceFromDXGIDevice", e))?;
            let monitor = output
                .GetDesc()
                .map_err(|e| map_err("IDXGIOutput::GetDesc", e))?
                .Monitor;
            let interop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                    .map_err(|e| map_err("IGraphicsCaptureItemInterop", e))?;
            let item: GraphicsCaptureItem = interop
                .CreateForMonitor(monitor)
                .map_err(|e| map_err("CreateForMonitor", e))?;
            let size = item
                .Size()
                .map_err(|e| map_err("GraphicsCaptureItem::Size", e))?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &winrt_device,
                PIXEL_FORMAT,
                BUFFERS,
                size,
            )
            .map_err(|e| map_err("CreateFreeThreaded", e))?;
            let session = pool
                .CreateCaptureSession(&item)
                .map_err(|e| map_err("CreateCaptureSession", e))?;
            // Sem moldura amarela (Windows 11) e com o cursor na imagem.
            let _ = session.SetIsBorderRequired(false);
            let _ = session.SetIsCursorCaptureEnabled(true);
            session
                .StartCapture()
                .map_err(|e| map_err("StartCapture", e))?;
            Wgc {
                device,
                context,
                winrt_device,
                pool,
                session,
                size,
                staging: None,
                name: info.name.clone(),
            }
        };
        let active = info.index;
        let list = outputs.into_iter().map(|(_, _, m)| m).collect();
        Ok((wgc, list, active))
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
            .map_err(|e| map_err("CreateTexture2D (staging WGC)", e))?;
        let tex = tex.ok_or_else(|| CaptureError::Fatal("textura staging nula".into()))?;
        self.staging = Some((tex.clone(), width, height));
        Ok(tex)
    }
}

/// Captura de monitor via Windows.Graphics.Capture.
pub struct WgcCapture {
    wgc: Option<Wgc>,
    selected: Option<u32>,
    frame: BgraFrame,
    events: VecDeque<SourceEvent>,
}

impl WgcCapture {
    /// Abre o monitor principal. Deve ser criado na thread que vai capturar.
    pub fn primary() -> Result<Self, CaptureError> {
        let mut capture = Self {
            wgc: None,
            selected: None,
            frame: BgraFrame::new(2, 2),
            events: VecDeque::new(),
        };
        capture.reopen()?;
        Ok(capture)
    }

    fn reopen(&mut self) -> Result<(), CaptureError> {
        self.wgc = None;
        let (wgc, list, active) = Wgc::open(self.selected)?;
        info!(monitor = %wgc.name, active, "captura WGC aberta");
        self.wgc = Some(wgc);
        self.events
            .push_back(SourceEvent::Monitors { list, active });
        Ok(())
    }

    /// Copia a textura do quadro para `self.frame` (recortada ao conteúdo).
    fn read(&mut self, texture: &ID3D11Texture2D, content: SizeInt32) -> Result<(), CaptureError> {
        let wgc = self.wgc.as_mut().expect("aberta");
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: destino válido.
        unsafe { texture.GetDesc(&mut desc) };
        let w = (content.Width.max(2) as u32).min(desc.Width) & !1;
        let h = (content.Height.max(2) as u32).min(desc.Height) & !1;
        let staging = wgc.staging(desc.Width, desc.Height)?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: mesma textura/dispositivo; mapeada só durante a cópia.
        unsafe {
            wgc.context.CopyResource(&staging, texture);
            wgc.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| map_err("Map (WGC)", e))?;
        }
        if self.frame.width != w || self.frame.height != h {
            self.frame = BgraFrame::new(w, h);
        }
        let row_bytes = w as usize * 4;
        for y in 0..h as usize {
            // SAFETY: `RowPitch * Height` bytes legíveis enquanto mapeado.
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
        unsafe { wgc.context.Unmap(&staging, 0) };
        Ok(())
    }
}

impl FrameSource for WgcCapture {
    fn capture(&mut self, timeout: Duration) -> Result<Option<&BgraFrame>, CaptureError> {
        if self.wgc.is_none() {
            self.reopen()?;
        }
        let deadline = Instant::now() + timeout;
        let frame = loop {
            let wgc = self.wgc.as_ref().expect("aberta");
            // Fica só com o quadro mais recente.
            let mut latest = None;
            while let Ok(f) = wgc.pool.TryGetNextFrame() {
                if let Some(old) = latest.replace(f) {
                    let _ = windows::Graphics::Capture::Direct3D11CaptureFrame::Close(&old);
                }
            }
            if let Some(f) = latest {
                break f;
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(2));
        };

        let result = (|| {
            let content = frame.ContentSize().map_err(|e| map_err("ContentSize", e))?;
            let surface = frame.Surface().map_err(|e| map_err("Surface", e))?;
            let access: IDirect3DDxgiInterfaceAccess = surface
                .cast()
                .map_err(|e| map_err("IDirect3DDxgiInterfaceAccess", e))?;
            // SAFETY: a superfície da WGC é uma textura D3D11 do nosso dispositivo.
            let texture: ID3D11Texture2D =
                unsafe { access.GetInterface() }.map_err(|e| map_err("GetInterface", e))?;
            self.read(&texture, content)?;
            // Tela mudou de tamanho: recria o pool no tamanho novo.
            let wgc = self.wgc.as_mut().expect("aberta");
            if content.Width != wgc.size.Width || content.Height != wgc.size.Height {
                wgc.size = content;
                wgc.pool
                    .Recreate(&wgc.winrt_device, PIXEL_FORMAT, BUFFERS, content)
                    .map_err(|e| map_err("Recreate", e))?;
            }
            Ok::<(), CaptureError>(())
        })();
        let _ = frame.Close();
        match result {
            Ok(()) => Ok(Some(&self.frame)),
            Err(err) => {
                warn!(%err, "falha na captura WGC; reabrindo");
                self.wgc = None;
                Err(err)
            }
        }
    }

    fn name(&self) -> String {
        match &self.wgc {
            Some(w) => format!("Windows.Graphics.Capture ({})", w.name),
            None => "Windows.Graphics.Capture".into(),
        }
    }

    fn take_event(&mut self) -> Option<SourceEvent> {
        self.events.pop_front()
    }

    fn select_monitor(&mut self, index: u32) -> Result<(), CaptureError> {
        let previous = self.selected;
        self.selected = Some(index);
        if let Err(err) = self.reopen() {
            self.selected = previous;
            let _ = self.reopen();
            return Err(err);
        }
        Ok(())
    }
}

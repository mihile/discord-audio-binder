//! WGC 캡처 → CPU 처리(크롭 + HDR 톤매핑) → ①egui 미리보기 버퍼 ②네이티브 'GameOutput' 출력 창.
//! 모든 D3D/WGC 객체는 이 스레드 소유. egui 와는 채널(Cmd) + 공유 Preview 로만 통신.

use std::sync::{
    Arc, Mutex,
    mpsc::{Receiver, Sender},
};
use std::time::{Duration, Instant};
use windows::Foundation::TimeSpan;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, PCSTR, s, w};

/// egui 미리보기에 보여줄 최신 프레임(RGBA8).
#[derive(Default)]
pub struct Preview {
    pub w: usize,
    pub h: usize,
    pub rgba: Arc<[u8]>,
    pub version: u64,
    pub out_fps: f32,    // 출력 창 실제 present FPS (진단용)
    pub wgc_fps: f32,    // WGC 가 실제로 전달한 프레임 FPS (진단용)
    pub max_gap_ms: f32, // 최근 1초 출력 프레임 최대 간격
}

pub enum Cmd {
    Start { target_hwnd: isize },
    Stop,
    Show,
    Hide,
    SetCrop(bool),
    SetAspectCrop(bool),
    SetAspect { w: f32, h: f32 },
    SetAspectAlign { x: i32, y: i32 },
    SetHdr(bool),
    SetNits(f32),
    SetVsync(bool),
    SetOutputFps(f32),
    Quit,
}

pub struct CaptureHandle {
    tx: Sender<Cmd>,
    preview: Arc<Mutex<Preview>>,
}

impl CaptureHandle {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let preview = Arc::new(Mutex::new(Preview::default()));
        let p2 = preview.clone();
        let _ = std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                if let Err(e) = run(rx, p2) {
                    eprintln!("capture thread error: {e:?}");
                }
            });
        Self { tx, preview }
    }

    pub fn preview(&self) -> Arc<Mutex<Preview>> {
        self.preview.clone()
    }

    pub fn start(&self, target_hwnd: isize) {
        let _ = self.tx.send(Cmd::Start { target_hwnd });
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
    pub fn show(&self) {
        let _ = self.tx.send(Cmd::Show);
    }
    pub fn hide(&self) {
        let _ = self.tx.send(Cmd::Hide);
    }
    pub fn set_crop(&self, on: bool) {
        let _ = self.tx.send(Cmd::SetCrop(on));
    }
    pub fn set_aspect_crop(&self, on: bool) {
        let _ = self.tx.send(Cmd::SetAspectCrop(on));
    }
    pub fn set_aspect(&self, w: f32, h: f32) {
        let _ = self.tx.send(Cmd::SetAspect { w, h });
    }
    pub fn set_aspect_align(&self, x: i32, y: i32) {
        let _ = self.tx.send(Cmd::SetAspectAlign { x, y });
    }
    pub fn set_hdr(&self, on: bool) {
        let _ = self.tx.send(Cmd::SetHdr(on));
    }
    pub fn set_nits(&self, n: f32) {
        let _ = self.tx.send(Cmd::SetNits(n));
    }
    pub fn set_vsync(&self, on: bool) {
        let _ = self.tx.send(Cmd::SetVsync(on));
    }
    pub fn set_output_fps(&self, fps: f32) {
        let _ = self.tx.send(Cmd::SetOutputFps(fps));
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
    }
}

struct State {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    rt_device: IDirect3DDevice,
    swapchain: IDXGISwapChain1,
    backbuffer: Option<ID3D11Texture2D>,
    rtv: Option<ID3D11RenderTargetView>,
    hwnd: HWND,
    w: i32,
    h: i32,

    target: isize,
    crop: bool,
    aspect_crop: bool,
    aspect_w: f32,
    aspect_h: f32,
    aspect_align_x: i32,
    aspect_align_y: i32,
    crop_cache: Option<(i32, i32, i32, i32)>,
    crop_cache_fw: i32,
    crop_cache_fh: i32,
    crop_cache_checked: Instant,
    hdr: bool,
    nits: f32,
    vsync: bool,
    output_fps: f32,
    next_output: Instant,

    pool: Option<Direct3D11CaptureFramePool>,
    session: Option<GraphicsCaptureSession>,
    item: Option<GraphicsCaptureItem>,
    capturing: bool,
    hires_timer: bool,

    pv_scratch: Vec<u8>,
    pv_tex: [Option<ID3D11Texture2D>; 2], // 미리보기 더블 버퍼 staging
    pv_cur: usize,
    pv_sw: i32,
    pv_sh: i32,
    pv_sfp16: bool,
    last_preview: Instant,
    frame_count: u32,
    wgc_count: u32,
    fps_timer: Instant,
    last_present: Option<Instant>,
    max_gap_ms: f32,
    lut: [u8; 1024],
    saved_pos: (i32, i32),
    preview: Arc<Mutex<Preview>>,

    // HDR GPU 톤매핑 파이프라인
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    cbuffer: ID3D11Buffer,
    cbuffer_scale: f32, // cbuffer 에 마지막으로 올린 scale (nits 변경 시에만 재업로드)
    hdr_tex: Option<ID3D11Texture2D>,
    hdr_srv: Option<ID3D11ShaderResourceView>,
    hdr_w: i32,
    hdr_h: i32,
}

const SHADER_HLSL: &str = r#"
Texture2D tex0 : register(t0);
SamplerState smp0 : register(s0);
cbuffer Params : register(b0) { float scale; float3 _pad; };
struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
VSOut VSMain(uint id : SV_VertexID) {
    VSOut o;
    o.uv = float2((id << 1) & 2, id & 2);
    o.pos = float4(o.uv * float2(2, -2) + float2(-1, 1), 0, 1);
    return o;
}
float3 lin2srgb(float3 c) {
    c = saturate(c);
    return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}
float4 PSMain(VSOut i) : SV_Target {
    float3 lin = tex0.Sample(smp0, i.uv).rgb * scale; // scRGB linear → SDR 정규화
    return float4(lin2srgb(lin), 1.0);
}
"#;

fn run(rx: Receiver<Cmd>, preview: Arc<Mutex<Preview>>) -> windows::core::Result<()> {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
    let mut st = State::new(preview)?;

    let mut msg = MSG::default();
    let mut pending_frame: Option<Direct3D11CaptureFrame> = None;
    let mut last_frame: Option<Direct3D11CaptureFrame> = None;
    let mut last_alive_check = Instant::now();
    loop {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Quit => {
                    st.stop_capture(); // 고해상 타이머도 여기서 해제
                    return Ok(());
                }
                Cmd::Start { target_hwnd } => {
                    pending_frame = None;
                    last_frame = None;
                    st.target = target_hwnd;
                    if let Err(e) = st.start_capture() {
                        eprintln!("start_capture: {e:?}");
                    }
                }
                Cmd::Stop => {
                    pending_frame = None;
                    last_frame = None;
                    st.stop_capture();
                }
                Cmd::Show => st.show(),
                Cmd::Hide => st.hide(),
                Cmd::SetCrop(on) => {
                    st.crop = on;
                    st.crop_cache = None;
                }
                Cmd::SetAspectCrop(on) => {
                    st.aspect_crop = on;
                    st.crop_cache = None;
                }
                Cmd::SetAspect { w, h } => {
                    st.aspect_w = w.clamp(1.0, 64.0);
                    st.aspect_h = h.clamp(1.0, 64.0);
                    st.crop_cache = None;
                }
                Cmd::SetAspectAlign { x, y } => {
                    st.aspect_align_x = x.clamp(0, 2);
                    st.aspect_align_y = y.clamp(0, 2);
                    st.crop_cache = None;
                }
                Cmd::SetHdr(on) => {
                    if st.hdr != on {
                        st.hdr = on;
                        if st.capturing {
                            pending_frame = None;
                            last_frame = None;
                            let _ = st.start_capture(); // 포맷 바뀌므로 재시작
                        }
                    }
                }
                Cmd::SetNits(n) => st.nits = n,
                Cmd::SetVsync(on) => st.vsync = on,
                Cmd::SetOutputFps(fps) => st.set_output_fps(fps),
            }
        }

        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        let mut rendered = false;
        if st.capturing {
            // 캡처 대상 창(게임)이 닫히면 깨끗하게 캡처 중지.
            // 안 그러면 스테일 프레임을 계속 재-present 하며, 게임 종료 시점에
            // Discord 오버레이 훅(DiscordHook64.dll) teardown 과 경합해 AV 크래시 유발 가능.
            if last_alive_check.elapsed() >= Duration::from_millis(500) {
                last_alive_check = Instant::now();
                let alive = unsafe { IsWindow(Some(HWND(st.target as *mut _))).as_bool() };
                if !alive {
                    pending_frame = None;
                    last_frame = None;
                    st.stop_capture();
                    if let Ok(mut p) = st.preview.lock() {
                        p.out_fps = 0.0;
                        p.wgc_fps = 0.0;
                        p.max_gap_ms = 0.0;
                    }
                    continue;
                }
            }

            // 큐에 쌓인 프레임을 모두 빼고 '가장 최신'만 렌더 (전달량은 wgc_count 집계)
            let mut latest = None;
            let mut drained = 0u32;
            if let Some(pool) = &st.pool {
                while let Ok(frame) = pool.TryGetNextFrame() {
                    drained += 1;
                    latest = Some(frame);
                }
            }
            st.wgc_count += drained;
            if let Some(frame) = latest {
                pending_frame = Some(frame);
            }

            if st.output_due() {
                if let Some(frame) = pending_frame.take() {
                    last_frame = Some(frame);
                }
                if let Some(frame) = last_frame.as_ref() {
                    if let Err(e) = st.render(frame) {
                        eprintln!("render: {e:?}");
                    }
                    rendered = true;
                }
            }
        }

        if !rendered {
            if st.capturing {
                // 다음 출력 시각까지 남은 만큼 자되 [300µs, 1ms] 로 클램프
                // (출력 직전엔 반응성 유지, 멀면 wakeup 횟수 절감: ~3300/s → ~1000/s)
                let until = st
                    .next_output
                    .saturating_duration_since(Instant::now())
                    .clamp(Duration::from_micros(300), Duration::from_millis(1));
                std::thread::sleep(until);
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

impl State {
    fn output_due(&self) -> bool {
        Instant::now() >= self.next_output
    }

    fn schedule_next_output(&mut self) {
        let interval = Duration::from_secs_f32(1.0 / self.output_fps.clamp(30.0, 180.0));
        let now = Instant::now();
        self.next_output += interval;
        if self.next_output <= now || self.next_output.duration_since(now) > interval * 2 {
            self.next_output = now + interval;
        }
    }

    fn set_output_fps(&mut self, fps: f32) {
        self.output_fps = fps.clamp(30.0, 180.0);
        if let Some(session) = &self.session {
            let _ = session.SetMinUpdateInterval(self.capture_interval());
        }
    }

    fn capture_interval(&self) -> TimeSpan {
        TimeSpan {
            Duration: (10_000_000.0 / self.output_fps.clamp(30.0, 180.0)).round() as i64,
        }
    }
}

impl State {
    fn new(preview: Arc<Mutex<Preview>>) -> windows::core::Result<Self> {
        let (device, context) = create_d3d_device()?;
        let rt_device = create_winrt_device(&device)?;
        let hwnd = create_output_window()?;
        let (w, h) = (960, 540);
        let swapchain = create_swapchain(&device, hwnd, w, h)?;

        let mut lut = [0u8; 1024];
        for (i, v) in lut.iter_mut().enumerate() {
            let l = i as f32 / 1023.0;
            let s = if l <= 0.0031308 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            *v = (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }

        let (vs, ps, sampler, cbuffer) = create_pipeline(&device)?;

        let mut st = Self {
            device,
            context,
            rt_device,
            swapchain,
            backbuffer: None,
            rtv: None,
            hwnd,
            w,
            h,
            target: 0,
            crop: true,
            aspect_crop: true,
            aspect_w: 16.0,
            aspect_h: 9.0,
            aspect_align_x: 1,
            aspect_align_y: 1,
            crop_cache: None,
            crop_cache_fw: 0,
            crop_cache_fh: 0,
            crop_cache_checked: Instant::now(),
            hdr: false,
            nits: 200.0,
            vsync: true,
            output_fps: 60.0,
            next_output: Instant::now(),
            pool: None,
            session: None,
            item: None,
            capturing: false,
            hires_timer: false,
            pv_scratch: Vec::new(),
            pv_tex: [None, None],
            pv_cur: 0,
            pv_sw: 0,
            pv_sh: 0,
            pv_sfp16: false,
            last_preview: Instant::now(),
            frame_count: 0,
            wgc_count: 0,
            fps_timer: Instant::now(),
            last_present: None,
            max_gap_ms: 0.0,
            lut,
            saved_pos: (120, 120),
            preview,
            vs,
            ps,
            sampler,
            cbuffer,
            cbuffer_scale: f32::NAN, // 첫 프레임에 반드시 업로드되도록
            hdr_tex: None,
            hdr_srv: None,
            hdr_w: 0,
            hdr_h: 0,
        };
        st.hide();
        Ok(st)
    }

    fn start_capture(&mut self) -> windows::core::Result<()> {
        self.stop_capture();
        self.next_output = Instant::now();
        self.last_present = None;
        self.crop_cache = None;
        let hwnd = HWND(self.target as *mut _);
        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
        let size = item.Size()?;
        let fmt = if self.hdr {
            DirectXPixelFormat::R16G16B16A16Float
        } else {
            DirectXPixelFormat::B8G8R8A8UIntNormalized
        };
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&self.rt_device, fmt, 3, size)?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetMinUpdateInterval(self.capture_interval());
        session.StartCapture()?;

        self.item = Some(item);
        self.pool = Some(pool);
        self.session = Some(session);
        self.capturing = true;
        // 캡처 중에만 시스템 타이머 해상도 1ms (sleep 정밀도 ↑, 유휴 시엔 시스템 부담 없음)
        if !self.hires_timer {
            unsafe {
                let _ = timeBeginPeriod(1);
            }
            self.hires_timer = true;
        }
        Ok(())
    }

    fn stop_capture(&mut self) {
        self.capturing = false;
        self.session = None;
        self.pool = None;
        self.item = None;
        if self.hires_timer {
            unsafe {
                let _ = timeEndPeriod(1);
            }
            self.hires_timer = false;
        }
    }

    fn render(&mut self, frame: &Direct3D11CaptureFrame) -> windows::core::Result<()> {
        let surface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let src: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let cs = frame.ContentSize()?;
        let (fw, fh) = (cs.Width, cs.Height);
        if fw <= 0 || fh <= 0 {
            return Ok(());
        }

        // 크롭 영역
        let (mut cx, mut cy, mut cw, mut ch) = (0, 0, fw, fh);
        if let Some(r) = self.cached_crop_rect(fw, fh) {
            cx = r.0;
            cy = r.1;
            cw = r.2;
            ch = r.3;
        }

        if cw <= 0 || ch <= 0 {
            return Ok(());
        }
        self.ensure_size(cw, ch)?;

        // ── 출력 창 ── 둘 다 순수 GPU 경로(60fps)
        if self.hdr {
            self.output_hdr_gpu(&src, cx, cy, cw, ch)?; // 픽셀 셰이더 톤매핑
        } else {
            self.output_sdr_gpu(&src, cx, cy, cw, ch)?; // 직접 복사
        }
        self.record_present_gap();
        self.schedule_next_output();

        // 진단: 출력 present FPS 측정
        self.frame_count += 1;
        let el = self.fps_timer.elapsed().as_secs_f32();
        if el >= 1.0 {
            let out = self.frame_count as f32 / el;
            let wgc = self.wgc_count as f32 / el;
            if let Ok(mut p) = self.preview.lock() {
                p.out_fps = out;
                p.wgc_fps = wgc;
                p.max_gap_ms = self.max_gap_ms;
            }
            self.frame_count = 0;
            self.wgc_count = 0;
            self.max_gap_ms = 0.0;
            self.fps_timer = Instant::now();
        }

        // ── 미리보기: 저빈도 CPU 읽기(출력 fps 에 영향 안 줌) ──
        if self.last_preview.elapsed() >= Duration::from_millis(500) {
            self.update_preview(&src, fw, fh, (cx, cy, cw, ch))?;
            self.last_preview = Instant::now();
        }
        Ok(())
    }

    fn cached_crop_rect(&mut self, fw: i32, fh: i32) -> Option<(i32, i32, i32, i32)> {
        let now = Instant::now();
        if self.crop_cache_fw == fw
            && self.crop_cache_fh == fh
            && now.duration_since(self.crop_cache_checked) < Duration::from_millis(500)
        {
            return self.crop_cache;
        }

        let mut rect = if self.crop {
            crop_rect(self.target, fw, fh).unwrap_or((0, 0, fw, fh))
        } else {
            (0, 0, fw, fh)
        };
        if self.aspect_crop {
            rect = crop_to_aspect(
                rect,
                self.aspect_w,
                self.aspect_h,
                self.aspect_align_x,
                self.aspect_align_y,
            );
        }
        self.crop_cache = Some(rect);
        self.crop_cache_fw = fw;
        self.crop_cache_fh = fh;
        self.crop_cache_checked = now;
        self.crop_cache
    }

    /// SDR: 소스(BGRA8)를 백버퍼로 GPU 직접 복사. CPU 왕복 없음.
    fn output_sdr_gpu(
        &mut self,
        src: &ID3D11Texture2D,
        cx: i32,
        cy: i32,
        cw: i32,
        ch: i32,
    ) -> windows::core::Result<()> {
        let src_box = D3D11_BOX {
            left: cx as u32,
            top: cy as u32,
            front: 0,
            right: (cx + cw) as u32,
            bottom: (cy + ch) as u32,
            back: 1,
        };
        unsafe {
            let backbuffer = self.backbuffer()?;
            self.context
                .CopySubresourceRegion(&backbuffer, 0, 0, 0, 0, src, 0, Some(&src_box));
            self.swapchain
                .Present(self.present_interval(), DXGI_PRESENT(0))
                .ok()?;
        }
        Ok(())
    }

    /// HDR: FP16 crop 을 SRV 텍스처로 복사 → 픽셀 셰이더로 톤매핑하여 백버퍼에 그림. 60fps.
    fn output_hdr_gpu(
        &mut self,
        src: &ID3D11Texture2D,
        cx: i32,
        cy: i32,
        cw: i32,
        ch: i32,
    ) -> windows::core::Result<()> {
        self.ensure_hdr_tex(cw, ch)?;
        let tex = self.hdr_tex.clone().unwrap();
        let srv = self.hdr_srv.clone().unwrap();
        let src_box = D3D11_BOX {
            left: cx as u32,
            top: cy as u32,
            front: 0,
            right: (cx + cw) as u32,
            bottom: (cy + ch) as u32,
            back: 1,
        };
        let scale_v = 80.0f32 / self.nits.max(1.0);
        let rtv = self.render_target_view()?;
        unsafe {
            self.context
                .CopySubresourceRegion(&tex, 0, 0, 0, 0, src, 0, Some(&src_box));
            // nits(scale) 변경 시에만 cbuffer 업로드 (매 프레임 중복 업로드 제거)
            if self.cbuffer_scale != scale_v {
                let scale = [scale_v, 0.0, 0.0, 0.0];
                self.context.UpdateSubresource(
                    &self.cbuffer,
                    0,
                    None,
                    scale.as_ptr() as *const _,
                    0,
                    0,
                );
                self.cbuffer_scale = scale_v;
            }

            let vp = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: cw as f32,
                Height: ch as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            self.context.RSSetViewports(Some(&[vp]));
            self.context.OMSetRenderTargets(Some(&[Some(rtv)]), None);
            self.context.VSSetShader(&self.vs, None);
            self.context.PSSetShader(&self.ps, None);
            self.context.PSSetShaderResources(0, Some(&[Some(srv)]));
            self.context
                .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.context
                .PSSetConstantBuffers(0, Some(&[Some(self.cbuffer.clone())]));
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.context.Draw(3, 0);

            self.context.PSSetShaderResources(0, Some(&[None]));
            self.context.OMSetRenderTargets(None, None);
            self.swapchain
                .Present(self.present_interval(), DXGI_PRESENT(0))
                .ok()?;
        }
        Ok(())
    }

    fn present_interval(&self) -> u32 {
        if self.vsync { 1 } else { 0 }
    }

    fn record_present_gap(&mut self) {
        let now = std::time::Instant::now();
        if let Some(last) = self.last_present {
            self.max_gap_ms = self.max_gap_ms.max((now - last).as_secs_f32() * 1000.0);
        }
        self.last_present = Some(now);
    }

    fn backbuffer(&mut self) -> windows::core::Result<ID3D11Texture2D> {
        if self.backbuffer.is_none() {
            let backbuffer = unsafe { self.swapchain.GetBuffer(0)? };
            self.backbuffer = Some(backbuffer);
        }
        Ok(self.backbuffer.as_ref().unwrap().clone())
    }

    fn render_target_view(&mut self) -> windows::core::Result<ID3D11RenderTargetView> {
        if self.rtv.is_none() {
            let backbuffer = self.backbuffer()?;
            let mut rtv = None;
            unsafe {
                self.device
                    .CreateRenderTargetView(&backbuffer, None, Some(&mut rtv))?;
            }
            self.rtv = rtv;
        }
        Ok(self.rtv.as_ref().unwrap().clone())
    }

    fn ensure_hdr_tex(&mut self, cw: i32, ch: i32) -> windows::core::Result<()> {
        if self.hdr_tex.is_some() && self.hdr_w == cw && self.hdr_h == ch {
            return Ok(());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: cw as u32,
            Height: ch as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        let tex = tex.unwrap();
        let mut srv = None;
        unsafe {
            self.device
                .CreateShaderResourceView(&tex, None, Some(&mut srv))?
        };
        self.hdr_tex = Some(tex);
        self.hdr_srv = srv;
        self.hdr_w = cw;
        self.hdr_h = ch;
        Ok(())
    }

    /// 미리보기용: staging 을 읽어 RGBA8 로 변환 후 공유 버퍼에 push (egui 표시용).
    /// 프레임 전체가 아닌 '크롭 영역만' GPU→CPU 복사 (1440p HDR 기준 회당 수십 MB 절감).
    fn update_preview(
        &mut self,
        src: &ID3D11Texture2D,
        _fw: i32,
        _fh: i32,
        crop: (i32, i32, i32, i32),
    ) -> windows::core::Result<()> {
        let (cx, cy, cw, ch) = crop;
        self.ensure_pv_staging(cw, ch)?;
        let cur = self.pv_cur;
        let prev = 1 - cur;
        let tex_cur = self.pv_tex[cur].clone().unwrap();
        // 이전 프레임(이미 GPU 복사 완료된) 텍스처를 매핑 → Map 이 GPU 를 기다리지 않음(stall 제거)
        let staging = self.pv_tex[prev].clone().unwrap();
        let src_box = D3D11_BOX {
            left: cx as u32,
            top: cy as u32,
            front: 0,
            right: (cx + cw) as u32,
            bottom: (cy + ch) as u32,
            back: 1,
        };
        unsafe {
            // 크롭 영역만 비동기 복사 (staging 은 크롭 크기)
            self.context
                .CopySubresourceRegion(&tex_cur, 0, 0, 0, 0, src, 0, Some(&src_box));
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }
        let (pw, ph) = preview_dimensions(cw, ch);
        let need = pw * ph * 4;
        if self.pv_scratch.len() < need {
            self.pv_scratch.resize(need, 0);
        }
        let rp = mapped.RowPitch as usize;
        let base = mapped.pData as usize;
        let lut = self.lut;
        let scale = 80.0f32 / self.nits.max(1.0);
        let hdr = self.hdr;
        let cwu = cw as usize;
        let chu = ch as usize;
        // 진단용 미리보기만 최대 960x540으로 샘플링해 원본 출력 경로의 CPU 부담을 제한한다.
        self.pv_scratch[..need]
            .chunks_mut(pw * 4)
            .enumerate()
            .for_each(|(row, d)| {
                let src_row = row * chu / ph;
                if hdr {
                    let srow = (base + src_row * rp) as *const u16;
                    for col in 0..pw {
                        let src_col = col * cwu / pw;
                        let r = half::f16::from_bits(unsafe { *srow.add(src_col * 4) }).to_f32()
                            * scale;
                        let g = half::f16::from_bits(unsafe { *srow.add(src_col * 4 + 1) })
                            .to_f32()
                            * scale;
                        let b = half::f16::from_bits(unsafe { *srow.add(src_col * 4 + 2) })
                            .to_f32()
                            * scale;
                        let o = col * 4;
                        d[o] = tone(&lut, r); // RGBA
                        d[o + 1] = tone(&lut, g);
                        d[o + 2] = tone(&lut, b);
                        d[o + 3] = 255;
                    }
                } else {
                    let srow = (base + src_row * rp) as *const u8;
                    for col in 0..pw {
                        let src_col = col * cwu / pw;
                        let s = unsafe { std::slice::from_raw_parts(srow.add(src_col * 4), 4) };
                        let o = col * 4;
                        d[o] = s[2]; // BGRA src → RGBA
                        d[o + 1] = s[1];
                        d[o + 2] = s[0];
                        d[o + 3] = 255;
                    }
                }
            });
        unsafe { self.context.Unmap(&staging, 0) };

        let rgba: Arc<[u8]> = Arc::from(&self.pv_scratch[..need]);
        if let Ok(mut p) = self.preview.lock() {
            p.w = pw;
            p.h = ph;
            p.rgba = rgba;
            p.version = p.version.wrapping_add(1);
        }
        self.pv_cur = prev; // 다음엔 반대 텍스처에 복사
        Ok(())
    }

    fn ensure_pv_staging(&mut self, fw: i32, fh: i32) -> windows::core::Result<()> {
        if self.pv_tex[0].is_some()
            && self.pv_sw == fw
            && self.pv_sh == fh
            && self.pv_sfp16 == self.hdr
        {
            return Ok(());
        }
        let fmt = if self.hdr {
            DXGI_FORMAT_R16G16B16A16_FLOAT
        } else {
            DXGI_FORMAT_B8G8R8A8_UNORM
        };
        let desc = D3D11_TEXTURE2D_DESC {
            Width: fw as u32,
            Height: fh as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: fmt,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        for i in 0..2 {
            let mut t = None;
            unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut t))? };
            self.pv_tex[i] = t;
        }
        self.pv_sw = fw;
        self.pv_sh = fh;
        self.pv_sfp16 = self.hdr;
        self.pv_cur = 0;
        Ok(())
    }

    fn ensure_size(&mut self, cw: i32, ch: i32) -> windows::core::Result<()> {
        if cw == self.w && ch == self.h {
            return Ok(());
        }
        self.w = cw;
        self.h = ch;
        self.rtv = None;
        self.backbuffer = None;
        unsafe {
            self.swapchain.ResizeBuffers(
                2,
                cw as u32,
                ch as u32,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                cw,
                ch,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        Ok(())
    }

    fn show(&mut self) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOP),
                self.saved_pos.0,
                self.saved_pos.1,
                self.w,
                self.h,
                SWP_SHOWWINDOW,
            );
            remove_taskbar_button(self.hwnd);
        }
    }

    fn hide(&mut self) {
        unsafe {
            // Discord가 공유 대상으로 찾을 수 있게 창은 visible 상태로 유지하되,
            // 사용자가 보기 전에는 가상 화면 바깥으로 옮긴다.
            let vx = GetSystemMetrics(SM_CXVIRTUALSCREEN) + GetSystemMetrics(SM_XVIRTUALSCREEN);
            let _ = SetWindowPos(
                self.hwnd,
                None,
                vx + 50,
                50,
                self.w,
                self.h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            remove_taskbar_button(self.hwnd);
        }
    }
}

/// 창은 Discord의 일반 창 열거에 남기고 작업표시줄 버튼만 제거한다.
/// WS_EX_TOOLWINDOW는 Discord 열거에서도 제외될 수 있어 사용하지 않는다.
unsafe fn remove_taskbar_button(hwnd: HWND) {
    unsafe {
        if let Ok(taskbar) =
            CoCreateInstance::<_, ITaskbarList>(&TaskbarList, None, CLSCTX_INPROC_SERVER)
        {
            let _ = taskbar.HrInit();
            let _ = taskbar.DeleteTab(hwnd);
        }
    }
}

fn preview_dimensions(w: i32, h: i32) -> (usize, usize) {
    const MAX_W: f32 = 960.0;
    const MAX_H: f32 = 540.0;
    let scale = (MAX_W / w as f32).min(MAX_H / h as f32).min(1.0);
    (
        (w as f32 * scale).round().max(1.0) as usize,
        (h as f32 * scale).round().max(1.0) as usize,
    )
}

// ---------- 헬퍼 ----------
fn create_pipeline(
    device: &ID3D11Device,
) -> windows::core::Result<(
    ID3D11VertexShader,
    ID3D11PixelShader,
    ID3D11SamplerState,
    ID3D11Buffer,
)> {
    unsafe {
        let vsb = compile_shader(SHADER_HLSL, s!("VSMain"), s!("vs_5_0"))?;
        let psb = compile_shader(SHADER_HLSL, s!("PSMain"), s!("ps_5_0"))?;

        let mut vs = None;
        device.CreateVertexShader(blob_bytes(&vsb), None, Some(&mut vs))?;
        let mut ps = None;
        device.CreatePixelShader(blob_bytes(&psb), None, Some(&mut ps))?;

        let sd = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MipLODBias: 0.0,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            BorderColor: [0.0; 4],
            MinLOD: 0.0,
            MaxLOD: f32::MAX,
        };
        let mut sampler = None;
        device.CreateSamplerState(&sd, Some(&mut sampler))?;

        let bd = D3D11_BUFFER_DESC {
            ByteWidth: 16,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut cb = None;
        device.CreateBuffer(&bd, None, Some(&mut cb))?;

        Ok((vs.unwrap(), ps.unwrap(), sampler.unwrap(), cb.unwrap()))
    }
}

fn compile_shader(src: &str, entry: PCSTR, target: PCSTR) -> windows::core::Result<ID3DBlob> {
    let mut blob = None;
    let mut err = None;
    let r = unsafe {
        D3DCompile(
            src.as_ptr() as *const _,
            src.len(),
            PCSTR::null(),
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut blob,
            Some(&mut err),
        )
    };
    if r.is_err() {
        if let Some(e) = &err {
            let msg = unsafe {
                std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize())
            };
            eprintln!("shader compile error: {}", String::from_utf8_lossy(msg));
        }
        r?;
    }
    Ok(blob.unwrap())
}

fn blob_bytes(b: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }
}

#[inline]
fn tone(lut: &[u8; 1024], lin: f32) -> u8 {
    let i = if lin <= 0.0 {
        0
    } else if lin >= 1.0 {
        1023
    } else {
        (lin * 1023.0) as usize
    };
    lut[i]
}

fn create_d3d_device() -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
    for dt in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let r = unsafe {
            D3D11CreateDevice(
                None,
                dt,
                HMODULE::default(),
                flags,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        if r.is_ok() {
            break;
        }
    }
    Ok((device.unwrap(), context.unwrap()))
}

fn create_winrt_device(device: &ID3D11Device) -> windows::core::Result<IDirect3DDevice> {
    let dxgi: IDXGIDevice = device.cast()?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? };
    inspectable.cast()
}

fn create_swapchain(
    device: &ID3D11Device,
    hwnd: HWND,
    w: i32,
    h: i32,
) -> windows::core::Result<IDXGISwapChain1> {
    let dxgi: IDXGIDevice = device.cast()?;
    let adapter = unsafe { dxgi.GetAdapter()? };
    let factory: IDXGIFactory2 = unsafe { adapter.GetParent()? };
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: w as u32,
        Height: h as u32,
        // WGC SDR 소스(BGRA8) 및 ResizeBuffers 와 통일 → 초기 프레임 포맷 불일치 복사 방지
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: 0,
    };
    unsafe { factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None) }
}

fn create_output_window() -> windows::core::Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class_name = w!("GameOutputWindowClass");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("GameOutput"),
            WS_POPUP | WS_VISIBLE,
            120,
            120,
            960,
            540,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        Ok(hwnd)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

fn crop_rect(target: isize, fw: i32, fh: i32) -> Option<(i32, i32, i32, i32)> {
    if target == 0 {
        return None;
    }
    let hwnd = HWND(target as *mut _);
    unsafe {
        let mut cr = RECT::default();
        GetClientRect(hwnd, &mut cr).ok()?;
        let cw = cr.right - cr.left;
        let ch = cr.bottom - cr.top;
        if cw <= 0 || ch <= 0 {
            return None;
        }
        let mut tl = POINT { x: 0, y: 0 };
        if !ClientToScreen(hwnd, &mut tl).as_bool() {
            return None;
        }
        let mut efb = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut efb as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()?;

        let mut x = (tl.x - efb.left).max(0);
        let mut y = (tl.y - efb.top).max(0);
        let mut w = cw;
        let mut h = ch;
        if x + w > fw {
            w = fw - x;
        }
        if y + h > fh {
            h = fh - y;
        }
        let _ = (&mut x, &mut y);
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((x, y, w, h))
    }
}

fn crop_to_aspect(
    rect: (i32, i32, i32, i32),
    aspect_w: f32,
    aspect_h: f32,
    align_x: i32,
    align_y: i32,
) -> (i32, i32, i32, i32) {
    let (mut x, mut y, mut w, mut h) = rect;
    if w <= 0 || h <= 0 || aspect_w <= 0.0 || aspect_h <= 0.0 {
        return rect;
    }

    let target = aspect_w as f64 / aspect_h as f64;
    let current = w as f64 / h as f64;
    if current > target {
        let new_w = ((h as f64 * target).round() as i32).clamp(1, w);
        let dx = w - new_w;
        x += match align_x.clamp(0, 2) {
            0 => 0,
            2 => dx,
            _ => dx / 2,
        };
        w = new_w;
    } else if current < target {
        let new_h = ((w as f64 / target).round() as i32).clamp(1, h);
        let dy = h - new_h;
        y += match align_y.clamp(0, 2) {
            0 => 0,
            2 => dy,
            _ => dy / 2,
        };
        h = new_h;
    }
    (x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::crop_to_aspect;

    #[test]
    fn horizontal_crop_uses_left_center_and_right_alignment() {
        let rect = (0, 0, 200, 100);
        assert_eq!(crop_to_aspect(rect, 1.0, 1.0, 0, 1), (0, 0, 100, 100));
        assert_eq!(crop_to_aspect(rect, 1.0, 1.0, 1, 1), (50, 0, 100, 100));
        assert_eq!(crop_to_aspect(rect, 1.0, 1.0, 2, 1), (100, 0, 100, 100));
    }

    #[test]
    fn vertical_crop_uses_top_center_and_bottom_alignment() {
        let rect = (0, 0, 100, 200);
        assert_eq!(crop_to_aspect(rect, 1.0, 1.0, 1, 0), (0, 0, 100, 100));
        assert_eq!(crop_to_aspect(rect, 1.0, 1.0, 1, 1), (0, 50, 100, 100));
        assert_eq!(crop_to_aspect(rect, 1.0, 1.0, 1, 2), (0, 100, 100, 100));
    }
}

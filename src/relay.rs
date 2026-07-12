//! 게임 오디오 Process Loopback 캡처 → 선택한 출력장치로 재생(relay).
//! 우리 앱 프로세스에서 재생되므로 Discord 가 'GameOutput' 창을 공유할 때 게임 소리도 함께 잡힌다.
//! relay 출력을 '안 듣는 장치'로 보내면 로컬 2중 재생을 피할 수 있다.

use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantToStringAlloc};
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_core::{Interface, implement};

pub struct OutputDevice {
    pub id: String,
    pub name: String,
}

/// 활성 렌더 출력장치 목록 (relay 출력 대상 선택용).
pub fn list_output_devices() -> Vec<OutputDevice> {
    let mut out = Vec::new();
    unsafe {
        let Ok(enumerator): windows::core::Result<IMMDeviceEnumerator> =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return out;
        };
        let Ok(coll) = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
            return out;
        };
        let count = coll.GetCount().unwrap_or(0);
        for i in 0..count {
            let Ok(dev) = coll.Item(i) else { continue };
            let id = dev
                .GetId()
                .ok()
                .and_then(|p| p.to_string().ok())
                .unwrap_or_default();
            let name = friendly_name(&dev).unwrap_or_else(|| "(알 수 없음)".into());
            if !id.is_empty() {
                out.push(OutputDevice { id, name });
            }
        }
    }
    out
}

fn friendly_name(dev: &IMMDevice) -> Option<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    let store = unsafe { dev.OpenPropertyStore(STGM_READ).ok()? };
    let prop = unsafe { store.GetValue(&PKEY_Device_FriendlyName).ok()? };
    let s = unsafe { PropVariantToStringAlloc(&prop).ok()? };
    let out = unsafe { s.to_string().ok() };
    unsafe {
        windows::Win32::System::Com::CoTaskMemFree(Some(s.0 as *const _));
    }
    out
}

pub enum RelayCmd {
    Start { pid: u32 },
    Stop,
    SetDevice(Option<String>),
    Quit,
}

pub struct RelayHandle {
    tx: Sender<RelayCmd>,
}

impl RelayHandle {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("relay".into())
            .spawn(move || run(rx));
        Self { tx }
    }
    pub fn start(&self, pid: u32) {
        let _ = self.tx.send(RelayCmd::Start { pid });
    }
    pub fn stop(&self) {
        let _ = self.tx.send(RelayCmd::Stop);
    }
    pub fn set_device(&self, id: Option<String>) {
        let _ = self.tx.send(RelayCmd::SetDevice(id));
    }
}

impl Drop for RelayHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(RelayCmd::Quit);
    }
}

fn run(rx: Receiver<RelayCmd>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED); // ActivateAudioInterfaceAsync 는 MTA
    }
    let mut device_id: Option<String> = None;
    let mut want_pid: Option<u32> = None;
    let mut session: Option<RelaySession> = None;
    let mut next_try = Instant::now();

    loop {
        let mut rebuild = false;
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                RelayCmd::Quit => return,
                RelayCmd::Start { pid } => {
                    want_pid = Some(pid);
                    rebuild = true;
                }
                RelayCmd::Stop => {
                    want_pid = None;
                    session = None;
                }
                RelayCmd::SetDevice(id) => {
                    device_id = id;
                    if want_pid.is_some() {
                        rebuild = true;
                    }
                }
            }
        }
        if rebuild {
            session = None;
            next_try = Instant::now();
        }

        if session.is_none()
            && let Some(pid) = want_pid
            && Instant::now() >= next_try
        {
            match RelaySession::new(pid, device_id.clone()) {
                Ok(s) => session = Some(s),
                Err(e) => {
                    eprintln!("relay start: {e:?}");
                    next_try = Instant::now() + Duration::from_millis(1000); // 스핀 방지 후 재시도
                }
            }
        }

        if let Some(s) = &session {
            if let Err(e) = s.pump() {
                eprintln!("relay pump: {e:?}");
                session = None;
                next_try = Instant::now() + Duration::from_millis(500);
            }
            std::thread::sleep(Duration::from_millis(5));
        } else {
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

struct RelaySession {
    capture_client: IAudioClient,
    render_client: IAudioClient,
    capture: IAudioCaptureClient,
    render: IAudioRenderClient,
    render_bufsize: u32,
    frame_bytes: u32,
}

impl RelaySession {
    fn new(pid: u32, device_id: Option<String>) -> windows::core::Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = match &device_id {
                Some(id) => enumerator.GetDevice(&windows::core::HSTRING::from(id))?,
                None => enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?,
            };

            // ── 렌더(출력) 클라이언트 ──
            let render_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let wfx = render_client.GetMixFormat()?; // *mut WAVEFORMATEX (CoTaskMem)
            let frame_bytes = (*wfx).nBlockAlign as u32;
            let buf_dur: i64 = 200_000; // 20ms (100ns 단위)
            render_client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, buf_dur, 0, wfx, None)?;
            let render_bufsize = render_client.GetBufferSize()?;
            let render: IAudioRenderClient = render_client.GetService()?;

            // ── 게임 Process Loopback 캡처 클라이언트 ──
            let capture_client = activate_loopback(pid)?;
            capture_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                buf_dur,
                0,
                wfx, // 렌더와 동일 포맷 → 리샘플 불필요
                None,
            )?;
            let capture: IAudioCaptureClient = capture_client.GetService()?;

            CoTaskMemFree(Some(wfx as *const _));

            render_client.Start()?;
            capture_client.Start()?;

            Ok(Self {
                capture_client,
                render_client,
                capture,
                render,
                render_bufsize,
                frame_bytes,
            })
        }
    }

    fn pump(&self) -> windows::core::Result<()> {
        unsafe {
            loop {
                let packet = self.capture.GetNextPacketSize()?;
                if packet == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames: u32 = 0;
                let mut flags: u32 = 0;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;

                if frames > 0 {
                    let padding = self.render_client.GetCurrentPadding()?;
                    let avail = self.render_bufsize.saturating_sub(padding);
                    let towrite = frames.min(avail);
                    if towrite > 0 {
                        let rbuf = self.render.GetBuffer(towrite)?;
                        if !silent && !data.is_null() {
                            std::ptr::copy_nonoverlapping(
                                data,
                                rbuf,
                                (towrite * self.frame_bytes) as usize,
                            );
                        }
                        let rflags = if silent {
                            AUDCLNT_BUFFERFLAGS_SILENT.0 as u32
                        } else {
                            0
                        };
                        self.render.ReleaseBuffer(towrite, rflags)?;
                    }
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    }
}

impl Drop for RelaySession {
    fn drop(&mut self) {
        unsafe {
            let _ = self.capture_client.Stop();
            let _ = self.render_client.Stop();
        }
    }
}

// ---------- Process Loopback 활성화 ----------
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActHandler(isize); // event HANDLE 을 isize 로 보관(Send 안전)

impl IActivateAudioInterfaceCompletionHandler_Impl for ActHandler_Impl {
    fn ActivateCompleted(
        &self,
        _op: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        unsafe {
            let _ = windows::Win32::System::Threading::SetEvent(HANDLE(self.0 as *mut _));
        }
        Ok(())
    }
}

// VT_BLOB PROPVARIANT 를 직접 구성 (windows-rs 에 안전한 생성자가 없음)
#[repr(C)]
struct PropVariantBlob {
    vt: u16,
    _r1: u16,
    _r2: u16,
    _r3: u16,
    cb_size: u32,
    _pad: u32,
    p_blob: *mut std::ffi::c_void,
}

fn activate_loopback(pid: u32) -> windows::core::Result<IAudioClient> {
    unsafe {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let pv = PropVariantBlob {
            vt: 65, // VT_BLOB
            _r1: 0,
            _r2: 0,
            _r3: 0,
            cb_size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            _pad: 0,
            p_blob: &mut params as *mut _ as *mut _,
        };

        let event = CreateEventW(None, false, false, None)?;
        let handler: IActivateAudioInterfaceCompletionHandler = ActHandler(event.0 as isize).into();

        let op = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&pv as *const _ as *const PROPVARIANT),
            &handler,
        )?;

        let waited = WaitForSingleObject(event, 5000);
        let _ = windows::Win32::Foundation::CloseHandle(event);
        if waited != WAIT_OBJECT_0 {
            // 활성화 완료 통지를 못 받음(타임아웃 등) → 미완료 결과를 만지지 않고 에러 반환
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_FAIL,
                "process loopback activation timed out",
            ));
        }

        let mut hr = windows::core::HRESULT(0);
        let mut unk: Option<windows::core::IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unk)?;
        hr.ok()?;
        // unwrap 패닉 방지: None 이면 에러로
        let unk = unk.ok_or_else(|| {
            windows::core::Error::new(windows::Win32::Foundation::E_POINTER, "null audio client")
        })?;
        unk.cast()
    }
}

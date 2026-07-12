//! 별도 네이티브 창(직접 만든 Win32 창)에 WebView2(wry)를 자식으로 붙여 TIDAL 재생.
//! tao/winit 이벤트루프를 안 쓰고 우리가 메시지를 펌프한다(메인 eframe 루프와 충돌 회피).
//! 우리 앱 프로세스에서 재생 → Discord 가 'GameOutput' 공유 시 소리가 함께 잡힘.

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use std::num::NonZeroIsize;
use std::sync::mpsc::{Receiver, Sender};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;
use wry::{WebViewBuilder, WebViewBuilderExtWindows};

pub enum TidalCmd {
    Navigate(String),
    Html(String),
    Show,
    Hide,
    Quit,
}

pub struct TidalHandle {
    tx: Sender<TidalCmd>,
}

impl TidalHandle {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("tidal".into())
            .spawn(move || {
                if let Err(e) = tidal_thread(rx) {
                    eprintln!("tidal thread: {e:?}");
                }
            });
        Self { tx }
    }
    pub fn navigate(&self, url: &str) {
        let _ = self.tx.send(TidalCmd::Navigate(url.to_string()));
    }
    pub fn html(&self, html: String) {
        let _ = self.tx.send(TidalCmd::Html(html));
    }
    pub fn show(&self) {
        let _ = self.tx.send(TidalCmd::Show);
    }
    pub fn hide(&self) {
        let _ = self.tx.send(TidalCmd::Hide);
    }
}

impl Drop for TidalHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(TidalCmd::Quit);
    }
}

struct RawWin(isize);
impl HasWindowHandle for RawWin {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
        let handle = Win32WindowHandle::new(hwnd);
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}

fn tidal_thread(rx: Receiver<TidalCmd>) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); // WebView2 는 STA
    }
    let hwnd = create_window()?;
    let raw = RawWin(hwnd.0 as isize);

    let tidal_url = "https://listen.tidal.com";
    let webview = WebViewBuilder::new()
        .with_additional_browser_args("--disable-features=AudioServiceOutOfProcess")
        .with_url(tidal_url)
        .build(&raw)?;
    // 빌드 시 이미 TIDAL 로드됨 → 같은 URL 재탐색을 막아 'TIDAL 열기' 시 새로고침 방지
    let mut last_url = tidal_url.to_string();

    let mut msg = MSG::default();
    loop {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                TidalCmd::Quit => return Ok(()),
                TidalCmd::Navigate(url) => {
                    if url != last_url {
                        let _ = webview.load_url(&url);
                        last_url = url;
                    }
                }
                TidalCmd::Html(html) => {
                    let _ = webview.load_html(&html);
                    last_url = "about:tone".to_string();
                }
                TidalCmd::Show => unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = SetForegroundWindow(hwnd);
                },
                TidalCmd::Hide => unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                },
            }
        }

        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // 2ms 폴링(상시 500 wakeup/s) 대신 '메시지 도착 or 30ms' 대기.
            // 채널 명령(Show/Navigate)은 최대 30ms 지연 — 버튼 조작엔 체감 없음.
            let _ = MsgWaitForMultipleObjects(None, false, 30, QS_ALLINPUT);
        }
    }
}

fn create_window() -> windows::core::Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class_name = w!("TidalAudioWindowClass");
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
            w!("TIDAL — 오디오 소스 (나만 봄, 공유 안 됨)"),
            WS_OVERLAPPEDWINDOW, // 일반 창(제목/닫기/리사이즈), 시작은 숨김
            120,
            120,
            480,
            820,
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
        WM_CLOSE => {
            // 닫지 말고 숨김 (앱이 수명 관리)
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

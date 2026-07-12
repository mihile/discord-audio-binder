#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod capture;
mod relay;
mod settings;
mod sysinfo;
mod tidal;

use eframe::egui;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

/// 패닉을 %APPDATA%\discord-audio-binder\crash.log 에 기록.
/// 윈도우드 릴리스는 콘솔이 없어 패닉 메시지가 유실되므로, 위치(file:line)+메시지를 파일로 남긴다.
fn install_panic_logger() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "?".into());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "(non-string panic)".into()
        };
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = format!("[epoch {secs}] thread '{thread}' panicked at {loc}: {msg}\n");
        if let Ok(base) = std::env::var("APPDATA") {
            let mut p = std::path::PathBuf::from(base);
            p.push("discord-audio-binder");
            let _ = std::fs::create_dir_all(&p);
            p.push("crash.log");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
            {
                use std::io::Write;
                let _ = f.write_all(line.as_bytes());
            }
        }
        default(info);
    }));
}

fn main() -> eframe::Result<()> {
    install_panic_logger();

    // Core Audio / 향후 WGC interop 용 COM 초기화 (메인 스레드).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon.png"))
        .expect("embedded app icon must be a valid PNG");
    let options = eframe::NativeOptions {
        // glow(OpenGL) 렌더러: 게임 실행 시 디스플레이/GPU 리셋에 wgpu 가 패닉하던 문제 회피
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(icon)
            .with_title("Discord Audio Source Binder"),
        ..Default::default()
    };

    eframe::run_native(
        "Discord Audio Source Binder",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

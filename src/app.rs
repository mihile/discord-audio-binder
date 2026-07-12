//! egui 컨트롤 UI.

use eframe::egui::{self, Color32, RichText, Stroke};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::audio::{AudioMonitor, AudioSession};
use crate::capture::CaptureHandle;
use crate::relay::{self, OutputDevice, RelayHandle};
use crate::settings::Settings;
use crate::sysinfo::{self, WindowInfo};
use crate::tidal::TidalHandle;

const TONE_HTML: &str = r#"<!doctype html><html><body style="background:#111;color:#eee;font-family:sans-serif;text-align:center;padding-top:60px">
<h2>WebView2 Test Tone</h2>
<button id="b" style="font-size:22px;padding:14px 28px">&#9654; 440Hz 재생/정지</button>
<p id="s">stopped</p>
<script>
let ctx,osc,on=false;
document.getElementById('b').onclick=()=>{
  if(!ctx)ctx=new(window.AudioContext||window.webkitAudioContext)();
  if(on){osc.stop();on=false;document.getElementById('s').textContent='stopped';return;}
  osc=ctx.createOscillator();const g=ctx.createGain();
  g.gain.value=0.15;osc.frequency.value=440;
  osc.connect(g).connect(ctx.destination);osc.start();on=true;
  document.getElementById('s').textContent='playing 440Hz';
};
</script></body></html>"#;

const ACCENT: Color32 = Color32::from_rgb(0x58, 0x65, 0xF2); // Discord blurple
const GREEN: Color32 = Color32::from_rgb(0x57, 0xF2, 0x87);
const GREY: Color32 = Color32::from_rgb(0x9A, 0xA0, 0xA6);
const PANEL: Color32 = Color32::from_rgb(0x2B, 0x2D, 0x31);

pub struct App {
    audio: Option<AudioMonitor>,
    windows: Vec<WindowInfo>,
    selected_hwnd: Option<isize>,
    filter: String,

    self_pid: u32,
    self_tree: HashSet<u32>,
    proc_names: HashMap<u32, String>,
    last_audio: Vec<AudioSession>,
    last_audio_tick: Instant,
    last_proc_tick: Instant,

    // 캡처/옵션 상태
    capture: CaptureHandle,
    relay: RelayHandle,
    relay_devices: Vec<OutputDevice>,
    relay_device_id: Option<String>,
    tidal: Option<TidalHandle>,
    preview: std::sync::Arc<std::sync::Mutex<crate::capture::Preview>>,
    preview_tex: Option<egui::TextureHandle>,
    preview_ver: u64,
    pub mirroring: bool,
    pub crop_titlebar: bool,
    pub crop_16_9: bool,
    pub crop_aspect_w: f32,
    pub crop_aspect_h: f32,
    pub crop_align: i32,
    pub vsync: bool,
    pub output_fps: f32,
    pub hdr_fix: bool,
    pub hdr_nits: f32,
    pub tidal_audio: bool,
    pub game_audio: bool,
    pub tidal_volume: f32,
    pub show_output: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_theme(&cc.egui_ctx);
        let snap = sysinfo::snapshot_processes();
        let self_pid = std::process::id();
        let self_tree = sysinfo::descendants_and_self(&snap.parent, self_pid);
        let windows = sysinfo::enumerate_windows(&snap.names);
        let capture_handle = CaptureHandle::spawn();
        let relay_handle = RelayHandle::spawn();
        let relay_devices = relay::list_output_devices();
        let preview = capture_handle.preview();

        // 영속 설정 로드 후 캡처에 적용 (미러링 대상은 저장/복원하지 않음)
        let s = Settings::load();
        capture_handle.set_crop(s.crop_titlebar);
        capture_handle.set_aspect_crop(s.crop_16_9);
        capture_handle.set_aspect(s.crop_aspect_w, s.crop_aspect_h);
        capture_handle.set_aspect_align(s.crop_align);
        capture_handle.set_vsync(s.vsync);
        capture_handle.set_output_fps(s.output_fps);
        capture_handle.set_hdr(s.hdr_fix);
        capture_handle.set_nits(s.hdr_nits);

        Self {
            audio: AudioMonitor::new().ok(),
            windows,
            selected_hwnd: None,
            filter: String::new(),
            self_pid,
            self_tree,
            proc_names: snap.names,
            last_audio: Vec::new(),
            last_audio_tick: Instant::now(),
            last_proc_tick: Instant::now(),
            capture: capture_handle,
            relay: relay_handle,
            relay_devices,
            relay_device_id: s.relay_device_id.clone(),
            tidal: Some(TidalHandle::spawn()),
            preview,
            preview_tex: None,
            preview_ver: 0,
            mirroring: false,
            crop_titlebar: s.crop_titlebar,
            crop_16_9: s.crop_16_9,
            crop_aspect_w: s.crop_aspect_w,
            crop_aspect_h: s.crop_aspect_h,
            crop_align: s.crop_align,
            vsync: s.vsync,
            output_fps: s.output_fps,
            hdr_fix: s.hdr_fix,
            hdr_nits: s.hdr_nits,
            tidal_audio: s.tidal_audio,
            game_audio: s.game_audio,
            tidal_volume: s.volume,
            show_output: false,
        }
    }

    fn save_settings(&self) {
        Settings {
            hdr_fix: self.hdr_fix,
            hdr_nits: self.hdr_nits,
            crop_titlebar: self.crop_titlebar,
            crop_16_9: self.crop_16_9,
            crop_aspect_w: self.crop_aspect_w,
            crop_aspect_h: self.crop_aspect_h,
            crop_align: self.crop_align,
            vsync: self.vsync,
            output_fps: self.output_fps,
            volume: self.tidal_volume,
            tidal_audio: self.tidal_audio,
            game_audio: self.game_audio,
            relay_device_id: self.relay_device_id.clone(),
        }
        .save();
    }

    fn selected_game_pid(&self) -> Option<u32> {
        let hwnd = self.selected_hwnd?;
        self.windows.iter().find(|w| w.hwnd == hwnd).map(|w| w.pid)
    }

    fn effective_tidal_volume(&self) -> f32 {
        if self.tidal_audio {
            self.tidal_volume
        } else {
            0.0
        }
    }

    fn sync_game_audio_relay(&self) {
        self.relay.set_device(self.relay_device_id.clone());
        if self.game_audio
            && let Some(pid) = self.selected_game_pid()
        {
            self.relay.start(pid);
        } else {
            self.relay.stop();
        }
    }

    fn refresh_relay_devices(&mut self) {
        self.relay_devices = relay::list_output_devices();
        if let Some(id) = &self.relay_device_id
            && !self.relay_devices.iter().any(|d| &d.id == id)
        {
            self.relay_device_id = None;
        }
        self.sync_game_audio_relay();
        self.save_settings();
    }

    fn refresh_windows(&mut self) {
        let snap = sysinfo::snapshot_processes();
        self.self_tree = sysinfo::descendants_and_self(&snap.parent, self.self_pid);
        self.windows = sysinfo::enumerate_windows(&snap.names);
        self.proc_names = snap.names;
        self.last_proc_tick = Instant::now();
    }

    fn tick(&mut self) {
        if self.last_proc_tick.elapsed() >= Duration::from_millis(1500) {
            let snap = sysinfo::snapshot_processes();
            self.self_tree = sysinfo::descendants_and_self(&snap.parent, self.self_pid);
            self.proc_names = snap.names;
            self.last_proc_tick = Instant::now();
        }

        if self.last_audio_tick.elapsed() >= Duration::from_millis(400) {
            if let Some(a) = &self.audio {
                // 세션 열거 1회로 스냅샷 + 트리 볼륨 적용(늦게 뜨는 WebView2 세션도 지속 반영)
                self.last_audio = a.snapshot_with_volume(
                    &self.self_tree,
                    self.effective_tidal_volume(),
                    self.self_pid, // 우리 자신(게임 relay 재생)은 TIDAL 볼륨에서 제외
                );
            }
            self.last_audio_tick = Instant::now();
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick();
        // 미리보기는 2fps, fps 라벨은 1/s 갱신이라 200ms(5fps) 리페인트면 충분
        if self.mirroring {
            ctx.request_repaint_after(Duration::from_millis(200));
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        top_bar(ui);
        left_panel(ui, self);
        right_panel(ui, self);
        center_panel(ui, self);
    }
}

fn top_bar(ui: &mut egui::Ui) {
    egui::Panel::top("top")
        .exact_size(52.0)
        .frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(0x1E, 0x1F, 0x22))
                .inner_margin(12.0),
        )
        .show_inside(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(
                    RichText::new("🎧 Discord Audio Source Binder")
                        .size(20.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.add_space(10.0);
                ui.label(RichText::new("게임 화면 + TIDAL 소리를 한 창으로").color(GREY));
            });
        });
}

fn left_panel(ui: &mut egui::Ui, app: &mut App) {
    egui::Panel::left("left")
        .exact_size(340.0)
        .frame(panel_frame())
        .show_inside(ui, |ui| {
            section(ui, "① 게임 창 선택");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut app.filter)
                        .hint_text("필터…")
                        .desired_width(220.0),
                );
                if ui.button("새로고침").clicked() {
                    app.refresh_windows();
                }
            });
            ui.add_space(4.0);

            let filter = app.filter.to_lowercase();
            let windows = &app.windows;
            let self_pid = app.self_pid;
            let selected = app.selected_hwnd;
            let mut clicked: Option<isize> = None;

            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    let mut any = false;
                    for w in windows.iter().filter(|w| w.pid != self_pid).filter(|w| {
                        filter.is_empty()
                            || w.title.to_lowercase().contains(&filter)
                            || w.process.to_lowercase().contains(&filter)
                    }) {
                        any = true;
                        let label = format!("{}\n   {} · pid {}", w.title, w.process, w.pid);
                        if ui
                            .selectable_label(
                                selected == Some(w.hwnd),
                                RichText::new(label).size(12.5),
                            )
                            .clicked()
                        {
                            clicked = Some(w.hwnd);
                        }
                    }
                    if !any {
                        ui.colored_label(GREY, "창이 없습니다. 새로고침을 눌러보세요.");
                    }
                });
            if let Some(h) = clicked {
                app.selected_hwnd = Some(h);
                app.sync_game_audio_relay();
            }

            ui.add_space(10.0);
            section(ui, "② 미러링");
            ui.horizontal(|ui| {
                let start = ui.add_enabled(
                    app.selected_hwnd.is_some(),
                    egui::Button::new("▶ 미러링 시작").fill(ACCENT),
                );
                if start.clicked()
                    && let Some(hwnd) = app.selected_hwnd
                {
                    app.capture.set_crop(app.crop_titlebar);
                    app.capture.set_aspect_crop(app.crop_16_9);
                    app.capture.set_aspect(app.crop_aspect_w, app.crop_aspect_h);
                    app.capture.set_aspect_align(app.crop_align);
                    app.capture.start(hwnd);
                    app.sync_game_audio_relay();
                    app.mirroring = true;
                }
                if ui.button("■ 중지").clicked() {
                    app.capture.stop();
                    app.mirroring = false;
                }
            });
            ui.colored_label(
                GREY,
                RichText::new(
                    "미러링 시작 시 'GameOutput' 출력 창이 캡처를 시작합니다(기본 숨김).",
                )
                .size(11.0),
            );

            ui.add_space(6.0);
            if ui
                .checkbox(&mut app.crop_titlebar, "상단바(제목표시줄) 제외")
                .changed()
            {
                app.capture.set_crop(app.crop_titlebar);
                app.save_settings();
            }
            if ui
                .checkbox(&mut app.crop_16_9, "비율 크롭")
                .on_hover_text(
                    "선택한 창의 캡처 영역을 지정한 비율로 잘라 GameOutput에 출력합니다.",
                )
                .changed()
            {
                app.capture.set_aspect_crop(app.crop_16_9);
                app.save_settings();
            }
            ui.add_enabled_ui(app.crop_16_9, |ui| {
                let mut changed = false;
                ui.horizontal(|ui| {
                    ui.label("크롭 비율");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut app.crop_aspect_w)
                                .range(1.0..=64.0)
                                .speed(0.25)
                                .prefix("가로 "),
                        )
                        .changed();
                    ui.label(":");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut app.crop_aspect_h)
                                .range(1.0..=64.0)
                                .speed(0.25)
                                .prefix("세로 "),
                        )
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("정렬");
                    changed |= ui
                        .selectable_value(&mut app.crop_align, 0, "왼쪽")
                        .changed();
                    changed |= ui
                        .selectable_value(&mut app.crop_align, 1, "중앙")
                        .changed();
                    changed |= ui
                        .selectable_value(&mut app.crop_align, 2, "우측")
                        .changed();
                });
                if changed {
                    app.crop_aspect_w = app.crop_aspect_w.clamp(1.0, 64.0);
                    app.crop_aspect_h = app.crop_aspect_h.clamp(1.0, 64.0);
                    app.crop_align = app.crop_align.clamp(0, 2);
                    app.capture.set_aspect(app.crop_aspect_w, app.crop_aspect_h);
                    app.capture.set_aspect_align(app.crop_align);
                    app.save_settings();
                }
            });
            if ui
                .checkbox(&mut app.vsync, "출력 VSync")
                .on_hover_text("끊겨 보이면 꺼서 출력 창 Present 대기를 줄여보세요.")
                .changed()
            {
                app.capture.set_vsync(app.vsync);
                app.save_settings();
            }
            ui.horizontal(|ui| {
                ui.label("출력 FPS");
                let resp = ui.add(
                    egui::Slider::new(&mut app.output_fps, 30.0..=180.0)
                        .step_by(30.0)
                        .suffix(" fps"),
                );
                if resp.changed() {
                    app.capture.set_output_fps(app.output_fps);
                    app.save_settings();
                }
            });
            if ui
                .checkbox(&mut app.hdr_fix, "HDR 보정 (하얗게 나올 때)")
                .changed()
            {
                app.capture.set_hdr(app.hdr_fix);
                app.save_settings();
            }
            ui.add_enabled_ui(app.hdr_fix, |ui| {
                ui.horizontal(|ui| {
                    ui.label("HDR 밝기");
                    if ui
                        .add(egui::Slider::new(&mut app.hdr_nits, 80.0..=480.0).suffix(" nit"))
                        .changed()
                    {
                        app.capture.set_nits(app.hdr_nits);
                        app.save_settings();
                    }
                });
            });

            ui.add_space(4.0);
            let status = if app.mirroring {
                RichText::new("● 미러링 중").color(GREEN)
            } else {
                RichText::new("○ 정지").color(GREY)
            };
            ui.label(status);
        });
}

fn right_panel(ui: &mut egui::Ui, app: &mut App) {
    egui::Panel::right("right")
        .exact_size(380.0)
        .frame(panel_frame())
        .show_inside(ui, |ui| {
            section(ui, "⑤ 공유용 출력 창 (게임 화면만)");
            ui.horizontal(|ui| {
                if ui.checkbox(&mut app.show_output, "출력 창 화면에 보이기").changed() {
                    if app.show_output {
                        app.capture.show();
                    } else {
                        app.capture.hide();
                    }
                }
            });
            hint(ui, "Discord 에는 'GameOutput' 창을 공유하세요. TIDAL 은 이 메인 창에서만 재생되어(나만 봄) 공유되지 않지만, 같은 프로세스라 소리는 함께 송출됩니다.");

            ui.add_space(8.0);
            section(ui, "⑥ 송출 오디오");
            if ui
                .checkbox(&mut app.game_audio, "게임 오디오 송출")
                .on_hover_text("선택한 게임 프로세스의 소리를 이 앱에서 다시 재생해 Discord 창 공유 오디오에 포함합니다.")
                .changed()
            {
                app.sync_game_audio_relay();
                app.save_settings();
            }
            ui.add_enabled_ui(app.game_audio, |ui| {
                let devices: Vec<(String, String)> = app
                    .relay_devices
                    .iter()
                    .map(|d| (d.id.clone(), d.name.clone()))
                    .collect();
                let selected_name = app
                    .relay_device_id
                    .as_ref()
                    .and_then(|id| devices.iter().find(|(dev_id, _)| dev_id == id))
                    .map(|(_, name)| name.as_str())
                    .unwrap_or("기본 출력 장치");
                let mut changed = false;
                egui::ComboBox::from_id_salt("relay_output_device")
                    .selected_text(selected_name)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(app.relay_device_id.is_none(), "기본 출력 장치")
                            .clicked()
                        {
                            app.relay_device_id = None;
                            changed = true;
                        }
                        for (id, name) in devices {
                            if ui
                                .selectable_label(app.relay_device_id.as_ref() == Some(&id), name)
                                .clicked()
                            {
                                app.relay_device_id = Some(id);
                                changed = true;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("장치 새로고침").clicked() {
                        app.refresh_relay_devices();
                    }
                    hint(ui, "2중으로 들리면 VB-CABLE 같은 안 듣는 출력장치를 선택하세요.");
                });
                if changed {
                    app.sync_game_audio_relay();
                    app.save_settings();
                }
            });
            if ui
                .checkbox(&mut app.tidal_audio, "TIDAL 오디오 송출")
                .changed()
            {
                if let Some(a) = &app.audio {
                    a.set_tree_volume(&app.self_tree, app.effective_tidal_volume(), app.self_pid);
                }
                app.save_settings();
            }

            ui.add_space(8.0);
            section(ui, "④ TIDAL / 오디오 소스 (나만 보는 별도 창)");
            hint(ui, "TIDAL은 별도 'TIDAL' 창에서 재생됩니다(나만 봄, Discord엔 공유 안 됨). 같은 프로세스라 'GameOutput' 창을 공유하면 소리는 함께 송출됩니다.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new("TIDAL 열기").fill(ACCENT)).clicked()
                    && let Some(t) = &app.tidal
                {
                    t.show();
                    t.navigate("https://listen.tidal.com");
                }
                if ui.button("테스트 톤").clicked() && let Some(t) = &app.tidal {
                    t.show();
                    t.html(TONE_HTML.to_string());
                }
                if ui.button("YouTube").clicked() && let Some(t) = &app.tidal {
                    t.show();
                    t.navigate("https://www.youtube.com");
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("TIDAL 창 보이기").clicked() && let Some(t) = &app.tidal {
                    t.show();
                }
                if ui.button("TIDAL 창 숨기기").clicked() && let Some(t) = &app.tidal {
                    t.hide();
                }
            });
            ui.add_space(6.0);
            ui.add_enabled_ui(app.tidal_audio, |ui| {
                ui.horizontal(|ui| {
                    ui.label("🔊 볼륨");
                    let resp = ui.add(
                        egui::Slider::new(&mut app.tidal_volume, 0.0..=1.0)
                            .custom_formatter(|v, _| format!("{}%", (v * 100.0).round() as i32))
                            .custom_parser(|s| s.trim_end_matches('%').parse::<f64>().ok().map(|v| v / 100.0)),
                    );
                    if resp.changed() {
                        if let Some(a) = &app.audio {
                            a.set_tree_volume(&app.self_tree, app.effective_tidal_volume(), app.self_pid);
                        }
                        app.save_settings();
                    }
                });
            });
            hint(ui, "TIDAL UI 와 무관하게 WebView2 오디오 볼륨을 조절합니다(Discord 로 나가는 소리에도 적용).");
            if app.tidal.is_none() {
                ui.add_space(4.0);
                ui.colored_label(Color32::from_rgb(0xED, 0x42, 0x45), "WebView2 런타임을 찾지 못했습니다. (Edge WebView2 Runtime 설치 필요)");
            }
        });
}

fn center_panel(ui: &mut egui::Ui, app: &mut App) {
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(0x1E, 0x1F, 0x22))
                .inner_margin(12.0),
        )
        .show_inside(ui, |ui| {
            let mut out_fps = 0.0f32;
            let mut wgc_fps = 0.0f32;
            let mut max_gap_ms = 0.0f32;
            let mut preview_frame = None;
            // 캡처 스레드가 채운 최신 프레임을 텍스처로 업로드
            {
                let pv = app.preview.clone();
                if let Ok(p) = pv.lock() {
                    out_fps = p.out_fps;
                    wgc_fps = p.wgc_fps;
                    max_gap_ms = p.max_gap_ms;
                    if p.version != app.preview_ver
                        && p.w > 0
                        && p.h > 0
                        && p.rgba.len() == p.w * p.h * 4
                    {
                        preview_frame = Some((p.w, p.h, p.version, p.rgba.clone()));
                    }
                }
            }
            if let Some((w, h, version, rgba)) = preview_frame {
                let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                app.preview_ver = version;
                match &mut app.preview_tex {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => {
                        app.preview_tex = Some(ui.ctx().load_texture(
                            "preview",
                            img,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                }
            }
            ui.horizontal(|ui| {
                section(ui, "미리보기");
                ui.add_space(8.0);
                let col = if out_fps >= 50.0 {
                    GREEN
                } else if out_fps >= 30.0 {
                    Color32::from_rgb(0xF2, 0xC9, 0x4C)
                } else {
                    Color32::from_rgb(0xED, 0x42, 0x45)
                };
                ui.colored_label(
                    col,
                    RichText::new(format!("출력 {out_fps:.0} FPS")).strong(),
                );
                ui.colored_label(
                    GREY,
                    RichText::new(format!("· WGC 캡처 {wgc_fps:.0} fps")).size(12.0),
                );
                let gap_color = if max_gap_ms <= 20.0 {
                    GREEN
                } else if max_gap_ms <= 28.0 {
                    Color32::from_rgb(0xF2, 0xC9, 0x4C)
                } else {
                    Color32::from_rgb(0xED, 0x42, 0x45)
                };
                ui.colored_label(
                    gap_color,
                    RichText::new(format!("· 최대 간격 {max_gap_ms:.1} ms")).size(12.0),
                );
            });
            egui::Frame::new()
                .stroke(Stroke::new(2.0, ACCENT))
                .fill(Color32::BLACK)
                .inner_margin(2.0)
                .show(ui, |ui| {
                    ui.set_min_height(300.0);
                    if let Some(t) = &app.preview_tex {
                        let sz = t.size();
                        let avail = ui.available_width().min(720.0);
                        let aspect = sz[1] as f32 / sz[0].max(1) as f32;
                        let size = egui::vec2(avail, avail * aspect);
                        let sized = egui::load::SizedTexture::new(t.id(), size);
                        ui.centered_and_justified(|ui| ui.add(egui::Image::new(sized)));
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.colored_label(
                                GREY,
                                "미러링을 시작하면 게임 화면이 여기에 표시됩니다.",
                            );
                        });
                    }
                });

            ui.add_space(10.0);
            section(
                ui,
                "③ 오디오 세션 (▣ = 우리 프로세스 트리 = Discord 가 함께 캡처)",
            );
            hint(
                ui,
                "TIDAL 재생 시 msedgewebview2.exe 가 초록 ▣ 로 떠야 정상입니다.",
            );

            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut rows: Vec<&AudioSession> =
                    app.last_audio.iter().filter(|s| s.pid != 0).collect();
                rows.sort_by(|a, b| {
                    b.peak
                        .partial_cmp(&a.peak)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                for s in rows {
                    let in_tree = app.self_tree.contains(&s.pid);
                    let name = app
                        .proc_names
                        .get(&s.pid)
                        .cloned()
                        .unwrap_or_else(|| "?".into());
                    let mark = if in_tree { "▣" } else { "  " };
                    let color = if in_tree { GREEN } else { GREY };
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            color,
                            RichText::new(format!("{mark} {:<26} pid {:<6}", name, s.pid))
                                .monospace(),
                        );
                        // peak meter
                        let w = 120.0 * s.peak.clamp(0.0, 1.0);
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(120.0, 12.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, PANEL);
                        let mut bar = rect;
                        bar.set_width(w);
                        ui.painter()
                            .rect_filled(bar, 2.0, if s.active { color } else { GREY });
                    });
                }
            });
        });
}

// ---------- 스타일 헬퍼 ----------
fn load_korean_font(ctx: &egui::Context) {
    // egui 기본 폰트엔 한글 글리프가 없어 ㅁㅁㅁ 로 나옴 → 맑은 고딕 로드.
    let candidates = [
        "C:/Windows/Fonts/malgun.ttf", // 맑은 고딕
        "C:/Windows/Fonts/malgunsl.ttf",
        "C:/Windows/Fonts/gulim.ttc",
    ];
    let mut data = None;
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            data = Some(bytes);
            break;
        }
    }
    let Some(bytes) = data else { return };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "korean".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    // 한글을 최우선 폰트로(라틴은 기존 폰트가 이어받아 렌더)
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "korean".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("korean".to_owned());
    ctx.set_fonts(fonts);
}

fn apply_theme(ctx: &egui::Context) {
    load_korean_font(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(0x1E, 0x1F, 0x22);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.selection.bg_fill = ACCENT.linear_multiply(0.5);
    v.hyperlink_color = ACCENT;
    ctx.set_visuals(v);

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    ctx.set_global_style(style);
}

fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x25, 0x27, 0x2B))
        .inner_margin(12.0)
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(2.0);
    ui.label(
        RichText::new(title)
            .size(14.0)
            .strong()
            .color(Color32::from_rgb(0xDB, 0xDE, 0xE1)),
    );
    ui.add_space(4.0);
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(GREY));
}

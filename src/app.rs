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
const MIN_CROP_SIZE: f32 = 0.005;

#[derive(Clone, Copy)]
struct CropResizeEdges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

#[derive(Clone, Copy)]
enum CropDragMode {
    New,
    Move,
    Resize(CropResizeEdges),
}

#[derive(Clone, Copy)]
struct CropDragState {
    mode: CropDragMode,
    pointer_start: [f32; 2],
    crop_start: Option<[f32; 4]>,
}

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
    pub crop_align_x: i32,
    pub crop_align_y: i32,
    pub manual_crop: Option<[f32; 4]>,
    crop_selecting: bool,
    crop_working: Option<[f32; 4]>,
    crop_edit_original: Option<[f32; 4]>,
    crop_drag_state: Option<CropDragState>,
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
        capture_handle.set_aspect_align(s.crop_align_x, s.crop_align_y);
        capture_handle.set_manual_crop(s.manual_crop);
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
            crop_align_x: s.crop_align_x,
            crop_align_y: s.crop_align_y,
            manual_crop: s.manual_crop,
            crop_selecting: false,
            crop_working: None,
            crop_edit_original: None,
            crop_drag_state: None,
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
            crop_align_x: self.crop_align_x,
            crop_align_y: self.crop_align_y,
            manual_crop: self.manual_crop,
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
            egui::ScrollArea::vertical()
                .id_salt("left_panel_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
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
                                let label =
                                    format!("{}\n   {} · pid {}", w.title, w.process, w.pid);
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
                            app.capture
                                .set_aspect_align(app.crop_align_x, app.crop_align_y);
                            app.capture.set_manual_crop(app.manual_crop);
                            app.capture.start(hwnd);
                            app.sync_game_audio_relay();
                            app.mirroring = true;
                        }
                        if ui.button("■ 중지").clicked() {
                            cancel_crop_edit(app);
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
                    ui.horizontal(|ui| {
                        let select_text = if app.crop_selecting {
                            "✓ 선택 완료"
                        } else {
                            "▣ 영역 선택"
                        };
                        let select = ui
                            .add_enabled(
                                app.mirroring && app.preview_tex.is_some(),
                                egui::Button::new(select_text)
                                    .fill(if app.crop_selecting { GREEN } else { ACCENT }),
                            )
                            .on_hover_text(if app.crop_selecting {
                                "현재 영역 편집을 완료하고 저장합니다."
                            } else {
                                "미리보기에서 새 영역을 드래그하거나 기존 영역을 편집합니다."
                            });
                        if select.clicked() {
                            if app.crop_selecting {
                                finish_crop_edit(app);
                            } else {
                                begin_crop_edit(app);
                            }
                        }
                        if (app.manual_crop.is_some() || app.crop_working.is_some())
                            && ui.button("영역 초기화").clicked()
                        {
                            app.manual_crop = None;
                            app.crop_selecting = false;
                            app.crop_working = None;
                            app.crop_edit_original = None;
                            app.crop_drag_state = None;
                            app.capture.set_manual_crop(None);
                            app.save_settings();
                        }
                    });
                    if app.crop_selecting {
                        hint(ui, "영역 안쪽 드래그=이동 · 변/모서리 드래그=크기 조절 · 바깥 드래그=새 선택 · Esc=취소");
                    } else if app.manual_crop.is_some() {
                        hint(ui, "수동 영역 적용 중 · 아래 자동 크롭 설정은 보존되며 일시적으로 무시됩니다.");
                    } else if !app.mirroring {
                        hint(ui, "영역 선택은 미러링을 시작한 뒤 사용할 수 있습니다.");
                    }

                    let automatic_crop_enabled = app
                        .crop_working
                        .or(app.manual_crop)
                        .is_none();
                    ui.add_enabled_ui(automatic_crop_enabled, |ui| {
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
                    });
                    ui.add_enabled_ui(automatic_crop_enabled && app.crop_16_9, |ui| {
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
                            changed |= crop_alignment_pad(
                                ui,
                                &mut app.crop_align_x,
                                &mut app.crop_align_y,
                            );
                        });
                        if changed {
                            app.crop_aspect_w = app.crop_aspect_w.clamp(1.0, 64.0);
                            app.crop_aspect_h = app.crop_aspect_h.clamp(1.0, 64.0);
                            app.crop_align_x = app.crop_align_x.clamp(0, 2);
                            app.crop_align_y = app.crop_align_y.clamp(0, 2);
                            app.capture.set_aspect(app.crop_aspect_w, app.crop_aspect_h);
                            app.capture
                                .set_aspect_align(app.crop_align_x, app.crop_align_y);
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
                                .add(
                                    egui::Slider::new(&mut app.hdr_nits, 80.0..=480.0)
                                        .suffix(" nit"),
                                )
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
        });
}

fn right_panel(ui: &mut egui::Ui, app: &mut App) {
    egui::Panel::right("right")
        .exact_size(380.0)
        .frame(panel_frame())
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("right_panel_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
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
                if ui.button("장치 새로고침").clicked() {
                    app.refresh_relay_devices();
                }
                hint(
                    ui,
                    "2중으로 들리면 VB-CABLE 같은 안 듣는 출력장치를 선택하세요.",
                );
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
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "WebView2 런타임을 찾지 못했습니다. (Edge WebView2 Runtime 설치 필요)",
                        )
                        .color(Color32::from_rgb(0xED, 0x42, 0x45)),
                    )
                    .wrap(),
                );
            }
                });
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
            if app.crop_selecting && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                cancel_crop_edit(app);
            }
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
            // 창 높이가 낮을 때 미리보기가 아래의 오디오 목록을 밀어내지 않도록
            // 현재 남은 높이에서 제목/목록 영역을 먼저 확보한다.
            let preview_height = (ui.available_height() - 190.0).clamp(180.0, 540.0);
            egui::Frame::new()
                .stroke(Stroke::new(2.0, ACCENT))
                .fill(Color32::BLACK)
                .inner_margin(2.0)
                .show(ui, |ui| {
                    ui.set_height(preview_height);
                    if let Some(t) = app.preview_tex.clone() {
                        let sz = t.size();
                        let source = egui::vec2(sz[0].max(1) as f32, sz[1].max(1) as f32);
                        let max_size = egui::vec2(ui.available_width().min(720.0), preview_height);
                        let scale = (max_size.x / source.x).min(max_size.y / source.y).min(1.0);
                        let size = source * scale;
                        let canvas_size = egui::vec2(ui.available_width(), preview_height);
                        let (canvas_rect, _) =
                            ui.allocate_exact_size(canvas_size, egui::Sense::hover());
                        let image_rect = egui::Rect::from_center_size(canvas_rect.center(), size);
                        ui.painter().image(
                            t.id(),
                            image_rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        // 검은 캔버스가 아니라 실제 영상 픽셀 사각형만 입력을 받는다.
                        let response = ui.interact(
                            image_rect,
                            ui.id().with("preview_crop_image"),
                            egui::Sense::click_and_drag(),
                        );
                        handle_crop_selection(ui, app, response);
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
fn begin_crop_edit(app: &mut App) {
    app.crop_selecting = true;
    app.crop_edit_original = app.manual_crop;
    app.crop_working = app.manual_crop;
    app.crop_drag_state = None;
}

fn finish_crop_edit(app: &mut App) {
    app.manual_crop = app.crop_working;
    app.capture.set_manual_crop(app.manual_crop);
    app.save_settings();
    app.crop_selecting = false;
    app.crop_working = None;
    app.crop_edit_original = None;
    app.crop_drag_state = None;
}

fn cancel_crop_edit(app: &mut App) {
    if !app.crop_selecting {
        return;
    }
    app.capture.set_manual_crop(app.crop_edit_original);
    app.crop_selecting = false;
    app.crop_working = None;
    app.crop_edit_original = None;
    app.crop_drag_state = None;
}

fn handle_crop_selection(ui: &mut egui::Ui, app: &mut App, response: egui::Response) {
    let image_rect = response.rect;
    let response = if app.crop_selecting {
        let cursor = response
            .hover_pos()
            .map(|pointer| {
                crop_edit_cursor(image_rect, app.crop_working.or(app.manual_crop), pointer)
            })
            .unwrap_or(egui::CursorIcon::Crosshair);
        response.on_hover_cursor(cursor)
    } else {
        response
    };

    if app.crop_selecting {
        if response.drag_started()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let point = normalized_preview_point(image_rect, pointer);
            let crop = app.crop_working.or(app.manual_crop);
            app.crop_drag_state = Some(CropDragState {
                mode: crop_drag_mode(image_rect, crop, pointer),
                pointer_start: point,
                crop_start: crop,
            });
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(drag) = app.crop_drag_state
        {
            let point = normalized_preview_point(image_rect, pointer);
            if let Some(crop) = update_crop_drag(drag, point) {
                app.crop_working = Some(crop);
                app.capture.set_manual_crop(Some(crop));
            }
        }
        if response.drag_stopped() {
            if let Some(pointer) = response.interact_pointer_pos()
                && let Some(drag) = app.crop_drag_state
            {
                let point = normalized_preview_point(image_rect, pointer);
                if let Some(crop) = update_crop_drag(drag, point) {
                    app.crop_working = Some(crop);
                    app.capture.set_manual_crop(Some(crop));
                }
            }
            app.crop_drag_state = None;
        }
    }

    if let Some(crop) = app.crop_working.or(app.manual_crop) {
        paint_crop_overlay(ui, image_rect, crop, app.crop_selecting);
    }
}

fn crop_drag_mode(image: egui::Rect, crop: Option<[f32; 4]>, pointer: egui::Pos2) -> CropDragMode {
    let Some(crop) = crop else {
        return CropDragMode::New;
    };
    let selected = crop_rect_on_image(image, crop);
    let handle = 10.0;
    if !selected.expand(handle).contains(pointer) {
        return CropDragMode::New;
    }

    let mut left = (pointer.x - selected.left()).abs() <= handle;
    let mut right = (pointer.x - selected.right()).abs() <= handle;
    let mut top = (pointer.y - selected.top()).abs() <= handle;
    let mut bottom = (pointer.y - selected.bottom()).abs() <= handle;
    if left && right {
        if (pointer.x - selected.left()).abs() <= (pointer.x - selected.right()).abs() {
            right = false;
        } else {
            left = false;
        }
    }
    if top && bottom {
        if (pointer.y - selected.top()).abs() <= (pointer.y - selected.bottom()).abs() {
            bottom = false;
        } else {
            top = false;
        }
    }
    if left || right || top || bottom {
        CropDragMode::Resize(CropResizeEdges {
            left,
            right,
            top,
            bottom,
        })
    } else if selected.contains(pointer) {
        CropDragMode::Move
    } else {
        CropDragMode::New
    }
}

fn crop_edit_cursor(
    image: egui::Rect,
    crop: Option<[f32; 4]>,
    pointer: egui::Pos2,
) -> egui::CursorIcon {
    match crop_drag_mode(image, crop, pointer) {
        CropDragMode::New => egui::CursorIcon::Crosshair,
        CropDragMode::Move => egui::CursorIcon::Grab,
        CropDragMode::Resize(edges) => match (edges.left, edges.right, edges.top, edges.bottom) {
            (true, false, true, false) | (false, true, false, true) => egui::CursorIcon::ResizeNwSe,
            (false, true, true, false) | (true, false, false, true) => egui::CursorIcon::ResizeNeSw,
            (true, false, false, false) | (false, true, false, false) => {
                egui::CursorIcon::ResizeHorizontal
            }
            _ => egui::CursorIcon::ResizeVertical,
        },
    }
}

fn update_crop_drag(drag: CropDragState, pointer: [f32; 2]) -> Option<[f32; 4]> {
    match drag.mode {
        CropDragMode::New => normalized_crop_from_points(drag.pointer_start, pointer),
        CropDragMode::Move => {
            let [x, y, w, h] = drag.crop_start?;
            let dx = pointer[0] - drag.pointer_start[0];
            let dy = pointer[1] - drag.pointer_start[1];
            Some([
                (x + dx).clamp(0.0, 1.0 - w),
                (y + dy).clamp(0.0, 1.0 - h),
                w,
                h,
            ])
        }
        CropDragMode::Resize(edges) => {
            let [x, y, w, h] = drag.crop_start?;
            let dx = pointer[0] - drag.pointer_start[0];
            let dy = pointer[1] - drag.pointer_start[1];
            let mut left = x;
            let mut right = x + w;
            let mut top = y;
            let mut bottom = y + h;
            if edges.left {
                left = (x + dx).clamp(0.0, right - MIN_CROP_SIZE);
            }
            if edges.right {
                right = (x + w + dx).clamp(left + MIN_CROP_SIZE, 1.0);
            }
            if edges.top {
                top = (y + dy).clamp(0.0, bottom - MIN_CROP_SIZE);
            }
            if edges.bottom {
                bottom = (y + h + dy).clamp(top + MIN_CROP_SIZE, 1.0);
            }
            Some([left, top, right - left, bottom - top])
        }
    }
}

fn normalized_preview_point(rect: egui::Rect, point: egui::Pos2) -> [f32; 2] {
    [
        ((point.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0),
        ((point.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0),
    ]
}

fn normalized_crop_from_points(start: [f32; 2], end: [f32; 2]) -> Option<[f32; 4]> {
    let left = start[0].min(end[0]).clamp(0.0, 1.0);
    let top = start[1].min(end[1]).clamp(0.0, 1.0);
    let right = start[0].max(end[0]).clamp(0.0, 1.0);
    let bottom = start[1].max(end[1]).clamp(0.0, 1.0);
    let width = right - left;
    let height = bottom - top;
    (width >= MIN_CROP_SIZE && height >= MIN_CROP_SIZE).then_some([left, top, width, height])
}

fn paint_crop_overlay(ui: &egui::Ui, image: egui::Rect, crop: [f32; 4], selecting: bool) {
    let selected = crop_rect_on_image(image, crop);
    let shade = Color32::from_black_alpha(70);
    let painter = ui.painter();
    painter.rect_filled(
        egui::Rect::from_min_max(image.min, egui::pos2(image.right(), selected.top())),
        0.0,
        shade,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(image.left(), selected.bottom()), image.max),
        0.0,
        shade,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(image.left(), selected.top()),
            egui::pos2(selected.left(), selected.bottom()),
        ),
        0.0,
        shade,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(selected.right(), selected.top()),
            egui::pos2(image.right(), selected.bottom()),
        ),
        0.0,
        shade,
    );
    painter.rect_stroke(
        selected,
        0.0,
        Stroke::new(2.0, if selecting { GREEN } else { ACCENT }),
        egui::StrokeKind::Inside,
    );
    if selecting {
        let handle_size = egui::vec2(8.0, 8.0);
        let points = [
            selected.left_top(),
            egui::pos2(selected.center().x, selected.top()),
            selected.right_top(),
            egui::pos2(selected.left(), selected.center().y),
            egui::pos2(selected.right(), selected.center().y),
            selected.left_bottom(),
            egui::pos2(selected.center().x, selected.bottom()),
            selected.right_bottom(),
        ];
        for point in points {
            let handle = egui::Rect::from_center_size(point, handle_size);
            painter.rect_filled(handle, 1.0, Color32::WHITE);
            painter.rect_stroke(
                handle,
                1.0,
                Stroke::new(1.0, GREEN),
                egui::StrokeKind::Inside,
            );
        }
    }
}

fn crop_rect_on_image(image: egui::Rect, crop: [f32; 4]) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(
            image.left() + crop[0] * image.width(),
            image.top() + crop[1] * image.height(),
        ),
        egui::pos2(
            image.left() + (crop[0] + crop[2]) * image.width(),
            image.top() + (crop[1] + crop[3]) * image.height(),
        ),
    )
}

fn crop_alignment_pad(ui: &mut egui::Ui, align_x: &mut i32, align_y: &mut i32) -> bool {
    const DIRECTIONS: [[(&str, &str, i32, i32); 3]; 3] = [
        [
            ("↖", "왼쪽 위", 0, 0),
            ("↑", "위", 1, 0),
            ("↗", "오른쪽 위", 2, 0),
        ],
        [("←", "왼쪽", 0, 1), ("", "", 1, 1), ("→", "오른쪽", 2, 1)],
        [
            ("↙", "왼쪽 아래", 0, 2),
            ("↓", "아래", 1, 2),
            ("↘", "오른쪽 아래", 2, 2),
        ],
    ];

    let mut changed = false;
    egui::Grid::new("crop_alignment_pad")
        .spacing(egui::vec2(3.0, 3.0))
        .show(ui, |ui| {
            for (row_index, row) in DIRECTIONS.iter().enumerate() {
                for &(icon, label, x, y) in row {
                    if icon.is_empty() {
                        let color = if *align_x == 1 && *align_y == 1 {
                            ACCENT
                        } else {
                            GREY
                        };
                        ui.add_sized(
                            [30.0, 30.0],
                            egui::Label::new(RichText::new("•").size(20.0).color(color)),
                        )
                        .on_hover_text("중앙 기준점");
                    } else {
                        let selected = *align_x == x && *align_y == y;
                        let response = ui
                            .add_sized(
                                [30.0, 30.0],
                                egui::Button::new(RichText::new(icon).size(17.0))
                                    .selected(selected),
                            )
                            .on_hover_text(label);
                        if response.clicked() {
                            *align_x = x;
                            *align_y = y;
                            changed = true;
                        }
                    }
                }
                if row_index < DIRECTIONS.len() - 1 {
                    ui.end_row();
                }
            }
        });
    changed
}

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
    ui.add(egui::Label::new(RichText::new(text).size(11.0).color(GREY)).wrap());
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    use super::{
        CropDragMode, CropDragState, CropResizeEdges, normalized_crop_from_points,
        normalized_preview_point, update_crop_drag,
    };

    #[test]
    fn crop_selection_supports_dragging_in_any_direction() {
        assert_eq!(
            normalized_crop_from_points([0.8, 0.7], [0.2, 0.1]),
            Some([0.2, 0.1, 0.6, 0.59999996])
        );
    }

    #[test]
    fn crop_selection_rejects_accidental_tiny_drags() {
        assert_eq!(normalized_crop_from_points([0.5, 0.5], [0.501, 0.9]), None);
    }

    #[test]
    fn preview_resize_does_not_change_normalized_pointer_position() {
        let small = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        let large = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(400.0, 200.0));
        assert_eq!(
            normalized_preview_point(small, egui::pos2(25.0, 40.0)),
            normalized_preview_point(large, egui::pos2(200.0, 130.0))
        );
    }

    #[test]
    fn moving_crop_is_clamped_inside_source_bounds() {
        let moved = update_crop_drag(
            CropDragState {
                mode: CropDragMode::Move,
                pointer_start: [0.75, 0.75],
                crop_start: Some([0.7, 0.7, 0.2, 0.2]),
            },
            [1.0, 1.0],
        );
        assert_eq!(moved, Some([0.8, 0.8, 0.2, 0.2]));
    }

    #[test]
    fn resizing_crop_edges_is_clamped_inside_source_bounds() {
        let resized = update_crop_drag(
            CropDragState {
                mode: CropDragMode::Resize(CropResizeEdges {
                    left: true,
                    right: false,
                    top: true,
                    bottom: false,
                }),
                pointer_start: [0.2, 0.2],
                crop_start: Some([0.2, 0.2, 0.6, 0.6]),
            },
            [0.0, 0.0],
        );
        assert_eq!(resized, Some([0.0, 0.0, 0.8, 0.8]));
    }
}

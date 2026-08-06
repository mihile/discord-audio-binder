//! 앱 재시작 후에도 유지되는 설정(HDR/크롭/볼륨). 미러링 대상은 저장하지 않는다.

use std::path::PathBuf;

pub struct Settings {
    pub hdr_fix: bool,
    pub hdr_nits: f32,
    pub crop_titlebar: bool,
    pub crop_16_9: bool,
    pub crop_aspect_w: f32,
    pub crop_aspect_h: f32,
    pub crop_align_x: i32,
    pub crop_align_y: i32,
    pub manual_crop: Option<[f32; 4]>,
    pub vsync: bool,
    pub output_fps: f32,
    pub volume: f32,
    pub tidal_audio: bool,
    pub game_audio: bool,
    pub relay_device_id: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hdr_fix: false,
            hdr_nits: 200.0,
            crop_titlebar: true,
            crop_16_9: true,
            crop_aspect_w: 16.0,
            crop_aspect_h: 9.0,
            crop_align_x: 1,
            crop_align_y: 1,
            manual_crop: None,
            vsync: true,
            output_fps: 60.0,
            volume: 1.0,
            tidal_audio: true,
            game_audio: false,
            relay_device_id: None,
        }
    }
}

impl Settings {
    fn path() -> Option<PathBuf> {
        let base = std::env::var("APPDATA").ok()?;
        let mut p = PathBuf::from(base);
        p.push("discord-audio-binder");
        std::fs::create_dir_all(&p).ok()?;
        p.push("settings.txt");
        Some(p)
    }

    pub fn load() -> Self {
        let mut s = Self::default();
        if let Some(p) = Self::path()
            && let Ok(txt) = std::fs::read_to_string(p)
        {
            for line in txt.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    let v = v.trim();
                    match k.trim() {
                        "hdr_fix" => s.hdr_fix = v == "true",
                        "hdr_nits" => {
                            if let Ok(n) = v.parse() {
                                s.hdr_nits = n;
                            }
                        }
                        "crop" => s.crop_titlebar = v == "true",
                        "crop_16_9" => s.crop_16_9 = v == "true",
                        "crop_aspect_w" => {
                            if let Ok(n) = v.parse::<f32>() {
                                s.crop_aspect_w = n.clamp(1.0, 64.0);
                            }
                        }
                        "crop_aspect_h" => {
                            if let Ok(n) = v.parse::<f32>() {
                                s.crop_aspect_h = n.clamp(1.0, 64.0);
                            }
                        }
                        "crop_align" => {
                            // 0.1.1 이하 설정 호환: 기존 값은 가로 정렬만 의미했다.
                            if let Ok(n) = v.parse::<i32>() {
                                s.crop_align_x = n.clamp(0, 2);
                            }
                        }
                        "crop_align_x" => {
                            if let Ok(n) = v.parse::<i32>() {
                                s.crop_align_x = n.clamp(0, 2);
                            }
                        }
                        "crop_align_y" => {
                            if let Ok(n) = v.parse::<i32>() {
                                s.crop_align_y = n.clamp(0, 2);
                            }
                        }
                        "manual_crop" => {
                            s.manual_crop = parse_manual_crop(v);
                        }
                        "vsync" => s.vsync = v == "true",
                        "output_fps" => {
                            if let Ok(n) = v.parse::<f32>() {
                                s.output_fps = n.clamp(30.0, 180.0);
                            }
                        }
                        "volume" => {
                            if let Ok(n) = v.parse() {
                                s.volume = n;
                            }
                        }
                        "tidal_audio" => s.tidal_audio = v == "true",
                        "game_audio" => s.game_audio = v == "true",
                        "relay_device" => {
                            s.relay_device_id = if v.is_empty() { None } else { Some(v.into()) };
                        }
                        _ => {}
                    }
                }
            }
        }
        s
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            let manual_crop = format_manual_crop(self.manual_crop);
            let txt = format!(
                "hdr_fix={}\nhdr_nits={}\ncrop={}\ncrop_16_9={}\ncrop_aspect_w={}\ncrop_aspect_h={}\ncrop_align_x={}\ncrop_align_y={}\nmanual_crop={}\nvsync={}\noutput_fps={}\nvolume={}\ntidal_audio={}\ngame_audio={}\nrelay_device={}\n",
                self.hdr_fix,
                self.hdr_nits,
                self.crop_titlebar,
                self.crop_16_9,
                self.crop_aspect_w,
                self.crop_aspect_h,
                self.crop_align_x,
                self.crop_align_y,
                manual_crop,
                self.vsync,
                self.output_fps,
                self.volume,
                self.tidal_audio,
                self.game_audio,
                self.relay_device_id.as_deref().unwrap_or("")
            );
            let _ = std::fs::write(p, txt);
        }
    }
}

fn sanitize_manual_crop([x, y, w, h]: [f32; 4]) -> Option<[f32; 4]> {
    if ![x, y, w, h].iter().all(|value| value.is_finite()) || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let w = w.clamp(0.0, 1.0 - x);
    let h = h.clamp(0.0, 1.0 - y);
    (w > 0.0 && h > 0.0).then_some([x, y, w, h])
}

fn parse_manual_crop(value: &str) -> Option<[f32; 4]> {
    let mut parts = value.split(',');
    let crop = [
        parts.next()?.trim().parse().ok()?,
        parts.next()?.trim().parse().ok()?,
        parts.next()?.trim().parse().ok()?,
        parts.next()?.trim().parse().ok()?,
    ];
    if parts.next().is_some() {
        return None;
    }
    sanitize_manual_crop(crop)
}

fn format_manual_crop(crop: Option<[f32; 4]>) -> String {
    crop.map(|[x, y, w, h]| format!("{x:.6},{y:.6},{w:.6},{h:.6}"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{format_manual_crop, parse_manual_crop, sanitize_manual_crop};

    #[test]
    fn manual_crop_setting_parses_and_clamps() {
        assert_eq!(
            parse_manual_crop("0.25,0.1,0.5,0.8"),
            Some([0.25, 0.1, 0.5, 0.8])
        );
        assert_eq!(
            sanitize_manual_crop([0.9, 0.9, 0.5, 0.5]),
            Some([0.9, 0.9, 0.100000024, 0.100000024])
        );
    }

    #[test]
    fn malformed_manual_crop_setting_is_ignored() {
        assert_eq!(parse_manual_crop(""), None);
        assert_eq!(parse_manual_crop("0.1,bad,0.5,0.5"), None);
        assert_eq!(parse_manual_crop("0.1,0.1,0.5,0.5,0.2"), None);
    }

    #[test]
    fn manual_crop_setting_round_trips_through_saved_text() {
        let crop = Some([0.123456, 0.234567, 0.5, 0.6]);
        assert_eq!(parse_manual_crop(&format_manual_crop(crop)), crop);
        assert_eq!(format_manual_crop(None), "");
    }
}

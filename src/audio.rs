//! 기본 렌더 디바이스의 오디오 세션 스냅샷 (pid / peak / state).

use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::core::Interface;

pub struct AudioSession {
    pub pid: u32,
    pub peak: f32,
    pub active: bool,
}

pub struct AudioMonitor {
    enumerator: IMMDeviceEnumerator,
}

impl AudioMonitor {
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            Ok(Self { enumerator })
        }
    }

    /// 스냅샷 + 트리 볼륨 적용을 한 번의 세션 열거로 처리 (틱마다 2회 열거하던 것 → 1회).
    /// skip_pid: 볼륨을 적용하지 않을 pid (우리 앱 자신 = 게임 relay 재생). TIDAL 볼륨이
    /// relay 출력까지 줄이지 않도록 분리한다.
    pub fn snapshot_with_volume(
        &self,
        tree: &std::collections::HashSet<u32>,
        vol: f32,
        skip_pid: u32,
    ) -> Vec<AudioSession> {
        let mut out = Vec::new();
        unsafe {
            let Ok(device) = self
                .enumerator
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
            else {
                return out;
            };
            let mgr: IAudioSessionManager2 = match device.Activate(CLSCTX_ALL, None) {
                Ok(m) => m,
                Err(_) => return out,
            };
            let Ok(sessions) = mgr.GetSessionEnumerator() else {
                return out;
            };
            let count = sessions.GetCount().unwrap_or(0);
            let vol = vol.clamp(0.0, 1.0);
            for i in 0..count {
                let Ok(ctrl) = sessions.GetSession(i) else {
                    continue;
                };
                let pid = ctrl
                    .cast::<IAudioSessionControl2>()
                    .ok()
                    .and_then(|c| c.GetProcessId().ok())
                    .unwrap_or(0);
                let active = ctrl
                    .GetState()
                    .map(|s| s == AudioSessionStateActive)
                    .unwrap_or(false);
                let peak = ctrl
                    .cast::<IAudioMeterInformation>()
                    .ok()
                    .and_then(|m| m.GetPeakValue().ok())
                    .unwrap_or(0.0);
                if pid != 0
                    && pid != skip_pid
                    && tree.contains(&pid)
                    && let Ok(sav) = ctrl.cast::<ISimpleAudioVolume>()
                {
                    let _ = sav.SetMasterVolume(vol, std::ptr::null());
                }
                out.push(AudioSession { pid, peak, active });
            }
        }
        out
    }

    /// 주어진 프로세스 트리(우리 앱+자식)의 모든 오디오 세션 마스터 볼륨을 vol(0~1)로 설정.
    /// → WebView2(TIDAL) 오디오 볼륨을 TIDAL UI 와 무관하게 조절. (슬라이더 즉시 반영용)
    pub fn set_tree_volume(&self, tree: &std::collections::HashSet<u32>, vol: f32, skip_pid: u32) {
        unsafe {
            let Ok(device) = self
                .enumerator
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
            else {
                return;
            };
            let mgr: IAudioSessionManager2 = match device.Activate(CLSCTX_ALL, None) {
                Ok(m) => m,
                Err(_) => return,
            };
            let Ok(sessions) = mgr.GetSessionEnumerator() else {
                return;
            };
            let count = sessions.GetCount().unwrap_or(0);
            for i in 0..count {
                let Ok(ctrl) = sessions.GetSession(i) else {
                    continue;
                };
                let pid = ctrl
                    .cast::<IAudioSessionControl2>()
                    .ok()
                    .and_then(|c| c.GetProcessId().ok())
                    .unwrap_or(0);
                if pid != 0
                    && pid != skip_pid
                    && tree.contains(&pid)
                    && let Ok(sav) = ctrl.cast::<ISimpleAudioVolume>()
                {
                    let _ = sav.SetMasterVolume(vol.clamp(0.0, 1.0), std::ptr::null());
                }
            }
        }
    }
}

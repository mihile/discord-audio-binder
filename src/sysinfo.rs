//! 창 열거 + 프로세스 스냅샷(이름/부모) + 프로세스 트리 계산.

use std::collections::{HashMap, HashSet};
use windows::Win32::Foundation::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::BOOL;

#[derive(Clone)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub pid: u32,
    pub title: String,
    pub process: String,
}

pub struct ProcSnapshot {
    pub names: HashMap<u32, String>,
    pub parent: HashMap<u32, u32>,
}

/// 보이는 최상위 창 목록 (Discord 애플리케이션 공유 후보와 같은 모집단).
pub fn enumerate_windows(proc_names: &HashMap<u32, String>) -> Vec<WindowInfo> {
    let mut list: Vec<WindowInfo> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut list as *mut _ as isize));
    }
    for w in &mut list {
        if let Some(n) = proc_names.get(&w.pid) {
            w.process = n.clone();
        }
    }
    list
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let list = &mut *(lparam.0 as *mut Vec<WindowInfo>);

        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return BOOL(1);
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let got = GetWindowTextW(hwnd, &mut buf);
        if got == 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&buf[..got as usize]);
        if title.trim().is_empty() {
            return BOOL(1);
        }
        // 도구 창 제외
        let exstyle = GetWindowLongW(hwnd, GWL_EXSTYLE);
        if (exstyle & WS_EX_TOOLWINDOW.0 as i32) != 0 {
            return BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        list.push(WindowInfo {
            hwnd: hwnd.0 as isize,
            pid,
            title,
            process: String::new(),
        });
        BOOL(1)
    }
}

/// 전체 프로세스 스냅샷: pid→이름, pid→부모pid.
pub fn snapshot_processes() -> ProcSnapshot {
    let mut names = HashMap::new();
    let mut parent = HashMap::new();
    unsafe {
        if let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap, &mut entry).is_ok() {
                loop {
                    let pid = entry.th32ProcessID;
                    let ppid = entry.th32ParentProcessID;
                    names.insert(pid, exe_name(&entry.szExeFile));
                    parent.insert(pid, ppid);
                    if Process32NextW(snap, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
    }
    ProcSnapshot { names, parent }
}

fn exe_name(raw: &[u16]) -> String {
    let s = String::from_utf16_lossy(raw);
    s.split('\0').next().unwrap_or("").to_string()
}

/// 루트 pid 의 자손 + 자신 집합 (Discord 프로세스-트리 캡처 후보).
pub fn descendants_and_self(parent: &HashMap<u32, u32>, root: u32) -> HashSet<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, &ppid) in parent {
        children.entry(ppid).or_default().push(pid);
    }
    let mut result = HashSet::new();
    result.insert(root);
    let mut stack = vec![root];
    while let Some(cur) = stack.pop() {
        if let Some(kids) = children.get(&cur) {
            for &k in kids {
                if result.insert(k) {
                    stack.push(k);
                }
            }
        }
    }
    result
}

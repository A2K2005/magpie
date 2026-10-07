//! The few things that differ per OS: background priority, user idle time, battery, drives.

use std::path::PathBuf;

/// Lowers the calling thread to background priority: CPU, disk I/O and memory priority on Windows,
/// the utility QoS class (efficiency cores, throttled I/O) on macOS.
pub fn background_thread() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

/// Seconds since the last keyboard or mouse input anywhere on the system.
pub fn idle_seconds() -> u64 {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::SystemInformation::GetTickCount;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut info).as_bool() {
            return u64::from(GetTickCount().wrapping_sub(info.dwTime)) / 1000;
        }
        0
    }
    #[cfg(target_os = "macos")]
    unsafe {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
        }
        // Combined session state, any input event.
        CGEventSourceSecondsSinceLastEventType(0, u32::MAX) as u64
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    0
}

/// True when running on battery.
pub fn on_battery() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut s = SYSTEM_POWER_STATUS::default();
        return GetSystemPowerStatus(&mut s).is_ok() && s.ACLineStatus == 0;
    }
    #[cfg(target_os = "macos")]
    {
        // `pmset -g batt` prints "Now drawing from 'Battery Power'" on battery.
        return std::process::Command::new("pmset")
            .args(["-g", "batt"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("Battery Power"));
    }
    #[allow(unreachable_code)]
    false
}

/// Where "everywhere" looks: every fixed drive on Windows (no removable, network or optical drives),
/// the home folder on macOS.
pub fn everywhere_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
        use windows::core::PCWSTR;
        const DRIVE_FIXED: u32 = 3;
        let mask = GetLogicalDrives();
        return (0..26u8)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| format!("{}:\\", (b'A' + i) as char))
            .filter(|root| {
                let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
                GetDriveTypeW(PCWSTR(wide.as_ptr())) == DRIVE_FIXED
            })
            .map(PathBuf::from)
            .collect();
    }
    #[allow(unreachable_code)]
    std::env::var_os("HOME").map(PathBuf::from).into_iter().collect()
}

/// A number that changes whenever anything is copied to the clipboard.
pub fn clipboard_seq() -> u64 {
    #[cfg(windows)]
    unsafe {
        return u64::from(windows::Win32::System::DataExchange::GetClipboardSequenceNumber());
    }
    #[cfg(target_os = "macos")]
    #[allow(unused_unsafe)]
    unsafe {
        return objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() as u64;
    }
    #[allow(unreachable_code)]
    0
}

/// Peak private memory of this process so far, in MB (Windows; 0 elsewhere). For benchmarks.
pub fn peak_memory_mb() -> u64 {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut c = PROCESS_MEMORY_COUNTERS::default();
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32).is_ok() {
            return c.PeakPagefileUsage as u64 / (1 << 20);
        }
    }
    #[allow(unreachable_code)]
    0
}

/// True when the window in front belongs to a screenshot tool (Snipping Tool, ShareX and others).
pub fn capture_tool_in_front() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
        const TOOLS: [&str; 9] = [
            "screenclippinghost.exe",
            "snippingtool.exe",
            "screensketch.exe",
            "sharex.exe",
            "greenshot.exe",
            "lightshot.exe",
            "flameshot.exe",
            "snagit32.exe",
            "snagiteditor.exe",
        ];
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return false;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
        let exe = path.rsplit('\\').next().unwrap_or_default();
        return TOOLS.contains(&exe);
    }
    #[allow(unreachable_code)]
    false
}

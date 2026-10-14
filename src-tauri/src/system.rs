//! What the tray and flyout need from the OS: the taskbar and app themes, the
//! accent colour, the tray icon size, a font for the battery number, and
//! Windows 11 rounded corners.

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RegGetValueW};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn read(key: &str, name: &str, flags: u32, buffer: &mut [u8]) -> Option<usize> {
        let mut size = buffer.len() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide(key).as_ptr(),
                wide(name).as_ptr(),
                flags,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        (status == 0).then_some(size as usize)
    }

    fn dword(key: &str, name: &str) -> Option<u32> {
        let mut buffer = [0u8; 4];
        read(key, name, RRF_RT_REG_DWORD, &mut buffer).map(|_| u32::from_le_bytes(buffer))
    }

    const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

    pub fn taskbar_light() -> bool {
        dword(PERSONALIZE, "SystemUsesLightTheme") == Some(1)
    }

    pub fn apps_light() -> bool {
        dword(PERSONALIZE, "AppsUseLightTheme") != Some(0)
    }

    /// The accent shade Windows 11 uses for filled controls: a light shade on
    /// dark backgrounds, a dark one on light.
    pub fn accent(light: bool) -> Option<String> {
        let mut palette = [0u8; 32];
        let size = read(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent",
            "AccentPalette",
            RRF_RT_REG_BINARY,
            &mut palette,
        )?;
        let at = if light { 4 } else { 1 } * 4;
        (size >= at + 3).then(|| format!("#{:02x}{:02x}{:02x}", palette[at], palette[at + 1], palette[at + 2]))
    }

    pub fn icon_size() -> u32 {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
        u32::try_from(unsafe { GetSystemMetrics(SM_CXSMICON) }).ok().filter(|size| *size >= 16).unwrap_or(16)
    }

    pub fn font_paths() -> Vec<std::path::PathBuf> {
        let fonts =
            std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into())).join("Fonts");
        ["seguisb.ttf", "segoeuib.ttf", "segoeui.ttf"].iter().map(|name| fonts.join(name)).collect()
    }

    pub fn round_corners(window: &tauri::WebviewWindow) {
        use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
        let Ok(hwnd) = window.hwnd() else { return };
        let preference = DWMWCP_ROUND;
        unsafe {
            DwmSetWindowAttribute(
                hwnd.0 as _,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&raw const preference).cast(),
                size_of_val(&preference) as u32,
            );
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn taskbar_light() -> bool {
        false
    }

    pub fn apps_light() -> bool {
        false
    }

    pub fn accent(_light: bool) -> Option<String> {
        None
    }

    pub fn icon_size() -> u32 {
        if cfg!(target_os = "macos") { 36 } else { 22 }
    }

    pub fn font_paths() -> Vec<std::path::PathBuf> {
        [
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
        ]
        .iter()
        .map(std::path::PathBuf::from)
        .collect()
    }

    pub fn round_corners(_window: &tauri::WebviewWindow) {}
}

pub use imp::*;

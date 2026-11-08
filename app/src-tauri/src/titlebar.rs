//! Brand-coloured native title bar (Design §4.4, §14).
//!
//! The title bar stays native (minimise/maximise/close, snap layouts, Windows
//! accessibility); only its colours change, through DWM window attributes:
//! caption `aqua-ink`, white title, light caption buttons. The webview keeps the
//! light theme. Windows 10 has no caption colour and shows its dark caption,
//! which still stands apart from the light app. High contrast keeps the
//! system's own colours.

/// `aqua-ink` (`tokens.css`) as a Windows COLORREF (0x00BBGGRR).
pub const CAPTION_COLOR: u32 = colorref(0x0E, 0x3B, 0x35);
/// White title text on `aqua-ink` (12:1).
pub const CAPTION_TEXT_COLOR: u32 = colorref(0xFF, 0xFF, 0xFF);

const fn colorref(r: u8, g: u8, b: u8) -> u32 {
    (b as u32) << 16 | (g as u32) << 8 | r as u32
}

/// Colour the main window's title bar. Best effort: failures leave the system look.
#[cfg(windows)]
#[allow(unsafe_code)] // DWM and user32 are C APIs; each call only reads or fills a local value.
pub fn apply<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWINDOWATTRIBUTE,
    };

    if high_contrast() {
        return;
    }
    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
    let set = |attr: DWMWINDOWATTRIBUTE, value: u32| {
        // SAFETY: `hwnd` is this process's live window; `value` outlives the call
        // and its size is passed.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attr as u32,
                (&value as *const u32).cast(),
                size_of::<u32>() as u32,
            )
        };
    };
    // Dark non-client mode only: light glyphs on the minimise/maximise/close buttons.
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    set(DWMWA_CAPTION_COLOR, CAPTION_COLOR);
    set(DWMWA_BORDER_COLOR, CAPTION_COLOR);
    set(DWMWA_TEXT_COLOR, CAPTION_TEXT_COLOR);
}

#[cfg(not(windows))]
pub fn apply<R: tauri::Runtime>(_window: &tauri::WebviewWindow<R>) {}

/// A Windows high-contrast theme is on (Design §11: respect contrast themes).
#[cfg(windows)]
#[allow(unsafe_code)]
fn high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST};

    let mut hc = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        dwFlags: 0,
        lpszDefaultScheme: std::ptr::null_mut(),
    };
    // SAFETY: `hc` is a correctly sized HIGHCONTRASTW that lives across the call.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            (&mut hc as *mut HIGHCONTRASTW).cast(),
            0,
        )
    };
    ok != 0 && hc.dwFlags & HCF_HIGHCONTRASTON != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_color_is_the_aqua_ink_token() {
        let tokens = include_str!("../../ui/src/styles/tokens.css");
        assert!(tokens.contains("--aqua-ink: #0E3B35;"));
        assert_eq!(CAPTION_COLOR, 0x0035_3B0E);
        assert_eq!(CAPTION_TEXT_COLOR, 0x00FF_FFFF);
    }
}

//! Windows backend: a `WH_MOUSE_LL` low-level mouse hook intercepts
//! `WM_MOUSEHWHEEL` globally, decodes it into notch units, feeds the shared
//! `WheelAccumulator`, and injects `VK_VOLUME_UP`/`VK_VOLUME_DOWN` via
//! `SendInput` — the same synthetic key Windows' own OSD volume indicator
//! reacts to, so no separate on-screen-display code is needed. The hook
//! swallows the event (returns 1) so the thumb wheel does not *also*
//! side-scroll whatever window has focus.
//!
//! `mouseData`'s signed high word being a multiple of `WHEEL_DELTA` (120)
//! is the documented convention for `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` and
//! carries over to `MSLLHOOKSTRUCT` (MS Learn: "WM_MOUSEHWHEEL message",
//! "MSLLHOOKSTRUCT structure").

use crate::config::Config;
use crate::core::{VolumeStep, WheelAccumulator};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
    VK_VOLUME_DOWN, VK_VOLUME_UP,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_HREDRAW, CS_VREDRAW, CallNextHookEx, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, HICON, HMENU,
    IDI_APPLICATION, LoadIconW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MF_CHECKED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, MSG, MSLLHOOKSTRUCT, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassExW, SetForegroundWindow, SetWindowsHookExW, TPM_BOTTOMALIGN, TPM_RIGHTALIGN,
    TrackPopupMenu, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL, WM_APP, WM_COMMAND,
    WM_DESTROY, WM_LBUTTONUP, WM_MOUSEHWHEEL, WM_NULL, WM_RBUTTONUP, WNDCLASSEXW,
    WS_OVERLAPPEDWINDOW,
};
use windows::core::{PCWSTR, w};

const WHEEL_DELTA_UNITS: i32 = 120;

/// Extract the signed wheel delta from a `WM_MOUSEHWHEEL` hook's
/// `mouseData`, normalized to notch units (1 == one physical detent,
/// matching evdev's `REL_HWHEEL` convention on Linux).
fn decode_notches(mouse_data: u32) -> i32 {
    let raw = (mouse_data >> 16) as i16 as i32;
    raw / WHEEL_DELTA_UNITS
}

/// A window handle is just an opaque id to the OS, never dereferenced here;
/// `PostMessageW` is explicitly documented as safe to call across threads
/// with one, so sharing it through a `static` (same-thread in practice,
/// since the LL hook always runs on the thread that installed it, but nothing
/// enforces that here) is sound.
struct SendableHwnd(HWND);
unsafe impl Send for SendableHwnd {}
unsafe impl Sync for SendableHwnd {}

static ACCUMULATOR: OnceLock<Mutex<WheelAccumulator>> = OnceLock::new();
static TRAY_HWND: OnceLock<SendableHwnd> = OnceLock::new();
/// Mirrors what's currently in effect (and, once written, on disk) so the
/// tray menu can show correct checkmarks and the invert/sensitivity toggles
/// have something to mutate and persist. Kept in sync with `ACCUMULATOR` —
/// every write to one is immediately followed by a rebuild of the other.
static CURRENT_CONFIG: OnceLock<Mutex<Config>> = OnceLock::new();
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

const TRAY_ICON_ID: u32 = 1;
/// Must match the `100 ICON "tray.ico"` line in `assets/tray.rc`.
const TRAY_ICON_RESOURCE_ID: u32 = 100;
const WM_TRAYICON: u32 = WM_APP + 1;
/// Posted by `hook_proc` to defer the actual `SendInput` calls out of the
/// low-level hook callback (see `hook_proc`'s doc comment for why).
/// `wparam` = 1 for `VolumeStep::Up`, 0 for `Down`; `lparam` = step count.
const WM_VOLUME_STEPS: u32 = WM_APP + 2;
const ID_MENU_RELOAD: usize = 1;
const ID_MENU_ABOUT: usize = 2;
const ID_MENU_EXIT: usize = 3;
const ID_MENU_INVERT: usize = 4;
/// Sensitivity preset `n` (1..=SENSITIVITY_MAX) is menu id `ID_SENSITIVITY_BASE + n`,
/// so ids [ID_SENSITIVITY_MIN_ID, ID_SENSITIVITY_MAX_ID] are the valid presets.
const ID_SENSITIVITY_BASE: usize = 10;
const SENSITIVITY_MAX: i32 = 5;
const ID_SENSITIVITY_MIN_ID: usize = ID_SENSITIVITY_BASE + 1;
const ID_SENSITIVITY_MAX_ID: usize = ID_SENSITIVITY_BASE + SENSITIVITY_MAX as usize;

/// Windows enforces a timeout on `WH_MOUSE_LL` callbacks (`LowLevelHooksTimeout`,
/// a few hundred ms) and *silently* unhooks a callback that overruns it — the
/// daemon keeps running, but the wheel stops doing anything, with no error
/// anywhere. A burst of up to `MAX_STEPS_PER_FEED` `SendInput` syscalls run
/// synchronously here could plausibly hit that budget under load, so this
/// callback does only cheap, syscall-free work (pointer deref, arithmetic, a
/// mutex lock) and posts the *count* of steps needed to the tray window,
/// which performs the actual `SendInput` calls from the normal message loop
/// — outside the hook's timing budget entirely.
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_MOUSEHWHEEL {
        let data = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let notches = decode_notches(data.mouseData);
        if notches != 0
            && let Some(mutex) = ACCUMULATOR.get()
        {
            let steps = mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .feed(notches);
            // A single feed() call only ever produces one direction's worth
            // of steps (see WheelAccumulator::feed), so the first step's
            // direction speaks for all of them.
            if let (Some(&first), Some(hwnd)) = (steps.first(), TRAY_HWND.get()) {
                let direction = WPARAM(usize::from(first == VolumeStep::Up));
                let count = LPARAM(steps.len() as isize);
                let _ = unsafe { PostMessageW(hwnd.0, WM_VOLUME_STEPS, direction, count) };
            }
        }
        // Swallow the event: don't let it also side-scroll the focused app.
        return LRESULT(1);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn key_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_volume_key(step: VolumeStep) {
    let vk = match step {
        VolumeStep::Up => VK_VOLUME_UP,
        VolumeStep::Down => VK_VOLUME_DOWN,
    };
    let inputs = [key_input(vk, false), key_input(vk, true)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn build_accumulator(config: &Config) -> anyhow::Result<WheelAccumulator> {
    WheelAccumulator::new(
        config.wheel.notches_per_step,
        config.wheel.invert,
        config.wheel.sensitivity,
    )
    .map_err(|e| anyhow::anyhow!(e))
}

/// Re-read the config file and swap in a freshly built accumulator. Bad
/// config on reload is reported (message box) but leaves the previous,
/// still-valid accumulator running rather than crashing a background daemon
/// over an edit-in-progress config file.
fn reload(hwnd: HWND) {
    let Some(path) = CONFIG_PATH.get() else {
        return;
    };
    match Config::load_or_default(path).map_err(anyhow::Error::from) {
        Ok(config) => apply_config(&config),
        Err(e) => report_fatal_error(&format!("не вдалося перечитати конфіг: {e}")),
    }
    let _ = hwnd; // reserved for a future "reloaded" balloon tip
}

/// Rebuild `ACCUMULATOR` from `config` and record it as the current
/// effective config; on failure, the previous accumulator/config are left
/// untouched.
fn apply_config(config: &Config) {
    match build_accumulator(config) {
        Ok(new_accumulator) => {
            if let Some(mutex) = ACCUMULATOR.get() {
                *mutex.lock().unwrap_or_else(|p| p.into_inner()) = new_accumulator;
            }
            if let Some(mutex) = CURRENT_CONFIG.get() {
                *mutex.lock().unwrap_or_else(|p| p.into_inner()) = config.clone();
            }
        }
        Err(e) => report_fatal_error(&format!("невалідні налаштування: {e}")),
    }
}

/// Apply `mutate` to the live config, rebuild the accumulator from it, and
/// persist it to disk — the pattern behind every tray-menu setting toggle
/// (invert, sensitivity), so a choice made there survives a restart.
fn update_wheel_config(mutate: impl FnOnce(&mut Config)) {
    let (Some(config_mutex), Some(path)) = (CURRENT_CONFIG.get(), CONFIG_PATH.get()) else {
        return;
    };
    let mut config = config_mutex
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    mutate(&mut config);
    apply_config(&config);
    if let Err(e) = config.save(path) {
        report_fatal_error(&format!("не вдалося зберегти конфіг: {e}"));
    }
}

fn toggle_invert() {
    update_wheel_config(|c| c.wheel.invert = !c.wheel.invert);
}

fn set_sensitivity(level: i32) {
    update_wheel_config(|c| c.wheel.sensitivity = level);
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            if matches!(lparam.0 as u32, WM_RBUTTONUP | WM_LBUTTONUP) {
                show_tray_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_VOLUME_STEPS => {
            let step = if wparam.0 != 0 {
                VolumeStep::Up
            } else {
                VolumeStep::Down
            };
            for _ in 0..lparam.0 {
                send_volume_key(step);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            match wparam.0 & 0xFFFF {
                ID_MENU_RELOAD => reload(hwnd),
                ID_MENU_ABOUT => show_about(),
                ID_MENU_INVERT => toggle_invert(),
                ID_MENU_EXIT => unsafe { PostQuitMessage(0) },
                id @ ID_SENSITIVITY_MIN_ID..=ID_SENSITIVITY_MAX_ID => {
                    set_sensitivity((id - ID_SENSITIVITY_BASE) as i32)
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn current_wheel_config() -> (bool, i32) {
    CURRENT_CONFIG
        .get()
        .map(|m| {
            let c = m.lock().unwrap_or_else(|p| p.into_inner());
            (c.wheel.invert, c.wheel.sensitivity)
        })
        .unwrap_or((false, 2))
}

fn build_sensitivity_submenu(current: i32) -> HMENU {
    unsafe {
        let Ok(submenu) = CreatePopupMenu() else {
            return HMENU::default();
        };
        for level in 1..=SENSITIVITY_MAX {
            let flag = if level == current {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let label = to_wide_null(&level.to_string());
            let _ = AppendMenuW(
                submenu,
                flag,
                ID_SENSITIVITY_BASE + level as usize,
                PCWSTR(label.as_ptr()),
            );
        }
        submenu
    }
}

fn show_tray_menu(hwnd: HWND) {
    let (invert, sensitivity) = current_wheel_config();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let submenu = build_sensitivity_submenu(sensitivity);
        let _ = AppendMenuW(menu, MF_POPUP, submenu.0 as usize, w!("Sensitivity"));
        let invert_flag = if invert {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(menu, invert_flag, ID_MENU_INVERT, w!("Invert direction"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_MENU_RELOAD, w!("Reload"));
        let _ = AppendMenuW(menu, MF_STRING, ID_MENU_ABOUT, w!("About"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_MENU_EXIT, w!("Exit"));

        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        // Required so the menu closes on an outside click; see
        // MS Learn "TrackPopupMenu": "you should force a task switch by
        // calling SetForegroundWindow" and follow up with a benign posted
        // message, otherwise the menu doesn't dismiss reliably.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(
            menu,
            TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
            cursor.x,
            cursor.y,
            0,
            hwnd,
            None,
        );
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn to_wide_fixed<const N: usize>(s: &str) -> [u16; N] {
    let mut buf = [0u16; N];
    for (dst, src) in buf.iter_mut().zip(s.encode_utf16().take(N - 1)) {
        *dst = src;
    }
    buf
}

fn show_about() {
    let text = format!(
        "thumbvol {}\nThumb-wheel-to-volume daemon for Logitech MX Master mice.\n\n{}\nMIT License",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_REPOSITORY"),
    );
    let wide = to_wide_null(&text);
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(wide.as_ptr()),
            w!("About thumbvol"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

pub fn report_fatal_error(message: &str) {
    let text = to_wide_null(message);
    let title = w!("thumbvol");
    unsafe {
        let _ = MessageBoxW(None, PCWSTR(text.as_ptr()), title, MB_OK | MB_ICONERROR);
    }
}

fn create_tray_window() -> anyhow::Result<(HWND, windows::Win32::Foundation::HINSTANCE)> {
    unsafe {
        let hinstance: windows::Win32::Foundation::HINSTANCE = GetModuleHandleW(None)
            .map_err(|e| anyhow::anyhow!("GetModuleHandleW failed: {e}"))?
            .into();
        let class_name = w!("ThumbvolTrayWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: class_name,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(anyhow::anyhow!("RegisterClassExW failed"));
        }

        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            w!("thumbvol"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            None,
            None,
            hinstance,
            None,
        )
        .map_err(|e| anyhow::anyhow!("CreateWindowExW failed: {e}"))?;
        // Deliberately never shown (no ShowWindow call) — this is a
        // message-only host for the tray icon, not a visible window.
        Ok((hwnd, hinstance))
    }
}

/// Load the app's own speaker-glyph icon (compiled in via `build.rs` +
/// `assets/tray.rc`); fall back to the generic system icon if that ever
/// fails (e.g. a stripped-down non-standard build) rather than erroring out
/// over what is purely cosmetic.
fn load_tray_icon(hinstance: windows::Win32::Foundation::HINSTANCE) -> HICON {
    unsafe {
        LoadIconW(hinstance, PCWSTR(TRAY_ICON_RESOURCE_ID as _))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

fn add_tray_icon(
    hwnd: HWND,
    hinstance: windows::Win32::Foundation::HINSTANCE,
) -> anyhow::Result<()> {
    unsafe {
        let icon = load_tray_icon(hinstance);
        let data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ICON_ID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAYICON,
            hIcon: icon,
            szTip: to_wide_fixed("thumbvol"),
            ..Default::default()
        };
        Shell_NotifyIconW(NIM_ADD, &data)
            .ok()
            .map_err(|e| anyhow::anyhow!("Shell_NotifyIconW(NIM_ADD) failed: {e}"))
    }
}

fn remove_tray_icon(hwnd: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// Install the global mouse hook and the tray icon, then pump messages
/// until "Exit" is chosen from the tray menu (or the window is otherwise
/// destroyed).
pub fn run(config: Config, config_path: PathBuf) -> anyhow::Result<()> {
    let _ = CONFIG_PATH.set(config_path);

    ACCUMULATOR
        .set(Mutex::new(build_accumulator(&config)?))
        .map_err(|_| anyhow::anyhow!("platform::windows::run() called more than once"))?;
    let _ = CURRENT_CONFIG.set(Mutex::new(config.clone()));

    let (hwnd, hinstance) = create_tray_window()?;
    let _ = TRAY_HWND.set(SendableHwnd(hwnd));
    add_tray_icon(hwnd, hinstance)?;

    // Installed only after TRAY_HWND is set: hook_proc posts to it as soon
    // as the message loop below starts pumping.
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), None, 0) }
        .map_err(|e| anyhow::anyhow!("failed to install mouse hook: {e}"))?;

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        remove_tray_icon(hwnd);
        let _ = UnhookWindowsHookEx(hook);
    }
    Ok(())
}

const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "thumbvol";

pub fn install_autostart() -> anyhow::Result<()> {
    set_autostart_value(RUN_KEY_PATH, VALUE_NAME, true)
}

pub fn uninstall_autostart() -> anyhow::Result<()> {
    set_autostart_value(RUN_KEY_PATH, VALUE_NAME, false)
}

fn set_autostart_value(key_path: &str, value_name: &str, enable: bool) -> anyhow::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(key_path)?;
    if enable {
        let exe = std::env::current_exe()?;
        // Windows parses an unquoted Run value by splitting on spaces
        // (CWE-428): "C:\Program Files\thumbvol\thumbvol.exe" would try to
        // run "C:\Program.exe" with the rest as arguments. Quoting the
        // whole value is the standard fix.
        key.set_value(value_name, &format!("\"{}\"", exe.display()))?;
    } else {
        match key.delete_value(value_name) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(delta: i16) -> u32 {
        (delta as u16 as u32) << 16
    }

    // --- correctness ---

    #[test]
    fn one_notch_forward_decodes_to_positive_one() {
        assert_eq!(decode_notches(pack(120)), 1);
    }

    #[test]
    fn one_notch_backward_decodes_to_negative_one() {
        assert_eq!(decode_notches(pack(-120)), -1);
    }

    #[test]
    fn fast_spin_decodes_to_multiple_notches() {
        assert_eq!(decode_notches(pack(360)), 3);
        assert_eq!(decode_notches(pack(-240)), -2);
    }

    // --- misuse: degenerate mouseData ---

    #[test]
    fn zero_delta_decodes_to_zero() {
        assert_eq!(decode_notches(pack(0)), 0);
    }

    #[test]
    fn low_word_bits_are_ignored() {
        // Only the high word carries the wheel delta; garbage in the low
        // word (e.g. button/extra-info bits some drivers set) must not
        // perturb the result.
        assert_eq!(decode_notches(pack(120) | 0xBEEF), 1);
    }

    mod autostart_tests {
        use super::super::*;

        // Isolated from the real Run key so `cargo test` never mutates the
        // developer's actual autostart entries. Each test gets its own leaf
        // subkey (not just its own value name) because `cargo test` runs
        // tests in parallel by default and `delete_subkey_all` on a shared
        // parent would race with a sibling test's in-flight write.
        fn cleanup(key_path: &str) {
            let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
            let _ = hkcu.delete_subkey_all(key_path);
        }

        // --- correctness ---

        #[test]
        fn enabling_then_disabling_round_trips() {
            const KEY_PATH: &str = r"Software\ThumbvolAutostartTests\round_trip";
            cleanup(KEY_PATH);
            set_autostart_value(KEY_PATH, "test_entry", true).unwrap();
            let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
            let key = hkcu.open_subkey(KEY_PATH).unwrap();
            let value: String = key.get_value("test_entry").unwrap();
            assert!(!value.is_empty());

            set_autostart_value(KEY_PATH, "test_entry", false).unwrap();
            let key = hkcu.open_subkey(KEY_PATH).unwrap();
            assert!(key.get_value::<String, _>("test_entry").is_err());
            cleanup(KEY_PATH);
        }

        #[test]
        fn stored_path_is_quoted_against_unquoted_search_order_hijack() {
            // CWE-428: an unquoted "C:\Program Files\thumbvol\thumbvol.exe"
            // Run value is parsed by splitting on spaces, so Windows would
            // try "C:\Program.exe" first. The whole value must be wrapped
            // in quotes regardless of where the real exe happens to live.
            const KEY_PATH: &str = r"Software\ThumbvolAutostartTests\quoting";
            cleanup(KEY_PATH);
            set_autostart_value(KEY_PATH, "test_entry", true).unwrap();
            let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
            let key = hkcu.open_subkey(KEY_PATH).unwrap();
            let value: String = key.get_value("test_entry").unwrap();
            assert!(value.starts_with('"') && value.ends_with('"'));
            cleanup(KEY_PATH);
        }

        // --- misuse: disabling something never enabled must not error ---

        #[test]
        fn disabling_when_not_present_is_a_noop() {
            const KEY_PATH: &str = r"Software\ThumbvolAutostartTests\noop_disable";
            cleanup(KEY_PATH);
            set_autostart_value(KEY_PATH, "never_set_entry", false).unwrap();
            cleanup(KEY_PATH);
        }
    }
}

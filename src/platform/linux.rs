//! Linux backend: reads `REL_HWHEEL` ticks from the mouse's evdev device
//! non-exclusively (no `EVIOCGRAB`) and emits `KEY_VOLUMEUP`/`KEY_VOLUMEDOWN`
//! through a virtual uinput keyboard — the same synthetic key most desktop
//! environments already bind to raise/lower volume with their native OSD,
//! mirroring the Windows backend's `SendInput(VK_VOLUME_*)` approach.
//!
//! Not grabbing the device is a deliberate v1 simplification, not an
//! oversight: the mouse's single evdev node also carries clicks and
//! vertical scroll, so an exclusive grab would silently kill normal mouse
//! function too. Suppressing just the horizontal-scroll side effect (like
//! the Windows hook does) would require cloning the device through a
//! passthrough virtual device — deferred; see DECISIONS.md.
//!
//! Unlike `platform::windows`, this file has only been type-checked and
//! linted (`cargo check`/`clippy --target x86_64-unknown-linux-gnu`) from
//! the Windows dev machine, which has no Linux linker — it has never
//! actually been linked, run, or tested against real hardware. CI
//! (ubuntu-latest, which does link) is its first real build; treat it as
//! unverified until that job is green and someone has tested it against a
//! real MX Master. See TASKS.md.

use crate::config::Config;
use crate::core::{VolumeStep, WheelAccumulator};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, Device, EventSummary, EventType, InputEvent, KeyCode, RelativeAxisCode};
use std::path::PathBuf;

/// Find the first evdev device that reports `REL_HWHEEL` — good enough for
/// the target use case (single mouse on a personal desktop); see
/// DECISIONS.md for why this isn't vendor/product-ID based.
pub fn find_wheel_device() -> Option<PathBuf> {
    evdev::enumerate()
        .find(|(_, device)| {
            device
                .supported_relative_axes()
                .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_HWHEEL))
        })
        .map(|(path, _)| path)
}

fn open_device(config: &Config) -> anyhow::Result<Device> {
    let path = match &config.linux.device_path {
        Some(p) => PathBuf::from(p),
        None => find_wheel_device().ok_or_else(|| {
            anyhow::anyhow!(
                "no input device with a horizontal wheel found; set [linux] device_path in the config"
            )
        })?,
    };
    Device::open(&path).map_err(|e| anyhow::anyhow!("failed to open {}: {e}", path.display()))
}

fn build_volume_uinput() -> anyhow::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::KEY_VOLUMEUP);
    keys.insert(KeyCode::KEY_VOLUMEDOWN);

    VirtualDevice::builder()
        .map_err(|e| {
            anyhow::anyhow!("cannot open /dev/uinput (need write access, e.g. `uinput` group): {e}")
        })?
        .name("thumbvol-volume-keys")
        .with_keys(&keys)
        .map_err(|e| anyhow::anyhow!("failed to declare virtual key capabilities: {e}"))?
        .build()
        .map_err(|e| anyhow::anyhow!("failed to create virtual volume-key device: {e}"))
}

fn emit_step(uinput: &mut VirtualDevice, step: VolumeStep) -> anyhow::Result<()> {
    let code = match step {
        VolumeStep::Up => KeyCode::KEY_VOLUMEUP,
        VolumeStep::Down => KeyCode::KEY_VOLUMEDOWN,
    };
    uinput.emit(&[InputEvent::new(EventType::KEY.0, code.0, 1)])?; // press
    uinput.emit(&[InputEvent::new(EventType::KEY.0, code.0, 0)])?; // release
    Ok(())
}

/// `config_path` is unused on Linux today (no reload UI yet — see
/// TASKS.md); kept in the signature so `main.rs` has one call shape for
/// both platforms.
pub fn run(config: Config, config_path: std::path::PathBuf) -> anyhow::Result<()> {
    let _ = config_path;
    let mut accumulator = WheelAccumulator::new(
        config.wheel.notches_per_step,
        config.wheel.invert,
        config.wheel.sensitivity,
    )
    .map_err(|e| anyhow::anyhow!(e))?;
    let mut device = open_device(&config)?;
    let mut uinput = build_volume_uinput()?;

    loop {
        for event in device.fetch_events()? {
            if let EventSummary::RelativeAxis(_, RelativeAxisCode::REL_HWHEEL, value) =
                event.destructure()
            {
                for step in accumulator.feed(value) {
                    emit_step(&mut uinput, step)?;
                }
            }
        }
    }
}

/// Unlike Windows (no console, so fatal errors would otherwise vanish
/// silently — a message box is used there), a Linux daemon's stderr is
/// normally captured by the terminal or the service manager's journal, so
/// that's the right channel here.
pub fn report_fatal_error(message: &str) {
    eprintln!("thumbvol: fatal: {message}");
}

const AUTOSTART_FILE_NAME: &str = "thumbvol.desktop";

fn autostart_dir() -> anyhow::Result<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| anyhow::anyhow!("neither XDG_CONFIG_HOME nor HOME is set"))?;
    Ok(config_home.join("autostart"))
}

/// Quote and escape `arg` per the Desktop Entry Specification's `Exec` key
/// rules, so a path is treated as a single argument instead of being
/// word-split on spaces (the spec, like a shell, splits unquoted `Exec`
/// values on whitespace — an unquoted `Exec=/opt/My App/thumbvol` would
/// exec `/opt/My` with `App/thumbvol` as an argument).
fn escape_desktop_exec_arg(arg: &str) -> String {
    let mut escaped = String::with_capacity(arg.len() + 2);
    escaped.push('"');
    for ch in arg.chars() {
        if matches!(ch, '\\' | '"' | '`' | '$') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped.push('"');
    escaped
}

/// Generate the XDG autostart `.desktop` entry contents for `exe_path`.
/// Pure and testable independent of the filesystem.
fn desktop_entry_contents(exe_path: &std::path::Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=thumbvol\n\
         Comment=Thumb-wheel-to-volume daemon for Logitech MX Master mice\n\
         Exec={}\n\
         X-GNOME-Autostart-enabled=true\n\
         NoDisplay=true\n",
        escape_desktop_exec_arg(&exe_path.display().to_string())
    )
}

pub fn install_autostart() -> anyhow::Result<()> {
    let dir = autostart_dir()?;
    std::fs::create_dir_all(&dir)?;
    let exe = std::env::current_exe()?;
    std::fs::write(dir.join(AUTOSTART_FILE_NAME), desktop_entry_contents(&exe))?;
    Ok(())
}

pub fn uninstall_autostart() -> anyhow::Result<()> {
    let path = autostart_dir()?.join(AUTOSTART_FILE_NAME);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- correctness ---

    #[test]
    fn desktop_entry_contains_required_keys_and_exe_path() {
        let contents = desktop_entry_contents(std::path::Path::new("/usr/local/bin/thumbvol"));
        assert!(contents.contains("[Desktop Entry]"));
        assert!(contents.contains("Type=Application"));
        assert!(contents.contains("Exec=\"/usr/local/bin/thumbvol\""));
    }

    // --- misuse: exe path containing spaces or shell-special characters
    // must be quoted, not word-split by an Exec-key-aware launcher ---

    #[test]
    fn desktop_entry_quotes_paths_with_spaces() {
        // Unquoted, the Desktop Entry Exec key is word-split just like a
        // shell: "Exec=/opt/My App/thumbvol" would exec "/opt/My" with
        // "App/thumbvol" as an argument.
        let contents = desktop_entry_contents(std::path::Path::new("/opt/My App/thumbvol"));
        assert!(contents.contains("Exec=\"/opt/My App/thumbvol\""));
    }

    #[test]
    fn desktop_entry_escapes_reserved_characters_inside_the_quoted_path() {
        let contents = desktop_entry_contents(std::path::Path::new("/opt/\"weird\"$app/thumbvol"));
        assert!(contents.contains("Exec=\"/opt/\\\"weird\\\"\\$app/thumbvol\""));
    }
}

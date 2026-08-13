// No console window on Windows: this runs as a background/autostart daemon,
// not an interactive CLI tool.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::PathBuf;
use thumbvol::config::Config;

fn config_path() -> anyhow::Result<PathBuf> {
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA")
            .ok_or_else(|| anyhow::anyhow!("APPDATA environment variable is not set"))?;
        Ok(PathBuf::from(appdata).join("thumbvol").join("config.toml"))
    }
    #[cfg(target_os = "linux")]
    {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or_else(|| anyhow::anyhow!("neither XDG_CONFIG_HOME nor HOME is set"))?;
        Ok(config_home.join("thumbvol").join("config.toml"))
    }
}

/// Windows runs headless (`windows_subsystem = "windows"`, no console), so
/// an `Err` from `main` would otherwise vanish with nothing but a nonzero
/// exit code — invisible on a daemon with no console and no tray icon
/// until the hook is up. Route it through the platform's own channel
/// (message box on Windows, stderr on Linux) before propagating it.
fn main() -> anyhow::Result<()> {
    if let Err(e) = try_main() {
        thumbvol::platform_current::report_fatal_error(&e.to_string());
        return Err(e);
    }
    Ok(())
}

fn try_main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--uninstall-autostart") {
        return thumbvol::platform_current::uninstall_autostart();
    }

    let config_path = config_path()?;
    let config = Config::load_or_default(&config_path)
        .map_err(|e| anyhow::anyhow!("{}: {e}", config_path.display()))?;

    if config.general.autostart {
        thumbvol::platform_current::install_autostart()?;
    } else {
        thumbvol::platform_current::uninstall_autostart()?;
    }

    thumbvol::platform_current::run(config, config_path)
}

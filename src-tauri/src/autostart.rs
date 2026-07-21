use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

/// 设置开机自启（Windows 注册表 / macOS LaunchAgent）
pub fn set_autostart(app: &AppHandle, enable: bool) -> anyhow::Result<()> {
    let m = app.autolaunch();
    if enable {
        m.enable()?;
    } else {
        m.disable()?;
    }
    Ok(())
}

pub fn is_autostart_enabled(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

/// 构建系统托盘：左键切换窗口显隐，右键菜单提供常用操作
pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let Some(icon) = app.default_window_icon().cloned() else {
        return Ok(());
    };

    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "隐藏窗口", true, None::<&str>)?;
    let rec = MenuItem::with_id(app, "monitor", "开始监控全部房间", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出程序", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &rec, &quit])?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("抖音直播录屏")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => crate::commands::show_window(app),
            "hide" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            "monitor" => {
                let state = app.state::<crate::state::SharedState>();
                let rooms: Vec<String> = {
                    let st = state.lock().unwrap();
                    st.config
                        .rooms
                        .iter()
                        .filter(|r| r.enabled)
                        .map(|r| r.id.clone())
                        .collect()
                };
                for rid in rooms {
                    if let Err(e) = crate::commands::start_monitor_inner(app, &state, &rid) {
                        eprintln!("监控 {} 启动失败: {}", rid, e);
                    }
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 仅响应左键单击 + 双击；右键用于弹菜单
            match event {
                TrayIconEvent::Click {
                    button: tauri::tray::MouseButton::Left,
                    ..
                } => {
                    crate::commands::show_window(tray.app_handle());
                }
                TrayIconEvent::DoubleClick {
                    button: tauri::tray::MouseButton::Left,
                    ..
                } => {
                    crate::commands::show_window(tray.app_handle());
                }
                _ => {}
            }
        })
        .build(app)?;

    Ok(())
}
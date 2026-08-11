mod autostart;
mod commands;
mod config;
mod detector;
mod ffmpeg;
mod logic;
mod monitor;
mod recorder;
mod state;
mod stream_resolver;
mod transcode;
mod tray;
mod types;

#[cfg(test)]
mod tests;

use tauri::Manager;
use state::SharedState;

/// 应用入口（Tauri v2：库形态，便于后续扩展移动端）
pub fn run() {
    let shared: SharedState = std::sync::Arc::new(std::sync::Mutex::new(state::AppState::new()));

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_opener::init())
        .manage(shared)
        .on_window_event(|window, event| {
            // 关闭主窗口仅隐藏，不退出（托盘常驻）
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            tray::setup_tray(app.handle())?;

            // 启动时强制显示主窗口（即使 visible=false 也能稳定出现）
            commands::show_window(app.handle());

            // 关键：把用户配置的输出目录加入 asset 协议作用域
            // 原因：tauri.conf.json 默认 scope 只含 $APPDATA/$VIDEO/$HOME（均解析到 C:\Users\...），
            //       用户常用的 E:\存储\录屏 等盘符外目录会被 asset 协议拒绝（返回 403），
            //       导致「预览剪辑」时 <video> 报"加载失败"。这里运行时按用户配置动态放行。
            {
                let st = app.state::<SharedState>();
                let output_dir = st.lock().unwrap().config.output_dir.clone();
                if let Err(e) = app.asset_protocol_scope().allow_directory(&output_dir, true) {
                    eprintln!("[warn] allow output_dir ({:?}) failed: {}", output_dir, e);
                }
            }

            // 若配置开机自启，则确保注册表/启动项已写入
            {
                let st = app.state::<SharedState>();
                let cfg = st.lock().unwrap().config.clone();
                if cfg.autostart {
                    let _ = autostart::set_autostart(app.handle(), true);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::list_rooms,
            commands::import_rooms,
            commands::remove_room,
            commands::update_room,
            commands::start_recording,
            commands::start_monitor,
            commands::stop_recording,
            commands::stop_monitor,
            commands::list_recordings,
            commands::transcode_file,
            commands::merge_videos,
            commands::export_segments,
            commands::set_autostart,
            commands::get_autostart,
            commands::show_main,
            commands::hide_main,
            commands::get_status,
            commands::resolve_room_url,
            commands::delete_recording,
            commands::list_pending_recordings,
            commands::cleanup_pending_recording,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

mod commands;
mod http_api;
mod models;
mod state;

use state::AppState;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::new())
        .setup(|app| {
            build_tray(app.handle())?;
            // 启动本地 HTTP 桥接服务，供 MCP 适配器 / agent 调用
            http_api::spawn(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // projects
            commands::projects::list_projects,
            commands::projects::save_project,
            commands::projects::delete_project,
            commands::projects::reorder_projects,
            commands::projects::list_project_groups,
            commands::projects::add_project_group,
            commands::projects::rename_project_group,
            commands::projects::delete_project_group,
            commands::projects::detect_project,
            commands::projects::pick_directory,
            // process
            commands::process::start_project,
            commands::process::stop_project,
            commands::process::restart_project,
            commands::process::project_statuses,
            commands::process::health_tick,
            // ports
            commands::ports::list_ports,
            commands::ports::list_project_ports,
            commands::ports::kill_process,
            // hosts
            commands::hosts::get_hosts,
            commands::hosts::get_hosts_raw,
            commands::hosts::save_hosts,
            // services (brew)
            commands::services::list_services,
            commands::services::start_service,
            commands::services::stop_service,
            commands::services::restart_service,
            // logs
            commands::logs::get_logs,
            commands::logs::clear_logs,
            // profiles
            commands::profiles::list_profiles,
            commands::profiles::save_profile,
            commands::profiles::delete_profile,
            commands::profiles::start_profile,
            commands::profiles::stop_profile,
            // shortcuts
            commands::shortcuts::open_in_editor,
            commands::shortcuts::open_url,
            commands::shortcuts::open_terminal,
            commands::shortcuts::reveal_in_finder,
        ])
        .run(tauri::generate_context!())
        .expect("运行 DevBox 时出错");
}

/// 构建菜单栏托盘图标与菜单
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示 DevBox", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("DevBox")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

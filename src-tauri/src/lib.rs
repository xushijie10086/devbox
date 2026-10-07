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
    let app = tauri::Builder::default()
        .manage(AppState::new())
        .setup(|app| {
            build_tray(app.handle())?;
            // 后台巡检：窗口关着也能发现项目崩溃，并持续记录「正在运行的项目」
            commands::lifecycle::spawn_supervisor(app.handle().clone());
            // Ctrl+C / kill：先停掉所有项目再退出，不留孤儿进程
            commands::lifecycle::install_signal_handler(app.handle().clone());
            // 启动本地 HTTP 桥接服务，供 MCP 适配器 / agent 调用
            http_api::spawn(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // 点窗口关闭按钮：有运行中的项目时先让前端确认
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                commands::lifecycle::on_close_requested(window, api);
            }
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
            commands::process::take_exit_events,
            // 应用生命周期
            commands::lifecycle::quit_app,
            commands::lifecycle::pending_restore,
            commands::lifecycle::resolve_restore,
            // git / 运行时版本
            commands::git::git_pull,
            commands::git::git_branches,
            commands::git::git_checkout,
            commands::git::git_fetch,
            commands::git::project_branches,
            commands::runtime::list_runtimes,
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
            // shortcuts
            commands::shortcuts::open_in_editor,
            commands::shortcuts::open_url,
            commands::shortcuts::open_terminal,
            commands::shortcuts::reveal_in_finder,
        ])
        .build(tauri::generate_context!())
        .expect("构建 DevBox 时出错");

    app.run(|app, event| match event {
        // 退出请求（关窗口、托盘「退出」、Cmd+Q）：有运行中的项目时先让前端确认
        tauri::RunEvent::ExitRequested { api, .. } => commands::lifecycle::on_exit_requested(app, &api),
        // 真正退出：记录运行集合并停止所有项目
        tauri::RunEvent::Exit => commands::lifecycle::handle_exit(&app.state::<AppState>()),
        _ => {}
    });
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

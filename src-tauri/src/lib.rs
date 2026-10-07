mod commands;
mod http_api;
mod logstore;
mod models;
mod notify;
mod state;
mod tray;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::new())
        .setup(|app| {
            let _ = state::APP_HANDLE.set(app.handle().clone());
            // 上次被强杀 / 崩溃时遗留的项目进程，先清理掉，免得占着端口、和新启动的冲突
            commands::lifecycle::reap_orphans(&app.state::<AppState>());
            tray::build(app.handle())?;
            // 后台巡检：窗口关着也能发现项目崩溃，并持续记录「正在运行的项目」
            commands::lifecycle::spawn_supervisor(app.handle().clone());
            // 后台刷新各项目的 git 状态（领先 / 落后 / 未提交）
            commands::git::spawn_status_worker(app.handle().clone());
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
            commands::startcmd::detect_start_commands,
            commands::projects::pick_directory,
            // process
            commands::process::start_project,
            commands::process::stop_project,
            commands::process::restart_project,
            commands::process::project_statuses,
            commands::process::health_tick,
            commands::process::take_exit_events,
            // 脚本任务
            commands::scripts::list_scripts,
            commands::scripts::run_script,
            commands::scripts::cancel_script,
            // 系统通知
            notify::get_notifications_enabled,
            notify::set_notifications_enabled,
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
            commands::git::project_git_status,
            commands::runtime::list_runtimes,
            // ports
            commands::ports::list_ports,
            commands::ports::list_project_ports,
            commands::ports::check_project_port,
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
            commands::logs::log_file_path,
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

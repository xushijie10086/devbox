use crate::models::Project;
use crate::state::AppState;
use tauri::State;
use uuid::Uuid;

/// 列出所有项目
#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Vec<Project> {
    state.config.lock().unwrap().projects.clone()
}

/// 新增或更新项目（按 id 判断；id 为空则新建）
#[tauri::command]
pub fn save_project(mut project: Project, state: State<AppState>) -> Result<Project, String> {
    if project.id.trim().is_empty() {
        project.id = Uuid::new_v4().to_string();
    }
    {
        let mut cfg = state.config.lock().unwrap();
        if let Some(existing) = cfg.projects.iter_mut().find(|p| p.id == project.id) {
            *existing = project.clone();
        } else {
            cfg.projects.push(project.clone());
        }
    }
    state.persist()?;
    Ok(project)
}

/// 删除项目
#[tauri::command]
pub fn delete_project(id: String, state: State<AppState>) -> Result<(), String> {
    // 先确保进程已停止
    let _ = crate::commands::process::stop_project_inner(&id, state.inner());
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.projects.retain(|p| p.id != id);
    }
    state.persist()?;
    Ok(())
}

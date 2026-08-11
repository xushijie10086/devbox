use crate::models::Profile;
use crate::state::AppState;
use tauri::State;
use uuid::Uuid;

#[tauri::command]
pub fn list_profiles(state: State<AppState>) -> Vec<Profile> {
    state.config.lock().unwrap().profiles.clone()
}

#[tauri::command]
pub fn save_profile(mut profile: Profile, state: State<AppState>) -> Result<Profile, String> {
    if profile.id.trim().is_empty() {
        profile.id = Uuid::new_v4().to_string();
    }
    {
        let mut cfg = state.config.lock().unwrap();
        if let Some(existing) = cfg.profiles.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile.clone();
        } else {
            cfg.profiles.push(profile.clone());
        }
    }
    state.persist()?;
    Ok(profile)
}

#[tauri::command]
pub fn delete_profile(id: String, state: State<AppState>) -> Result<(), String> {
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.profiles.retain(|p| p.id != id);
    }
    state.persist()?;
    Ok(())
}

/// 启动整个启动组：先拉起 brew 服务，再逐个启动项目
#[tauri::command]
pub fn start_profile(id: String, state: State<AppState>) -> Result<Vec<String>, String> {
    let profile = get_profile(state.inner(), &id).ok_or("找不到启动组")?;
    let mut errors = Vec::new();

    for svc in &profile.service_names {
        if let Err(e) = crate::commands::services::start_service(svc.clone()) {
            errors.push(format!("服务 {svc}: {e}"));
        }
    }
    for pid in &profile.project_ids {
        if let Err(e) = crate::commands::process::start_project_inner(pid, state.inner()) {
            // 已在运行不算错误
            if !e.contains("已在运行") {
                errors.push(format!("项目 {pid}: {e}"));
            }
        }
    }
    Ok(errors)
}

/// 停止整个启动组：先停项目，再停 brew 服务
#[tauri::command]
pub fn stop_profile(id: String, state: State<AppState>) -> Result<Vec<String>, String> {
    let profile = get_profile(state.inner(), &id).ok_or("找不到启动组")?;
    let mut errors = Vec::new();

    for pid in &profile.project_ids {
        if let Err(e) = crate::commands::process::stop_project_inner(pid, state.inner()) {
            errors.push(format!("项目 {pid}: {e}"));
        }
    }
    for svc in &profile.service_names {
        if let Err(e) = crate::commands::services::stop_service(svc.clone()) {
            errors.push(format!("服务 {svc}: {e}"));
        }
    }
    Ok(errors)
}

fn get_profile(state: &AppState, id: &str) -> Option<Profile> {
    state
        .config
        .lock()
        .unwrap()
        .profiles
        .iter()
        .find(|p| p.id == id)
        .cloned()
}

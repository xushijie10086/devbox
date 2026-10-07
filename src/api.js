import { invoke } from "@tauri-apps/api/core";

// 对后端命令的一层薄封装，集中管理命令名，避免拼写漂移
export const api = {
  // 项目
  listProjects: () => invoke("list_projects"),
  saveProject: (project) => invoke("save_project", { project }),
  deleteProject: (id) => invoke("delete_project", { id }),
  reorderProjects: (ids) => invoke("reorder_projects", { ids }),
  listProjectGroups: () => invoke("list_project_groups"),
  addProjectGroup: (name) => invoke("add_project_group", { name }),
  renameProjectGroup: (oldName, newName) => invoke("rename_project_group", { oldName, newName }),
  deleteProjectGroup: (name) => invoke("delete_project_group", { name }),
  detectProject: (path) => invoke("detect_project", { path }),
  pickDirectory: () => invoke("pick_directory"),

  // 进程 / 生命周期
  startProject: (id) => invoke("start_project", { id }),
  stopProject: (id) => invoke("stop_project", { id }),
  restartProject: (id) => invoke("restart_project", { id }),
  projectStatuses: () => invoke("project_statuses"),
  healthTick: () => invoke("health_tick"),

  // 端口
  listPorts: () => invoke("list_ports"),
  listProjectPorts: () => invoke("list_project_ports"),
  killProcess: (pid, port) => invoke("kill_process", { pid, port: port ?? null }),

  // hosts
  getHosts: () => invoke("get_hosts"),
  getHostsRaw: () => invoke("get_hosts_raw"),
  saveHosts: (entries) => invoke("save_hosts", { entries }),

  // brew 服务
  listServices: () => invoke("list_services"),
  startService: (name) => invoke("start_service", { name }),
  stopService: (name) => invoke("stop_service", { name }),
  restartService: (name) => invoke("restart_service", { name }),

  // 日志
  getLogs: (projectId) => invoke("get_logs", { projectId }),
  clearLogs: (projectId) => invoke("clear_logs", { projectId }),


  // 一键拉取代码 / 运行时版本（Node、JDK）
  gitPull: (id) => invoke("git_pull", { id }),
  listRuntimes: () => invoke("list_runtimes"),

  // 快捷入口
  openInEditor: (path, editor) => invoke("open_in_editor", { path, editor }),
  openUrl: (url) => invoke("open_url", { url }),
  openTerminal: (path) => invoke("open_terminal", { path }),
  revealInFinder: (path) => invoke("reveal_in_finder", { path }),
};

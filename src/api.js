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
  detectStartCommands: (path) => invoke("detect_start_commands", { path }),
  pickDirectory: () => invoke("pick_directory"),

  // 进程 / 生命周期
  startProject: (id) => invoke("start_project", { id }),
  stopProject: (id) => invoke("stop_project", { id }),
  restartProject: (id) => invoke("restart_project", { id }),
  projectStatuses: () => invoke("project_statuses"),
  healthTick: () => invoke("health_tick"),

  // 系统通知开关
  getNotificationsEnabled: () => invoke("get_notifications_enabled"),
  setNotificationsEnabled: (enabled) => invoke("set_notifications_enabled", { enabled }),

  // 应用生命周期：退出确认 / 启动时恢复上次运行的项目
  quitApp: () => invoke("quit_app"),
  pendingRestore: () => invoke("pending_restore"),
  resolveRestore: () => invoke("resolve_restore"),
  takeExitEvents: () => invoke("take_exit_events"),

  // 端口
  listPorts: () => invoke("list_ports"),
  listProjectPorts: () => invoke("list_project_ports"),
  checkProjectPort: (id) => invoke("check_project_port", { id }),
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
  // 增量拉取：after 是已见过的最大序号，epoch 是记住的纪元（「清空」会让纪元变化）
  getLogs: (projectId, after = null, epoch = null) => invoke("get_logs", { projectId, after, epoch }),
  clearLogs: (projectId) => invoke("clear_logs", { projectId }),
  logFilePath: (projectId) => invoke("log_file_path", { projectId }),


  // 脚本 / 构建任务
  listScripts: (id) => invoke("list_scripts", { id }),
  runScript: (id, command, label) => invoke("run_script", { id, command, label }),
  cancelScript: (id) => invoke("cancel_script", { id }),

  // 一键拉取代码 / 运行时版本（Node、JDK）
  gitPull: (id) => invoke("git_pull", { id }),
  gitBranches: (id) => invoke("git_branches", { id }),
  gitCheckout: (id, name, kind) => invoke("git_checkout", { id, name, kind }),
  gitFetch: (id) => invoke("git_fetch", { id }),
  projectBranches: () => invoke("project_branches"),
  projectGitStatus: () => invoke("project_git_status"),
  listRuntimes: () => invoke("list_runtimes"),

  // 快捷入口
  openInEditor: (path, editor) => invoke("open_in_editor", { path, editor }),
  openUrl: (url) => invoke("open_url", { url }),
  openTerminal: (path) => invoke("open_terminal", { path }),
  revealInFinder: (path) => invoke("reveal_in_finder", { path }),
};

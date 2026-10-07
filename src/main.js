import * as projects from "./views/projects.js";
import * as ports from "./views/ports.js";
import * as services from "./views/services.js";
import * as hosts from "./views/hosts.js";
import * as logs from "./views/logs.js";
import { icon } from "./icons.js";
import { api } from "./api.js";
import { confirmDialog, toast, summarizeStart } from "./ui.js";
import { listen } from "@tauri-apps/api/event";

const views = { projects, ports, services, hosts, logs };

const viewRoot = document.getElementById("view-root");
const nav = document.getElementById("nav");

// 注入自绘 SVG 图标（品牌、导航、刷新）
const logoEl = document.querySelector(".brand .logo");
if (logoEl) {
  const b = icon("brand", 22);
  b.classList.add("logo");
  logoEl.replaceWith(b);
}
for (const btn of nav.querySelectorAll(".nav-item")) {
  btn.prepend(icon(btn.dataset.view, 18));
}
const refreshBtn = document.getElementById("refresh-all");
if (refreshBtn) refreshBtn.prepend(icon("refresh", 16));
// 通知开关：显示当前状态，点击切换（托盘菜单里也能切，窗口获得焦点时同步）
const notifyBtn = document.getElementById("notify-toggle");
async function syncNotifyBtn() {
  try {
    const on = await api.getNotificationsEnabled();
    notifyBtn.classList.toggle("off", !on);
    notifyBtn.replaceChildren(icon(on ? "bell" : "bellOff", 16), document.createTextNode(on ? "通知：开" : "通知：关"));
  } catch (_) {}
}
notifyBtn.addEventListener("click", async () => {
  try {
    const on = await api.getNotificationsEnabled();
    await api.setNotificationsEnabled(!on);
    toast(on ? "已关闭系统通知" : "已开启系统通知（项目崩溃时，窗口不在前台会弹出）", "info");
  } catch (e) {
    toast(String(e), "error");
  }
  await syncNotifyBtn();
});
window.addEventListener("focus", syncNotifyBtn);
syncNotifyBtn();

let cleanup = null;
let currentName = "projects";

function switchView(name) {
  if (cleanup) { try { cleanup(); } catch (_) {} cleanup = null; }
  viewRoot.innerHTML = "";
  currentName = name;
  for (const btn of nav.querySelectorAll(".nav-item")) {
    btn.classList.toggle("active", btn.dataset.view === name);
  }
  const mod = views[name];
  if (mod && mod.mount) cleanup = mod.mount(viewRoot);
}

nav.addEventListener("click", (e) => {
  const btn = e.target.closest(".nav-item");
  if (btn) switchView(btn.dataset.view);
});

document.getElementById("refresh-all").addEventListener("click", () => switchView(currentName));

switchView("projects");

// ---------- 应用生命周期 ----------

// 退出（关窗口 / 托盘「退出」/ Cmd+Q）时若有运行中的项目，后端会拦下并通知这里确认。
// 退出必然停止项目：项目的输出管道由 DevBox 持有，DevBox 退出后它们会因 SIGPIPE 崩溃。
let askingQuit = false;
listen("quit-requested", async (e) => {
  if (askingQuit) return;
  askingQuit = true;
  try {
    const { projects = 0, jobs = 0 } = e.payload || {};
    const parts = [];
    if (projects) parts.push(`${projects} 个项目在运行`);
    if (jobs) parts.push(`${jobs} 个脚本任务在运行`);
    const tail = projects ? "退出 DevBox 会停止它们，下次启动时可以一键恢复项目。" : "退出 DevBox 会取消它们。";
    const ok = await confirmDialog(`现在有 ${parts.join("、")}。\n${tail}\n\n确定退出吗？`, { okText: "停止并退出", cancelText: "取消" });
    if (ok) await api.quitApp();
  } finally {
    askingQuit = false;
  }
}).catch(() => {});

// 启动时：上次退出（或崩溃、被强杀）时有项目在运行，问一下要不要恢复。
// 后端会先把上次异常退出后残留的项目进程清理掉（否则它们占着端口，恢复启动会冲突），这里一并告知。
(async () => {
  let info = { items: [], cleaned: [] };
  try {
    info = (await api.pendingRestore()) || info;
  } catch (_) {
    return;
  }
  const items = info.items || [];
  const cleaned = info.cleaned || [];
  const cleanedNote = cleaned.length ? `\n（上次异常退出后，这些项目的进程还残留着，已先清理：${cleaned.join("、")}）` : "";
  if (items.length === 0) {
    if (cleaned.length) toast(`已清理上次异常退出残留的项目进程：${cleaned.join("、")}`, "info");
    return;
  }
  const ok = await confirmDialog(
    `上次退出时有 ${items.length} 个项目在运行：\n${items.map((i) => `· ${i.name}`).join("\n")}${cleanedNote}\n\n要恢复启动它们吗？`,
    { okText: "恢复启动", cancelText: "忽略", danger: false },
  );
  if (ok) {
    toast("正在恢复启动…", "info");
    const results = await Promise.allSettled(items.map((i) => api.startProject(i.id)));
    const sum = summarizeStart(items, results);
    toast(sum.text, sum.kind);
  }
  await api.resolveRestore().catch(() => {});
  switchView(currentName); // 刷新当前页，显示最新的运行状态
})();

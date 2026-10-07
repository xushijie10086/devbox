import * as projects from "./views/projects.js";
import * as ports from "./views/ports.js";
import * as services from "./views/services.js";
import * as hosts from "./views/hosts.js";
import * as logs from "./views/logs.js";
import { icon } from "./icons.js";

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

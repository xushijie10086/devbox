// 统一的描边式 SVG 图标库（24x24，currentColor 描边，随主题着色）
// 用 icon(name, size) 取得一个 <svg> 元素，插入到按钮 / 导航项中。

const PATHS = {
  // 品牌：立方体
  brand:
    '<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z"/><path d="M4 7.5l8 4.5 8-4.5"/><path d="M12 12v9"/>',
  // 导航：项目（立方体 / 包）
  projects:
    '<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z"/><path d="M4 7.5l8 4.5 8-4.5"/><path d="M12 12v9"/>',
  // 导航：启动组（火箭）
  profiles:
    '<path d="M12 3c2.8 1.6 4.5 4.8 4.5 8.5 0 1.9-.5 3.6-1.3 4.9H8.8C8 15.1 7.5 13.4 7.5 11.5 7.5 7.8 9.2 4.6 12 3z"/><circle cx="12" cy="9.5" r="1.6"/><path d="M8.8 16.4l-2.3 2.6 3-.9M15.2 16.4l2.3 2.6-3-.9"/>',
  // 导航：端口（插头）
  ports:
    '<path d="M9 3v4M15 3v4"/><path d="M7 7h10v3.2a5 5 0 0 1-10 0V7z"/><path d="M12 15.2V21"/>',
  // 导航：服务（数据库）
  services:
    '<ellipse cx="12" cy="6" rx="7" ry="3"/><path d="M5 6v6c0 1.66 3.13 3 7 3s7-1.34 7-3V6"/><path d="M5 12v6c0 1.66 3.13 3 7 3s7-1.34 7-3v-6"/>',
  // 导航：hosts（地球）
  hosts:
    '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3c2.5 2.6 3.6 6 3.6 9s-1.1 6.4-3.6 9c-2.5-2.6-3.6-6-3.6-9S9.5 5.6 12 3z"/>',
  // 导航 / 卡片：日志（终端）
  logs:
    '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 9l3 3-3 3M13 15h4"/>',
  // 卡片：在编辑器打开（代码括号）
  code:
    '<path d="M8 7l-4 5 4 5M16 7l4 5-4 5M13.5 5l-3 14"/>',
  // 卡片：在浏览器打开（外链）
  browser:
    '<path d="M14 4h6v6"/><path d="M20 4l-8.5 8.5"/><path d="M18 13v5a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h5"/>',
  // 卡片：在终端打开
  terminal:
    '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 9l3 3-3 3M13 15h4"/>',
  // 卡片：在访达显示（文件夹）
  folder:
    '<path d="M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z"/>',
  // 卡片：查看日志（列表）
  logsView:
    '<path d="M8 6h12M8 12h12M8 18h12"/><circle cx="4" cy="6" r="1.1"/><circle cx="4" cy="12" r="1.1"/><circle cx="4" cy="18" r="1.1"/>',
  // 卡片：编辑（铅笔）
  edit:
    '<path d="M4 20h4L19 9l-4-4L4 16v4z"/><path d="M13.5 6.5l4 4"/>',
  // 卡片：删除（垃圾桶）
  trash:
    '<path d="M4 7h16"/><path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/><path d="M6.5 7l1 12a1 1 0 0 0 1 .9h7a1 1 0 0 0 1-.9l1-12"/><path d="M10 11v6M14 11v6"/>',
  // 表单：自动获取（魔法棒 / 星火）
  magic:
    '<path d="M15 4l1.2 2.8L19 8l-2.8 1.2L15 12l-1.2-2.8L11 8l2.8-1.2L15 4z"/><path d="M6 13l.8 1.9L8.7 15.7 6.8 16.5 6 18.4l-.8-1.9L3.3 15.7l1.9-.8L6 13z"/>',
  // 底部：刷新
  refresh:
    '<path d="M4 12a8 8 0 0 1 13.7-5.6L20 8"/><path d="M20 3.5V8h-4.5"/><path d="M20 12a8 8 0 0 1-13.7 5.6L4 16"/><path d="M4 20.5V16h4.5"/>',
};

/**
 * @param {string} name  图标名（见 PATHS）
 * @param {number} size  像素尺寸，默认 20
 * @returns {SVGSVGElement}
 */
export function icon(name, size = 20) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.7");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.classList.add("icon");
  svg.innerHTML = PATHS[name] || "";
  return svg;
}

// 项目环境（dev / test / prod …）的展示逻辑。环境的识别和生效在后端 src-tauri/src/commands/envs.rs。

/** 看起来是生产环境的名字：选中时标成红色，避免手滑 */
export function isProdEnv(name) {
  return /^(prod|production|online|release)$/i.test(name || "");
}

/**
 * 项目列表上「环境」标签的内容。项目里没识别到任何环境时返回 null（不显示标签）。
 * info 是后端 project_envs 给的 { active, profiles, default }。
 */
export function envChipView(info) {
  if (!info || !info.profiles || info.profiles.length === 0) return null;
  const active = info.active || "";
  if (!active) {
    const dflt = info.default ? `（${info.default}）` : "";
    return { text: "环境 默认", title: `使用项目原本的环境${dflt}（点击切换环境）`, cls: "" };
  }
  if (!info.profiles.some((p) => p.name === active)) {
    return {
      text: `环境 ${active} ⚠`,
      title: `已选择环境「${active}」，但项目里找不到它，启动会被拒绝（点击重新选择）`,
      cls: "set env-missing",
    };
  }
  return { text: `环境 ${active}`, title: `当前环境：${active}（点击切换）`, cls: `set${isProdEnv(active) ? " env-prod" : ""}` };
}

/** 切换环境后的提示：正在运行的项目要重启才会生效 */
export function envSwitchNotice(message, running) {
  return running
    ? { text: `${message}；项目正在运行，需点「重启」才会生效`, level: "warning" }
    : { text: message, level: "success" };
}

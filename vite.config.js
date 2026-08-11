import { defineConfig } from "vite";

// Tauri 期望前端跑在固定端口，且构建产物输出到 dist
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // 不监听 Rust 源码目录，避免无谓的热更新
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: "es2021",
    outDir: "dist",
  },
});

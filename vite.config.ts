import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { usageBridge } from "./scripts/usageBridge";
export default defineConfig({
  define: {
    __OBS_BUILD_TIME__: JSON.stringify(new Date().toISOString()),
  },
  plugins: [react(), usageBridge()],
  server: {
    watch: {
      ignored: [
        "**/native/target/**",
        "**/src-tauri/target/**",
        "**/target/**",
        "**/*.log",
        "**/.local-data/**",
        // Vite 原子更新 CSS 时会在源文件旁建一个 `.name.<hash>.tmpdir`
        // 临时目录；在 Windows 上 watch 那个临时文件会抛 EBUSY，watcher
        // 随之崩溃，开发服务器直接退出。改一次样式就断服务，必须忽略。
        "**/*.tmpdir/**",
      ],
    },
    fs: {
      deny: [
        ".env",
        ".env.*",
        "**/.git/**",
        "**/.local-data/**",
        "**/native/target/**",
        "**/src-tauri/target/**",
      ],
    },
  },
  build: {
    modulePreload: false,
    assetsInlineLimit: 1000000,
    rollupOptions: { output: { inlineDynamicImports: true } },
  },
});

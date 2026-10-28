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

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
        // When Vite updates CSS atomically it creates a `.name.<hash>.tmpdir`
        // temporary directory next to the source file; watching that temporary
        // file on Windows throws EBUSY, the watcher crashes with it and the dev
        // server exits outright. Losing the server on every style change is
        // unacceptable, so it has to be ignored.
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
    // Pages are split into their own chunks (see the lazy imports in main.tsx) so
    // startup and the tray panel load only the shell; Tauri serves dist/ through
    // its asset protocol, which loads the chunks like any web server.
  },
});

// Browser tests for the UI: `npm run native:build` once, then `npm run test:ui`.
// They drive the dev server (the same JSON-lines engine the app uses) against an
// empty temporary home, so they never read or change real usage data or logs,
// and run in the locally installed Chrome, so no browser download is needed.
import { defineConfig } from "@playwright/test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const home = fs.mkdtempSync(path.join(os.tmpdir(), "observatory-ui-"));
const port = 5199;

export default defineConfig({
  testDir: "tests-ui",
  timeout: 30_000,
  fullyParallel: false,
  workers: 1,
  reporter: "list",
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    channel: "chrome",
    headless: true,
    viewport: { width: 1360, height: 900 },
  },
  webServer: {
    command: `npx vite --port ${port} --strictPort --host 127.0.0.1`,
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: false,
    timeout: 60_000,
    env: {
      // Home and data directory for the engine and every collector.
      OBSERVATORY_TEST_HOME: home,
      // No first-launch import from a real CC Switch database.
      CC_SWITCH_DB: path.join(home, "no-cc-switch.db"),
    },
  },
});

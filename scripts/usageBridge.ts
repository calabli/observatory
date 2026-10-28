import type { Plugin, ViteDevServer, PreviewServer } from "vite";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";
import { randomBytes, timingSafeEqual } from "node:crypto";
import { existsSync } from "node:fs";
import { join } from "node:path";

export function usageBridge(): Plugin {
  let child: ChildProcessWithoutNullStreams | undefined;
  let id = 0;
  let closed = false;
  const token = randomBytes(32).toString("hex");
  const pending = new Map<
    number,
    {
      resolve: (v: unknown) => void;
      reject: (e: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    }
  >();
  const stop = () => {
    closed = true;
    child?.stdin.end();
    setTimeout(() => child?.kill(), 1000).unref();
  };
  function start() {
    if (child && !child.killed) return child;
    if (closed) throw new Error("Usage engine has stopped");
    const binary = join(
      process.cwd(),
      "native",
      "target",
      "debug",
      "observatory-engine" + (process.platform === "win32" ? ".exe" : ""),
    );
    if (!existsSync(binary))
      throw new Error("原生统计引擎尚未构建，请先运行 npm run native:build");
    child = spawn(binary, [], {
      cwd: process.cwd(),
      windowsHide: true,
      env: process.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      try {
        const result = JSON.parse(line);
        const task = pending.get(result.id);
        if (!task) return;
        clearTimeout(task.timer);
        pending.delete(result.id);
        result.error
          ? task.reject(new Error(result.error))
          : task.resolve(result.result);
      } catch {
        /* Ignore non-protocol output. */
      }
    });
    let failure = "";
    child.stderr.on("data", (buf) => {
      failure = (failure + String(buf)).slice(-4000);
    });
    const fail = (message: string) => {
      for (const task of pending.values()) {
        clearTimeout(task.timer);
        task.reject(new Error(message));
      }
      pending.clear();
      child = undefined;
    };
    child.stdin.on("error", (e) => fail(e.message));
    child.on("error", (e) => fail(e.message));
    child.on("exit", (code) =>
      fail(failure || `Usage engine exited (${code})`),
    );
    return child;
  }
  function call(op: string, args: unknown): Promise<unknown> {
    const engine = start();
    const requestId = ++id;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => {
          pending.delete(requestId);
          reject(new Error("统计引擎请求超时，请稍后刷新"));
        },
        op === "sync" ? 600000 : 120000,
      );
      pending.set(requestId, { resolve, reject, timer });
      engine.stdin.write(JSON.stringify({ id: requestId, op, args }) + "\n");
    });
  }
  const configure = (server: ViteDevServer | PreviewServer) => {
    server.httpServer?.once("close", stop);
    server.middlewares.use(async (req, res, next) => {
      const path = (req.url || "").split("?")[0];
      if (!path.startsWith("/__observatory/")) {
        next();
        return;
      }
      res.setHeader("Content-Type", "application/json; charset=utf-8");
      res.setHeader("Cache-Control", "no-store");
      const host = req.headers.host || "";
      if (!/^(localhost|127\.0\.0\.1|\[::1\])(?::\d+)?$/.test(host)) {
        res.statusCode = 403;
        res.end(JSON.stringify({ error: "Local access only" }));
        return;
      }
      if (req.headers.origin && req.headers.origin !== `http://${host}`) {
        res.statusCode = 403;
        res.end(JSON.stringify({ error: "Cross-origin access denied" }));
        return;
      }
      if (path === "/__observatory/session" && req.method === "GET") {
        if (
          req.headers["sec-fetch-site"] &&
          req.headers["sec-fetch-site"] !== "same-origin"
        ) {
          res.statusCode = 403;
          res.end(JSON.stringify({ error: "Same-origin access required" }));
          return;
        }
        res.end(JSON.stringify({ token }));
        return;
      }
      const supplied = String(req.headers["x-observatory-token"] || "");
      const suppliedBytes = Buffer.from(supplied);
      const tokenBytes = Buffer.from(token);
      if (
        req.method !== "POST" ||
        path !== "/__observatory/usage" ||
        suppliedBytes.length !== tokenBytes.length ||
        !timingSafeEqual(suppliedBytes, tokenBytes)
      ) {
        res.statusCode = 403;
        res.end(JSON.stringify({ error: "Invalid local session" }));
        return;
      }
      try {
        let data = "";
        for await (const chunk of req) {
          data += String(chunk);
          if (data.length > 4000000) throw new Error("Request too large");
        }
        const request = JSON.parse(data);
        if (typeof request.op !== "string")
          throw new Error("Invalid operation");
        const result = await call(request.op, request.args ?? {});
        res.end(JSON.stringify({ result }));
      } catch (error) {
        res.statusCode = 500;
        res.end(
          JSON.stringify({
            error: error instanceof Error ? error.message : String(error),
          }),
        );
      }
    });
  };
  return {
    name: "observatory-local-usage",
    configureServer: configure,
    configurePreviewServer: configure,
  };
}

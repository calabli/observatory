import { spawn } from "node:child_process";
const args = process.argv.slice(2);
const child = spawn(
  "cargo",
  [
    "+stable",
    ...(args.length ? args : ["build"]),
    "--manifest-path",
    "native/Cargo.toml",
  ],
  { stdio: "inherit", windowsHide: true, shell: false },
);
child.on("error", () => {
  console.error("Rust/Cargo 不可用，请安装 Rust stable 与平台编译工具。");
  process.exitCode = 1;
});
child.on("exit", (code) => {
  process.exitCode = code ?? 1;
});

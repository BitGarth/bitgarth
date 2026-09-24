import { spawn } from "node:child_process";
import fs from "node:fs";

const [command, ...args] = process.argv.slice(2);
const logPath = process.env.BITGARTH_E2E_LOG_PATH;
const child = spawn(command === "node" ? process.execPath : command, args, {
  stdio: ["ignore", "pipe", "pipe"],
});

for (const stream of ["stdout", "stderr"]) {
  child[stream].on("data", (data) => {
    fs.appendFileSync(logPath, data);
    process[stream].write(data);
  });
}

child.on("error", (error) => {
  console.error(error);
  process.exitCode = 1;
});
child.on("exit", (code) => {
  process.exitCode = code ?? 1;
});

import { clearRunDirectoryLock } from "./run-artifacts.mjs";

export default function globalTeardown() {
  clearRunDirectoryLock({ testType: "e2e" });
}

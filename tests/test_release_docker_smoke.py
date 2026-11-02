import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DockerSmokeTests(unittest.TestCase):
    def test_health_failure_blocks_success_and_always_removes_container(self):
        script = ROOT / "scripts/release/smoke-docker.sh"
        self.assertTrue(script.is_file(), "Docker startup check must exist")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "git").write_text('#!/bin/sh\nprintf "aaaaaaaaaaaa\\n"\n')
            (root / "docker").write_text('''#!/bin/sh
echo "$*" >> "$SMOKE_LOG"
case "$1" in
  run) echo container-id ;;
  inspect) echo "$SMOKE_HEALTH" ;;
  exec) exit "$SMOKE_EXEC_EXIT" ;;
esac
''')
            for name in ("git", "docker"):
                (root / name).chmod(0o755)
            for health, exec_exit, success in (("running|healthy", "0", True),
                                               ("running|unhealthy", "0", False),
                                               ("exited|unhealthy", "0", False),
                                               ("running|healthy", "1", False)):
                with self.subTest(health=health, exec_exit=exec_exit):
                    log = root / "log"
                    log.write_text("")
                    result = subprocess.run(["bash", str(script), "v1.2.3", "amd64"],
                                            env={**os.environ, "PATH": f"{root}:{os.environ['PATH']}",
                                                 "SMOKE_LOG": str(log), "SMOKE_HEALTH": health,
                                                 "SMOKE_EXEC_EXIT": exec_exit},
                                            capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode == 0, success, result.stderr)
                    self.assertIn("rm -f -v container-id", log.read_text())
                    self.assertIn("--network none", log.read_text())


if __name__ == "__main__":
    unittest.main()

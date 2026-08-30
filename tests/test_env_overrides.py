"""env-overrides.sh must report a stale override and stay quiet otherwise."""

import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "env-overrides.sh"


def _fake(path: Path, body: str) -> None:
    path.write_text("#!/bin/sh\n" + body)
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


class EnvOverrides(unittest.TestCase):
    def run_with(self, generated: str, live: str):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            gen = tmp / "gen"
            _fake(gen, f"printf '%s\\n' '{generated}'\n")
            # A fake systemctl on PATH stands in for the live manager.
            binq = tmp / "bin"
            binq.mkdir()
            _fake(binq / "systemctl", f"printf '%s\\n' '{live}'\n")
            env = dict(os.environ, ENV_GENERATOR=str(gen), PATH=f"{binq}:{os.environ['PATH']}")
            return subprocess.run([str(SCRIPT)], env=env, capture_output=True, text=True)

    def test_agreement_is_silent_and_exits_zero(self):
        r = self.run_with("WALLPAPER_PATH=/a.png", "WALLPAPER_PATH=/a.png")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout, "")

    def test_a_stale_override_is_named_and_exits_one(self):
        r = self.run_with("WALLPAPER_PATH=/real.png", "WALLPAPER_PATH=/stale.png")
        self.assertEqual(r.returncode, 1)
        self.assertIn("WALLPAPER_PATH live=/stale.png generated=/real.png", r.stdout)

    def test_a_variable_missing_from_the_live_manager_is_reported(self):
        # environment.d gained a file after the manager started: the value
        # is simply absent live, which is the lingering case.
        r = self.run_with("NEWVAR=1", "OTHER=2")
        self.assertEqual(r.returncode, 1)
        self.assertIn("NEWVAR live=<unset> generated=1", r.stdout)


if __name__ == "__main__":
    unittest.main()

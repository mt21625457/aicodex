"""Check SDK CI staging without invoking Bazel or a runtime binary."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class SdkWorkflowTest(unittest.TestCase):
    def test_exported_runtime_path_matches_staged_bazel_output(self):
        workflow = Path(__file__).resolve().parents[1] / "workflows/sdk.yml"
        text = workflow.read_text()
        staging = text.split("# Stage both binaries together", 1)[1]
        staging = staging.split("\n      - name:", 1)[0]
        # Drop the remainder of the first comment line, preserving the actual
        # workflow commands and their exported path.
        staging = staging.split("\n", 1)[1]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            outputs = root / "bazel output"
            outputs.mkdir()
            binaries = [outputs / "aicodex", outputs / "codex-code-mode-host"]
            for binary in binaries:
                binary.write_bytes(b"prebuilt fixture; never executed")
            environment_file = root / "environment"
            environment_file.touch()
            env = {
                **os.environ,
                "GITHUB_WORKSPACE": str(root),
                "GITHUB_ENV": str(environment_file),
            }
            subprocess.run(
                [
                    "bash",
                    "-c",
                    'set -euo pipefail\nbazel_output_paths=("$1" "$2")\n' + staging,
                    "sdk-stage-test",
                    *(str(binary) for binary in binaries),
                ],
                env=env,
                check=True,
                capture_output=True,
            )
            exported = dict(
                line.split("=", 1) for line in environment_file.read_text().splitlines()
            )
            entrypoint = Path(exported["CODEX_EXEC_PATH"])
            self.assertEqual(entrypoint.read_bytes(), binaries[0].read_bytes())
            self.assertTrue(os.access(entrypoint, os.X_OK))
            self.assertEqual(
                (entrypoint.parent / binaries[1].name).read_bytes(),
                binaries[1].read_bytes(),
            )


if __name__ == "__main__":
    unittest.main()

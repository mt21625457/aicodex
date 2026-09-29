"""验证已交付 CLI 能通过相邻 Code Mode 宿主执行工具，不依赖真实账号或模型服务。"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MARKER = "AICODEX_CODE_MODE_RUNTIME_OK"


def verify_runtime(binary: Path) -> None:
    """以隔离 home 和本地 Responses 服务验证真实子进程及工具输出回传。"""
    requests = []
    source = (
        'text(await tools.exec_command({cmd: "echo ' + MARKER + '", login: false}));'
    )

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            if not self.path.endswith("/responses"):
                self.send_error(404)
                return
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append(body)
            if len(requests) == 1:
                item = {
                    "type": "custom_tool_call",
                    "call_id": "runtime-smoke",
                    "name": "exec",
                    "input": source,
                }
            else:
                item = {
                    "type": "message",
                    "id": "done",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "runtime verified"}],
                }
            events = [
                {
                    "type": "response.created",
                    "response": {"id": f"smoke-{len(requests)}"},
                },
                {"type": "response.output_item.done", "item": item},
                {
                    "type": "response.completed",
                    "response": {
                        "id": f"smoke-{len(requests)}",
                        "usage": {
                            "input_tokens": 1,
                            "output_tokens": 1,
                            "total_tokens": 2,
                        },
                    },
                },
            ]
            payload = "".join(
                f"data: {json.dumps(event)}\n\n" for event in events
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    with tempfile.TemporaryDirectory(prefix="aicodex-runtime-smoke-") as tmp:
        home = Path(tmp)
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        # 使用白名单环境，避免读取真实凭据、代理或已有会话；只运行固定的 echo 命令。
        env = {
            key: os.environ[key]
            for key in ("PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT")
            if key in os.environ
        }
        env.update(
            HOME=tmp,
            USERPROFILE=tmp,
            CODEX_HOME=tmp,
            AICODEX_HOME=tmp,
            NO_PROXY="127.0.0.1,localhost",
            RUST_LOG="warn",
        )
        (home / "config.toml").write_text(
            f'model = "gpt-6-astra"\nmodel_provider = "smoke"\n'
            f'[model_providers.smoke]\nname = "smoke"\n'
            f'base_url = "http://127.0.0.1:{server.server_port}/v1"\n'
            'wire_api = "responses"\nrequires_openai_auth = false\n'
            "[features]\ncode_mode_only = true\nmemories = false\napps = false\n"
            "plugins = false\n[analytics]\nenabled = false\n",
            encoding="utf-8",
        )
        try:
            result = subprocess.run(
                [
                    str(binary.resolve()),
                    "exec",
                    "--skip-git-repo-check",
                    "--json",
                    "--sandbox",
                    "danger-full-access",
                    "Verify the runtime.",
                ],
                cwd=tmp,
                env=env,
                text=True,
                capture_output=True,
                timeout=90,
            )
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
        if result.returncode:
            raise RuntimeError(
                f"CLI failed ({result.returncode}): {result.stderr[-6000:]}"
            )
        outputs = [
            item.get("output")
            for request in requests
            for item in request.get("input", [])
            if item.get("type") == "custom_tool_call_output"
            and item.get("call_id") == "runtime-smoke"
        ]
        # 必须验证实际命令退出码与输出，不能把错误中回显的输入命令误认作成功。
        command_results = []
        if len(outputs) == 1 and isinstance(outputs[0], list):
            for content in outputs[0]:
                text = content.get("text", "")
                if not text.startswith("{"):
                    continue
                value = json.loads(text)
                if isinstance(value, dict) and "exit_code" in value:
                    command_results.append(value)
        if (
            len(command_results) != 1
            or command_results[0].get("exit_code") != 0
            or command_results[0].get("output", "").strip() != MARKER
        ):
            raise RuntimeError(
                f"Code Mode output missing or failed: {outputs!r}\n{result.stderr[-3000:]}"
            )
        print(
            json.dumps(
                {
                    "ok": True,
                    "binary": str(binary.resolve()),
                    "code_mode_tool_output": outputs[0],
                },
                ensure_ascii=False,
            )
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    verify_runtime(parser.parse_args().binary)

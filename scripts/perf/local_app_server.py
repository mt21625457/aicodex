#!/usr/bin/env python3
"""使用本地固定 SSE 服务比较完整 app-server 进程，不读取真实 API 凭据。

CPU/RSS 是整个子进程指标；请求延迟包含本地协议、历史和持久化处理。
本脚本不测累计分配，也不将本地 mock 延迟解释为远端服务性能。
"""
import argparse
from contextlib import contextmanager
import hashlib
import http.server
import json
import math
import os
from pathlib import Path
import queue
import re
import statistics
import subprocess
import tempfile
import threading
import time


def percentile(values, fraction):
    return sorted(values)[min(len(values)-1, max(0, math.ceil(len(values)*fraction)-1))]


class Fixture(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        body = b'{"data":[]}'
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        raw = self.rfile.read(int(self.headers['Content-Length']))
        body = json.loads(raw)
        # 原样保留请求，进程结束后再规范化/计算摘要，避免校验开销进入请求延迟。
        self.server.requests.append({'body': body, 'bytes': len(raw)})
        count = len(self.server.requests)
        text = self.server.output_text
        events = [
            {'type': 'response.created', 'response': {'id': f'resp_{count}'}},
            {'type': 'response.output_item.done', 'item': {
                'type': 'message', 'role': 'assistant', 'id': f'msg_{count}',
                'content': [{'type': 'output_text', 'text': text}]}},
            {'type': 'response.completed', 'response': {'id': f'resp_{count}',
                'usage': {'input_tokens': 100, 'output_tokens': 20, 'total_tokens': 120}}},
        ]
        payload = ''.join('event: '+e['type']+'\ndata: '+json.dumps(e)+'\n\n' for e in events).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)



# 仅规范化隔离运行中实际变化的协议标识，正文中的任意 UUID 不会被替换。
UUID_PATTERN = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
METADATA_ID_FIELDS = {
    'turn_id', 'thread_id', 'root_turn_id', 'session_id', 'installation_id',
    'window_id', 'context_window_id', 'x-codex-window-id', 'x-codex-installation-id',
}


def request_fingerprints(requests, state_root):
    """完整请求 JSON 等价检查；保留字段缺失/null区别、工具/指令和全部输入。

    同一生成 ID 在多次请求中的对应关系保留，避免掩盖错误复用/重建标识。
    """
    ids = {}
    normalized_paths = set()

    def visit(value, path):
        if isinstance(value, dict):
            return {key: visit(value[key], (*path, key)) for key in sorted(value)}
        if isinstance(value, list):
            return [visit(item, (*path, str(index))) for index, item in enumerate(value)]
        joined = '/'.join(path)
        if (len(path) == 3 and path[:2] == ('client_metadata', 'x-codex-turn-metadata')
                and path[-1] == 'turn_started_at_unix_ms'):
            assert type(value) is int, (joined, value)
            normalized_paths.add(joined)
            return 0
        if not isinstance(value, str):
            return value
        if path == ('client_metadata', 'x-codex-turn-metadata'):
            parsed = json.loads(value)
            assert isinstance(parsed, dict)
            return json.dumps(visit(parsed, path), sort_keys=True, separators=(',', ':'))
        # 只替换本次实验生成的状态目录，不折叠其他路径、文本或错误信息。
        if state_root in value:
            value = value.replace(state_root, '<isolated-state>')
            normalized_paths.add(joined + ' [isolated state path]')
        identifier = (path == ('prompt_cache_key',)
            or (len(path) == 3 and path[0] == 'input' and path[-1] == 'id')
            or (path and path[0] == 'client_metadata' and path[-1] in METADATA_ID_FIELDS))
        if identifier:
            def replace(match):
                raw = match.group(0)
                if raw not in ids:
                    ids[raw] = f'<generated-uuid-{len(ids)}>'
                return ids[raw]
            replaced = UUID_PATTERN.sub(replace, value)
            if replaced != value:
                normalized_paths.add(joined + ' [generated UUID]')
            value = replaced
        return value

    hashes = []
    for request in requests:
        normalized = visit(request['body'], ())
        encoded = json.dumps(normalized, sort_keys=True, separators=(',', ':')).encode()
        hashes.append(hashlib.sha256(encoded).hexdigest())
    return hashes, sorted(normalized_paths)

@contextmanager
def fixture_server():
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
    server.requests = []
    server.output_text = 'local benchmark response\n' * 128
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        yield server
    finally:
        # 包含临时目录、配置写入或 Popen 失败，不能只在子进程启动成功后清理。
        server.shutdown()
        server.server_close()
        worker.join()


def receive_message(messages, predicate, notifications, timeout=45):
    deadline = time.monotonic()+timeout
    while True:
        remaining = deadline-time.monotonic()
        # 即使队列持续收到非目标通知，也必须遵守总等待上限。
        if remaining <= 0:
            raise TimeoutError('app-server response deadline exceeded')
        try:
            message = messages.get(timeout=remaining)
        except queue.Empty as error:
            raise TimeoutError('app-server response deadline exceeded') from error
        if 'eof' in message or 'invalid_stdout' in message:
            raise AssertionError(message)
        if 'error' in message:
            raise AssertionError(message)
        if predicate(message):
            return message
        notifications.append(message)


def run(binary, turns, text_bytes, output_dir, label):
    with fixture_server() as server, tempfile.TemporaryDirectory(prefix='aicodex-local-perf-') as temporary:
        root = Path(temporary).resolve()
        state = root/'state'
        state.mkdir()
        # 两版使用同一个只读工作目录，使环境消息也能逐字比较。
        work = output_dir.resolve()/'workspace'
        work.mkdir(exist_ok=True)
        (state/'config.toml').write_text(f'''model = "gpt-6.1-sol"
model_provider = "local_perf"
approval_policy = "never"
sandbox_mode = "read-only"
[model_providers.local_perf]
name = "Local performance fixture"
base_url = "http://127.0.0.1:{server.server_port}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0
''')
        # 仅继承启动工具所需的非凭据环境；AICODEX_HOME 指向新建隔离目录。
        child_env = {key: os.environ[key] for key in ('PATH', 'TMPDIR', 'LANG', 'SYSTEMROOT')
                     if key in os.environ}
        child_env.update(AICODEX_HOME=str(state), NO_PROXY='127.0.0.1,localhost,::1',
                         RUST_MIN_STACK='33554432', TERM='dumb')
        stderr_path = output_dir/(label+'.stderr')
        with stderr_path.open('w') as stderr:
            start = time.perf_counter()
            proc = subprocess.Popen([str(binary), 'app-server'], cwd=work, env=child_env,
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=stderr, text=True, bufsize=1)
            messages = queue.Queue()
            def read():
                for line in proc.stdout:
                    try:
                        messages.put(json.loads(line))
                    except json.JSONDecodeError:
                        messages.put({'invalid_stdout': line})
                messages.put({'eof': True})
            threading.Thread(target=read, daemon=True).start()
            notifications = []
            def send(value):
                proc.stdin.write(json.dumps(value)+'\n')
                proc.stdin.flush()
            def receive(predicate, timeout=45):
                return receive_message(messages, predicate, notifications, timeout)
            def rpc(ident, method, params):
                send({'id': ident, 'method': method, 'params': params})
                return receive(lambda m: m.get('id') == ident)['result']
            try:
                rpc(1, 'initialize', {'clientInfo': {'name':'local_perf', 'version':'1'},
                                     'capabilities': {'experimentalApi': True}})
                send({'method':'initialized'})
                initialized_ms=(time.perf_counter()-start)*1000
                thread = rpc(2, 'thread/start', {'cwd':str(work), 'approvalPolicy':'never',
                              'sandbox':'read-only', 'ephemeral':True})['thread']
                startup_ms=(time.perf_counter()-start)*1000
                latencies=[]
                assistant=[]
                for index in range(turns):
                    before=time.perf_counter()
                    result=rpc(10+index, 'turn/start', {'threadId':thread['id'], 'input':[
                        {'type':'text', 'text':f'benchmark turn {index}\n'+'x'*text_bytes}]})
                    turn_id=result['turn']['id']
                    complete=receive(lambda m: m.get('method')=='turn/completed'
                        and m['params']['turn']['id']==turn_id)
                    assert complete['params']['turn']['status']=='completed', complete
                    latencies.append((time.perf_counter()-before)*1000)
                for note in notifications:
                    if note.get('method')=='item/completed':
                        item=note['params']['item']
                        if item.get('type')=='agentMessage':
                            assistant.append(item['text'])
                assert assistant == [server.output_text]*turns, (len(assistant), turns)
                assert len(server.requests)==turns, len(server.requests)
            finally:
                proc.stdin.close()
                # wait4 同时获取该进程自己的 CPU 和最大 RSS，避免混入父进程 mock 服务。
                deadline=time.monotonic()+15
                forced_termination=False
                kill_sent=False
                while True:
                    pid,status,usage=os.wait4(proc.pid, os.WNOHANG)
                    if pid:
                        break
                    if time.monotonic()>deadline:
                        if not forced_termination:
                            proc.terminate()
                            forced_termination=True
                            deadline=time.monotonic()+2
                        elif not kill_sent:
                            proc.kill()
                            kill_sent=True
                            deadline=time.monotonic()+5
                        else:
                            raise RuntimeError('app-server could not be reaped after SIGKILL')
                    time.sleep(.02)
                proc.returncode=os.waitstatus_to_exitcode(status)
                proc.stdout.close()
            assert not forced_termination, 'forced shutdown is not a valid performance sample'
            assert proc.returncode == 0, ('app-server exit', proc.returncode)
            request_hashes, normalization_paths = request_fingerprints(server.requests, str(state))
            result={
                'label':label, 'binary':str(binary), 'exit_code':proc.returncode,
                'turns':turns, 'input_bytes_per_turn':text_bytes,
                'cpu_user_s':usage.ru_utime, 'cpu_system_s':usage.ru_stime,
                'max_rss_bytes':usage.ru_maxrss if os.uname().sysname=='Darwin' else usage.ru_maxrss*1024,
                'initialize_ms':initialized_ms, 'thread_start_ms':startup_ms,
                'turn_latency_ms':latencies, 'turn_p50_ms':statistics.median(latencies),
                'turn_p95_ms':percentile(latencies,.95),
                'input_sizes':[r['bytes'] for r in server.requests],
                'normalized_request_sha256':request_hashes,
                'request_normalization_paths':normalization_paths,
                'assistant_sha256':hashlib.sha256(''.join(assistant).encode()).hexdigest(),
                'allocation_measurement':'not instrumented; see component probes',
            }
            return result


def publish_pair(results, pair, output_path):
    # 未通过等价校验的样本不能写入成功结果，即使两个进程都正常退出。
    assert pair[0]['normalized_request_sha256']==pair[1]['normalized_request_sha256'], 'normalized complete request JSON differs'
    assert pair[0]['request_normalization_paths']==pair[1]['request_normalization_paths'], 'request normalization paths differ'
    assert pair[0]['assistant_sha256']==pair[1]['assistant_sha256'], 'assistant output differs'
    results.extend(pair)
    output_path.write_text(json.dumps(results, indent=2))


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before',type=Path,required=True)
    parser.add_argument('--after',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--rounds',type=int,default=5)
    parser.add_argument('--turns',type=int,default=12)
    parser.add_argument('--text-bytes',type=int,default=32768)
    args=parser.parse_args()
    if args.rounds < 1 or args.turns < 1 or args.text_bytes < 0:
        parser.error('rounds/turns must be positive and text-bytes must be nonnegative')
    args.output.mkdir(parents=True,exist_ok=True)
    results=[]
    # 开始本次运行时清空成功样本，避免第一对失败后留下上一轮的 results.json。
    output_path=args.output/'results.json'
    output_path.write_text('[]\n')
    for round_index in range(args.rounds):
        order=[('before',args.before),('after',args.after)]
        if round_index%2: order.reverse()
        pair=[]
        for name,binary in order:
            result=run(binary.resolve(),args.turns,args.text_bytes,args.output,f'{name}-{round_index}')
            pair.append(result)
            print(json.dumps(result),flush=True)
        publish_pair(results, pair, output_path)
    print('Equivalent complete request JSON after documented normalization and assistant output across all pairs.',flush=True)

if __name__=='__main__':
    main()

#!/usr/bin/env python3
"""显式运行的 DeepSeek 官方 Responses 缓存实验；不等同于 Rust 端到端验收。"""
import argparse
import hashlib
import json
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from urllib.parse import urlsplit


def load_config(path):
    # 不执行 shell、不打印环境文件；密钥仅在进程内用于已核验的官方主机。
    values = {}
    for line in path.read_text().splitlines():
        if '=' in line and not line.lstrip().startswith('#'):
            key, value = line.split('=', 1)
            values[key.strip()] = value.strip().strip('\"\'')
    url = values.get('url', 'https://api.deepseek.com').rstrip('/')
    parsed = urlsplit(url)
    if parsed.scheme != 'https' or parsed.hostname != 'api.deepseek.com' or parsed.username or parsed.query:
        raise ValueError('仅允许 HTTPS 官方 api.deepseek.com 端点')
    return url, values['api_key']


class NoRedirect(urllib.request.HTTPRedirectHandler):
    # 阻止 Authorization 随重定向发送到其他主机。
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def request(url, key, payload=None):
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(url, data=data, headers={
        'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'})
    start = time.monotonic()
    try:
        with urllib.request.build_opener(NoRedirect).open(req, timeout=90) as res:
            if 'text/event-stream' in res.headers.get('Content-Type', ''):
                # Rust 导出的原始请求使用 stream=true；不改请求，读取完整 SSE 终态。
                response = {'error': 'missing SSE terminal response'}
                for line in res:
                    if not line.startswith(b'data: '):
                        continue
                    data = line[6:].strip()
                    if data == b'[DONE]':
                        continue
                    event = json.loads(data)
                    if event.get('type') in ['response.completed', 'response.incomplete', 'response.failed']:
                        response = event['response']
                return res.status, response, round(time.monotonic() - start, 3)
            return res.status, json.load(res), round(time.monotonic() - start, 3)
    except urllib.error.HTTPError as exc:
        # 不把任意服务端原文写入报告，避免回显凭据。
        try:
            error = json.loads(exc.read()).get('error', {})
            message = str(error.get('message', 'HTTP error')).replace(key, '[REDACTED]')
        except (ValueError, AttributeError):
            message = 'HTTP error'
        return exc.code, {'error': message[:500]}, round(time.monotonic() - start, 3)


def fixture(model, nonce):
    # 使用纯合成内容，不发送本地源码、工作目录、用户历史。
    tools = [{'type': 'function', 'name': f'inspect_record_{i}',
              'description': f'Fixture tool {i}. ' + 'Inspect synthetic inventory metadata, revision labels and audit status without changing records. ' * 32,
              'parameters': {'type': 'object', 'properties': {'record': {'type': 'string'}}, 'required': ['record'], 'additionalProperties': False}, 'strict': True}
             for i in range(8)]
    history = '\n'.join(f'Inventory entry {i:03}: synthetic component passed validation; no action required.' for i in range(80))
    return {'model': model, 'instructions': f'Experiment {nonce}. You summarize synthetic project status accurately. Never call tools in this experiment.',
            'input': [{'role': 'user', 'content': history + '\nProject ORBIT: owner Lin, deadline 2030-04-05, blocker missing checksum. Reply READY only.'}],
            'tools': tools, 'parallel_tool_calls': True, 'tool_choice': 'auto',
            'reasoning': {'effort': 'low'}, 'max_output_tokens': 1024, 'store': False}


def summarize(status, response, seconds):
    output = response.get('output', [])
    text = '\n'.join(c.get('text', '') for item in output if item.get('type') == 'message' and item.get('role') == 'assistant' for c in item.get('content', []) if c.get('type') == 'output_text')
    # 与 Rust 校验保持一致：只接受 assistant 消息和 reasoning。
    calls = [item.get('type') for item in output
             if item.get('type') != 'reasoning'
             and not (item.get('type') == 'message' and item.get('role') == 'assistant')]
    last_message = next((item for item in reversed(output)
                         if item.get('type') == 'message' and item.get('role') == 'assistant'), {})
    last_text = '\n'.join(c.get('text', '') for c in last_message.get('content', [])
                          if c.get('type') == 'output_text')
    if not last_text.strip():
        calls.append('missing_assistant_summary')
    return {'http_status': status, 'response_status': response.get('status'), 'returned_model': response.get('model'),
            'seconds': seconds, 'usage': response.get('usage'), 'text': text, 'tool_calls': calls,
            'facts_present': all(s.lower() in text.lower() for s in ['ORBIT', 'Lin', '2030-04-05', 'checksum']),
            'incomplete_details': response.get('incomplete_details'), 'error': response.get('error')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--env', type=Path, default=Path('.env'))
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--rust-fixtures', type=Path, help='原样回放 Rust 测试导出的 warm/baseline/optimized 请求')
    args = parser.parse_args()
    url, key = load_config(args.env)
    status, directory, _ = request(url + '/models', key)
    if status != 200:
        raise SystemExit('模型目录访问失败')
    models = [m['id'] for m in directory.get('data', []) if m['id'].startswith('deepseek')]
    if not models:
        raise SystemExit('官方目录没有返回 DeepSeek 模型，未执行验证')
    if len(models) > 2:
        raise SystemExit('模型数超过本次两模型预算，请人工调整实验范围')
    if args.rust_fixtures:
        report = {'evidence_layer': 'live replay of compiled Rust request-builder exports; not full CLI E2E',
                  'endpoint': url + '/responses', 'results': []}
        for model in models:
            for variant in ['warm', 'baseline', 'optimized']:
                path = args.rust_fixtures / f'{model}-{variant}.json'
                raw = path.read_bytes()
                body = json.loads(raw)
                if body.get('model') != model or not 0 < body.get('max_output_tokens', 0) <= 1024:
                    raise ValueError('导出请求的模型或输出预算不符合本次实验范围')
                status, response, seconds = request(url + '/responses', key, body)
                item = {'model': model, 'variant': variant, 'fixture_sha256': hashlib.sha256(raw).hexdigest(),
                        **summarize(status, response, seconds)}
                # 以下事实与 Rust fixture 保持一致，仅对摘要阶段断言。
                facts = ['ORCHID', 'Rust', '2026-11-02', '73', 'pagination']
                item['facts_present'] = (all(f.lower() in item['text'].lower() for f in facts)
                                         if variant != 'warm' else None)
                report['results'].append(item)
                print(json.dumps(item), flush=True)
                args.output.write_text(json.dumps(report, indent=2) + '\n')
                if status != 200:
                    break
        if any(i['http_status'] != 200 or i['response_status'] != 'completed' or i['tool_calls'] or i['facts_present'] is False for i in report['results']):
            raise SystemExit(1)
        return
    report = {'evidence_layer': 'synthetic Python Responses protocol experiment; NOT compiled Rust integration',
              'endpoint': url + '/responses', 'models': models, 'max_requests': 10, 'max_output_tokens_each': 1024, 'results': []}
    for model in models:
        body = fixture(model, uuid.uuid4().hex)
        # 两次原请求建立并复查缓存；同一模型内所有变体保持 reasoning/输出预算一致。
        for label in ['warm', 'warm_repeat', 'baseline_empty_tools', 'retained_tools_auto', 'retained_tools_none']:
            candidate = json.loads(json.dumps(body))
            if label not in ['warm', 'warm_repeat']:
                candidate['input'] += [{'role': 'assistant', 'content': 'READY'}, {'role': 'user', 'content': 'Summarize the project, owner, deadline and blocker in one sentence. Do not call tools.'}]
            if label == 'baseline_empty_tools':
                candidate['tools'] = []
                candidate['parallel_tool_calls'] = False
            if label == 'retained_tools_none':
                candidate['tool_choice'] = 'none'
            status, response, seconds = request(url + '/responses', key, candidate)
            item = {'model': model, 'variant': label, **summarize(status, response, seconds)}
            report['results'].append(item)
            print(json.dumps(item), flush=True)
            args.output.write_text(json.dumps(report, indent=2) + '\n')
            if status != 200:
                break

    if any(i['http_status'] != 200 or i['response_status'] != 'completed' or i['tool_calls'] or (i['variant'] not in ['warm', 'warm_repeat'] and not i['facts_present']) for i in report['results']):
        raise SystemExit(1)


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""有界长历史冷/热缓存实验：扩展 Rust fixture，明确不声称原样 Rust 请求回放。"""
import argparse
import copy
import hashlib
import json
import uuid
from pathlib import Path
from validate_deepseek_compaction_prefix import load_config, request, summarize


def expanded_fixture(source, nonce, lines):
    body = copy.deepcopy(source)
    # 每个独立 arm 在最早文本及工具说明引入随机前缀，减少跨实验缓存污染。
    body['input'][0]['content'][0]['text'] = f'Independent fixture {nonce}. ' + body['input'][0]['content'][0]['text']
    body['tools'][0]['description'] = f'Independent tool fixture {nonce}. ' + body['tools'][0]['description']
    body['prompt_cache_key'] = nonce
    records = '\n'.join(f'Record {i:04}: synthetic component validation completed; owner Team{i % 13}; iteration {i % 17}; checks passed; no pending work for this record.' for i in range(lines))
    original = body['input'][1]['content'][0]['text']
    body['input'][1]['content'][0]['text'] = records + '\n' + original
    return body


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--env', type=Path, default=Path('.env'))
    parser.add_argument('--rust-fixtures', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--lines', type=int, default=360)
    parser.add_argument('--eligible-smoke', action='store_true', help='8 次补充请求：缩短合成工具描述至800字符，检查低schema比例；非原样回放')
    args = parser.parse_args()
    if not 200 <= args.lines <= 450:
        raise SystemExit('合成历史限制为 200 至 450 行')
    url, key = load_config(args.env)
    status, directory, _ = request(url + '/models', key)
    models = [m['id'] for m in directory.get('data', []) if m['id'].startswith('deepseek')]
    if status != 200 or set(models) != {'deepseek-flash', 'deepseek-v4-pro'}:
        raise SystemExit('官方模型目录与已验证范围不符')
    report = {'evidence_layer': 'synthetic long-history expansion of Rust builder exports, NOT exact builder replay or CLI E2E',
              'endpoint': url + '/responses', 'max_generation_requests': 8 if args.eligible_smoke else 24, 'max_output_tokens_each': 1024,
              'synthetic_history_lines': args.lines, 'tool_description_chars': 800 if args.eligible_smoke else 'unchanged', 'repetitions': 1 if args.eligible_smoke else 2, 'results': []}
    for model_index, model in enumerate(models):
        fixture_path = args.rust_fixtures / f'{model}-optimized.json'
        raw = fixture_path.read_bytes()
        source = json.loads(raw)
        if source['model'] != model or source.get('max_output_tokens') != 1024:
            raise SystemExit('fixture 模型或预算不符合约束')
        if args.eligible_smoke:
            source['tools'][0]['description'] = source['tools'][0]['description'][:800]
        for repetition in range(1 if args.eligible_smoke else 2):
            # 两轮按 AB/BA 平衡顺序；每种冷热状态和每个 arm 使用独立随机前缀。
            arms = ['baseline', 'retained'] if (repetition + model_index) % 2 == 0 else ['retained', 'baseline']
            states = ['cold', 'warm'] if repetition == 0 else ['warm', 'cold']
            for state in states:
                for arm in arms:
                    if args.eligible_smoke and state == 'warm' and arm == 'baseline':
                        continue
                    body = expanded_fixture(source, uuid.uuid4().hex, args.lines)
                    stages = ['prewarm', 'summary'] if state == 'warm' else ['summary']
                    for stage in stages:
                        candidate = copy.deepcopy(body)
                        if stage == 'prewarm':
                            candidate['input'] = candidate['input'][:-1]
                        elif arm == 'baseline':
                            candidate['tools'] = []
                            candidate['parallel_tool_calls'] = False
                        status, response, seconds = request(url + '/responses', key, candidate)
                        item = {'model': model, 'repetition': repetition, 'cache_state': state, 'arm': arm,
                                'stage': stage, 'source_fixture_sha256': hashlib.sha256(raw).hexdigest(),
                                'expanded_request_sha256': hashlib.sha256(json.dumps(candidate, sort_keys=True).encode()).hexdigest(),
                                **summarize(status, response, seconds)}
                        facts = ['ORCHID', 'Rust', '2026-11-02', '73', 'pagination']
                        item['facts_present'] = all(f.lower() in item['text'].lower() for f in facts) if stage == 'summary' else None
                        # 响应仅来自合成资料，保留正文用于诊断事实缺失；不保存凭据或请求全文。
                        report['results'].append(item)
                        args.output.write_text(json.dumps(report, indent=2) + '\n')
                        print(json.dumps(item), flush=True)
                        if status != 200 or item['response_status'] != 'completed' or item['tool_calls'] or item['facts_present'] is False:
                            raise SystemExit(1)


if __name__ == '__main__':
    main()

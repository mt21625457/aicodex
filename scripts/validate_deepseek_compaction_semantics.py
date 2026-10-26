#!/usr/bin/env python3
"""回放编译 Rust 导出的摘要语义矩阵；只发送合成 fixture，保留失败证据。"""
import argparse
import hashlib
import json
import re
from pathlib import Path
from validate_deepseek_compaction_prefix import load_config, request, summarize


def fact_checks(text, expected):
    # 确定性检查仅验证标记是否出现；报告仍须阅读全文核对归属、否定和过期事实。
    def contains(alias):
        value = str(alias)
        before = r'(?<!\w)' if value and value[0].isalnum() else ''
        after = r'(?!\w)' if value and value[-1].isalnum() else ''
        return re.search(before + re.escape(value) + after, text, re.IGNORECASE) is not None
    return {name: any(contains(alias) for alias in (value if isinstance(value, list) else [value]))
            for name, value in expected.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--env', type=Path, default=Path('.env'))
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--final-optimized-only', action='store_true', help='最终验收：仅repetition0 warm+optimized，共8次，允许4096输出上限')
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    cases = manifest['cases'] if isinstance(manifest, dict) else manifest
    if len(cases) != 8:
        raise SystemExit('本轮限制为两模型、两情境、两重复，共8组24次请求')
    if args.final_optimized_only:
        cases = [case for case in cases if case['repetition'] == 0]
        if len(cases) != 4:
            raise SystemExit('最终验收必须为两模型×两情境')
    output_cap = 4096 if args.final_optimized_only else 1024
    url, key = load_config(args.env)
    status, directory, _ = request(url + '/models', key)
    official_models = {m['id'] for m in directory.get('data', [])}
    if status != 200 or not {'deepseek-flash', 'deepseek-v4-pro'} <= official_models:
        raise SystemExit('模型目录不符合已验证范围')
    report = {'evidence_layer': 'exact replay of compiled Rust synthetic semantic fixtures; not CLI E2E',
              'comparison': ('final optimized only; larger output cap, not comparable to previous baseline latency/cost' if args.final_optimized_only else 'baseline original prompt without tools vs optimized directive plus eligible tool prefix; combined intervention'),
              'endpoint': url + '/responses', 'max_requests': 8 if args.final_optimized_only else 24, 'max_output_tokens_each': output_cap,
              'fact_check_limit': 'lexical markers plus separately recorded full-text review; not a semantic equivalence proof',
              'manifest_sha256': hashlib.sha256(args.manifest.read_bytes()).hexdigest(), 'results': []}
    for index, case in enumerate(cases):
        model = case['model']
        if model not in {'deepseek-flash', 'deepseek-v4-pro'}:
            raise SystemExit('拒绝非本轮官方模型fixture')
        # 两次重复按AB/BA排列，预热始终首先执行。请求体完全由Rust fixture提供。
        arms = ['baseline', 'optimized'] if case['repetition'] % 2 == 0 else ['optimized', 'baseline']
        if args.final_optimized_only:
            arms = ['optimized']
        for variant in ['warm'] + arms:
            path = args.manifest.parent / case[variant]
            raw = path.read_bytes()
            body = json.loads(raw)
            if body.get('model') != model or not 0 < body.get('max_output_tokens', 0) <= output_cap:
                raise SystemExit('fixture 模型或token上限不符合约束')
            status, response, seconds = request(url + '/responses', key, body)
            item = {'case_index': index, 'model': model, 'scenario': case['scenario'], 'repetition': case['repetition'],
                    'variant': variant, 'fixture_sha256': hashlib.sha256(raw).hexdigest(),
                    **summarize(status, response, seconds)}
            checks = fact_checks(item['text'], case['expected_facts']) if variant != 'warm' else {}
            forbidden = fact_checks(item['text'], case.get('forbidden_facts', {})) if variant != 'warm' else {}
            item['expected_facts'] = case['expected_facts'] if variant != 'warm' else {}
            item['fact_checks'] = checks
            item['forbidden_facts_present'] = {k:v for k,v in forbidden.items() if v}
            item['facts_present'] = all(checks.values()) if checks else None
            report['results'].append(item)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
            print(json.dumps(item, ensure_ascii=False), flush=True)
            # HTTP失败停止以避免无意义消耗；语义失败仍完成有界矩阵，保留全部失败样本。
            if status != 200:
                raise SystemExit(1)
    if any(i['response_status'] != 'completed' or i['tool_calls'] or i['facts_present'] is False or i['forbidden_facts_present'] for i in report['results']):
        raise SystemExit(1)


if __name__ == '__main__':
    main()

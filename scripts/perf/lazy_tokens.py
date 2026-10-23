#!/usr/bin/env python3
"""提取真实请求预算/历史估算源码，比较惰性估算；不运行 Cargo，不访问服务。"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess

from components import DEFAULT_BASELINE, FIXTURES, ROOT, git_source, run_child, verify_binary

DEPS = ['libc', 'serde', 'serde_json', 'codex_protocol', 'codex_utils_cache',
        'codex_utils_audio', 'codex_utils_output_truncation', 'base64', 'image', 'tracing']
IMPORTS = '''
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_protocol::DEFAULT_FUNCTION_NAMESPACE;
use codex_protocol::models::{AgentMessageInputContent, ContentItem, FunctionCallOutputBody,
 FunctionCallOutputContentItem, ImageDetail, ImageReference, ResponseItem};
use codex_utils_audio::estimate_audio_token_count;
use codex_utils_cache::{BlockingLruCache, sha1_digest};
use codex_utils_output_truncation::{approx_bytes_for_tokens, approx_token_count,
 approx_tokens_from_byte_count_i64};
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock, OnceLock};
'''


def section(text, start, end):
    """锚点失配立即失败，不退回模拟估算器。"""
    assert text.count(start) == 1, start
    left = text.index(start)
    return text[left:text.index(end, left)]


def build(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    baseline = subprocess.check_output(['git', 'rev-parse', '--verify', f'{args.baseline}^{{commit}}'],
                                       cwd=ROOT, text=True).strip()
    manifest = {'baseline': baseline, 'sources': {}, 'platform': platform.platform(),
                'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
                'scope': 'request output-budget stage only; real ResponseItem and serialized tools; no complete Prompt/client/network'}
    def source(path, variant):
        value = git_source(baseline, path) if variant == 'before' else (ROOT / path).read_text()
        manifest['sources'][f'{variant}:{path}'] = hashlib.sha256(value.encode()).hexdigest()
        return value
    parts = []
    for variant in ['before', 'after']:
        history = source('codex-rs/core/src/context_manager/history.rs', variant)
        estimates = section(history, 'fn estimate_reasoning_length(', 'fn is_model_generated_item(')
        # 只加次数计数；保留估算算法和所有分支。每种模式同样插桩。
        for name, counter in [('estimate_request_input_tokens', 'ESTIMATES'),
                              ('estimate_serialized_tokens', 'TOOL_SERIALIZATIONS')]:
            start = estimates.index(f'pub(crate) fn {name}')
            brace = estimates.index('{', start)
            estimates = estimates[:brace+1] + f'\ncrate::{counter}.fetch_add(1, std::sync::atomic::Ordering::Relaxed);' + estimates[brace+1:]
        json_util = source('codex-rs/core/src/utils/json.rs', variant)
        common = source('codex-rs/codex-api/src/common.rs', variant)
        tool_type = section(common, '#[derive(Debug, Clone)]\npub struct ResponsesApiTools',
                            '#[derive(Debug, Serialize, Clone, PartialEq)]\npub struct ResponsesApiRequest')
        client = source('codex-rs/core/src/client.rs', variant)
        if variant == 'before':
            body = section(client, '        let tool_tokens = tools\n', '        let request = ResponsesApiRequest {')
            body += '\nmodel_info.clamped_max_output_tokens(Some(estimated_input))\n'
            helper = ''
        else:
            body = section(client, '        let max_output_tokens = output_token_budget::clamp_with_lazy_input',
                           '        let request = ResponsesApiRequest {')
            body += '\nmax_output_tokens\n'
            helper = 'mod output_token_budget {\n' + source('codex-rs/core/src/client_output_token_budget.rs', variant) + '\n}\n'
        body = body.replace('crate::context_manager::', 'context_manager::')
        parts.append(f'''mod {variant} {{
use std::sync::Arc; use serde::Serialize; use serde_json::value::RawValue;
use codex_protocol::models::ResponseItem; use codex_protocol::openai_models::ModelInfo;
{tool_type}
mod context_manager {{
{IMPORTS}
{json_util}
{estimates}
}}
{helper}
pub fn budget(model_info: &ModelInfo, input: &[ResponseItem], tools: &Option<ResponsesApiTools>) -> Option<u64> {{
{body}
}}
}}''')
    catalog = output / 'models.json'
    catalog.write_text(source('codex-rs/models-manager/models.json', 'before'))
    metrics = (FIXTURES / 'metrics.rs').read_text()
    fixture = (FIXTURES / 'lazy_tokens.rs').read_text().replace('__MODELS_PATH__', json.dumps(str(catalog)))
    generated = metrics + '\n' + '\n'.join(parts) + '\n' + fixture
    rust = output / 'lazy_tokens.rs'
    rust.write_text(generated)
    overrides = dict(value.split('=', 1) for value in args.rlib)
    libraries = {}
    for name in DEPS:
        candidates = list(args.deps.resolve().glob(f'lib{name}-*.rlib'))
        if name in overrides:
            path = Path(overrides[name]).resolve()
        elif candidates:
            path = max(candidates, key=lambda p: p.stat().st_mtime_ns)
        else:
            raise RuntimeError(f'缺少 {name}；先集中构建，或指定 --rlib {name}=路径')
        libraries[name] = str(path)
    binary = output / 'lazy_tokens'
    command = ['rustc', '--edition=2024', '-O', str(rust), '-L', f'dependency={args.deps.resolve()}', '-o', str(binary)]
    for name, path in libraries.items():
        command += ['--extern', f'{name}={path}']
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    (output / 'build.log').write_text(result.stdout + result.stderr)
    if result.returncode:
        raise RuntimeError('rustc 链接失败；请查 build.log 并使用同一依赖图的 --rlib；不会自动运行 Cargo')
    manifest.update(libraries=libraries, binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                    generated_sha256=hashlib.sha256(generated.encode()).hexdigest(),
                    fixture_sha256=hashlib.sha256((FIXTURES / 'lazy_tokens.rs').read_bytes()).hexdigest())
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2))


def measure(args):
    output = args.output.resolve()
    assert (output / 'manifest.json').is_file(), '先执行 --build-only'
    manifest = json.loads((output / 'manifest.json').read_text())
    verify_binary(output / 'lazy_tokens', manifest['binary_sha256'])
    results = []
    for round_index in range(args.rounds):
        for scenario in ['unused', 'required']:
            for variant in (['before', 'after'] if round_index % 2 == 0 else ['after', 'before']):
                command = [str(output / 'lazy_tokens'), variant, scenario, str(args.operations)]
                result = run_child(command, output / f'{scenario}-{variant}-{round_index}', args.timeout)
                result.update(component='lazy_tokens', variant=variant, scenario=scenario, round=round_index)
                results.append(result)
                print(json.dumps(result), flush=True)
                (output / 'results.json').write_text(json.dumps(results, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', default=DEFAULT_BASELINE)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--deps', type=Path, default=ROOT / 'codex-rs/target/debug/deps')
    parser.add_argument('--rlib', action='append', default=[], metavar='CRATE=PATH')
    parser.add_argument('--rounds', type=int, default=5)
    parser.add_argument('--operations', type=int, default=2000)
    parser.add_argument('--timeout', type=int, default=120)
    stage = parser.add_mutually_exclusive_group()
    stage.add_argument('--build-only', action='store_true')
    stage.add_argument('--run-only', action='store_true')
    args = parser.parse_args()
    if platform.system() not in ('Darwin', 'Linux'):
        parser.error('仅支持 macOS/Linux；Windows 实机未验证')
    if min(args.rounds, args.operations, args.timeout) < 1:
        parser.error('rounds/operations/timeout 必须为正数')
    if not args.run_only:
        build(args)
    if not args.build_only:
        measure(args)


if __name__ == '__main__':
    main()

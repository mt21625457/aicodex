#!/usr/bin/env python3
"""从指定基线/当前生产路径比较 shell 缓存候选；只复用已有依赖，不执行 Cargo。"""
import argparse
import hashlib
import itertools
import json
from pathlib import Path
import platform
import subprocess

from components import ROOT, FIXTURES, DEFAULT_BASELINE, git_source, run_child

SOURCE = 'codex-rs/core/src/client_common.rs'
VARIANTS = ('before', 'direct', 'bypass_large', 'read_only_large', 'after')
SCENARIOS = ('warm16', 'warm17', 'warm32', 'warm128', 'cold32', 'duplicate32', 'invalid32', 'small1000', 'mixed16')
IMPORTS = '''use codex_protocol::models::*;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
'''


def digest(path):
    sha = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            sha.update(chunk)
    return sha.hexdigest()


def replace_once(source, old, new):
    if source.count(old) != 1:
        raise RuntimeError(f'源码锚点不再唯一，请审核 probe 提取规则: {old}')
    return source.replace(old, new)


def section(source):
    return source[source.index('fn reserialize_shell_outputs('):source.index('fn normalize_image_detail(')]


def choices(deps, overrides, name):
    paths = ([Path(overrides[name]).resolve()] if name in overrides else
             sorted(deps.glob(f'lib{name}-*.rlib'), key=lambda p: p.stat().st_mtime_ns, reverse=True))
    if not paths or any(not p.is_file() for p in paths):
        raise RuntimeError(f'缺少 {name} rlib；先集中构建或指定 --rlib {name}=路径')
    return paths


def build(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    deps = args.deps.resolve()
    overrides = dict(value.split('=', 1) for value in args.rlib)
    candidates = {name: choices(deps, overrides, name) for name in
                  ('codex_protocol', 'serde_json', 'serde', 'codex_utils_cache', 'tokio', 'libc')}
    libraries = {name: values[0] for name, values in candidates.items()}
    errors = []

    def compile_text(name, text, selected, metadata=False):
        path, binary = output/(name+'.rs'), output/name
        path.write_text(text)
        command = ['rustc', '--edition=2024', '-O', str(path), '-L', f'dependency={deps}', '-o', str(binary)]
        if metadata:
            command.append('--emit=metadata')
        for crate, library in selected.items():
            command.extend(['--extern', f'{crate}={library}'])
        result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
        if result.returncode:
            errors.append(result.stderr)
            return None
        return binary

    # 先分别检查 trait 兼容，再链接实际 runtime 验证缓存可用。不同 feature 图即使能
    # 同时链接，也可能让缓存读取不到另一份 Tokio 的当前 runtime，不能仅依靠编译成功。
    protocol_probe = 'fn main(){let _:codex_protocol::models::ResponseItem=serde_json::from_str("{}").unwrap();}'
    for protocol, serde_json in itertools.product(candidates['codex_protocol'], candidates['serde_json']):
        if compile_text('link-protocol', protocol_probe, {'codex_protocol': protocol, 'serde_json': serde_json}, True):
            libraries.update(codex_protocol=protocol, serde_json=serde_json)
            break
    else:
        (output/'link-errors.log').write_text('\n'.join(errors))
        raise RuntimeError('protocol/serde_json 不兼容；见 link-errors.log')
    serde_probe = '#[derive(serde::Deserialize)] struct Probe { value: u32 } fn main(){let _:Probe=serde_json::from_str("{}").unwrap();}'
    for serde in candidates['serde']:
        if compile_text('link-serde', serde_probe, {'serde': serde, 'serde_json': libraries['serde_json']}, True):
            libraries['serde'] = serde
            break
    else:
        (output/'link-errors.log').write_text('\n'.join(errors))
        raise RuntimeError('serde/serde_json 不兼容；见 link-errors.log')
    runtime_probe = '''fn main(){let rt=tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();rt.block_on(async {let cache=codex_utils_cache::BlockingLruCache::new(std::num::NonZeroUsize::MIN);cache.insert("probe",1u8);assert_eq!(cache.get("probe"),Some(1));});}'''
    for cache, tokio in itertools.product(candidates['codex_utils_cache'], candidates['tokio']):
        binary = compile_text('link-runtime', runtime_probe, {'codex_utils_cache': cache, 'tokio': tokio})
        if binary and subprocess.run([str(binary)], capture_output=True).returncode == 0:
            libraries.update(codex_utils_cache=cache, tokio=tokio)
            break
    else:
        (output/'link-errors.log').write_text('\n'.join(errors))
        raise RuntimeError('cache/Tokio runtime 不兼容；指定一致依赖图的 --rlib')

    baseline = subprocess.check_output(['git', 'rev-parse', '--verify', f'{args.baseline}^{{commit}}'], cwd=ROOT, text=True).strip()
    before = git_source(baseline, SOURCE)
    after = (ROOT/SOURCE).read_text()
    manifest = {'baseline': baseline, 'platform': platform.platform(), 'root': str(ROOT),
                'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
                'sources': {'before:'+SOURCE: hashlib.sha256(before.encode()).hexdigest(),
                            'after:'+SOURCE: hashlib.sha256(after.encode()).hexdigest()},
                'libraries': {name: {'path': str(path), 'sha256': digest(path)} for name, path in libraries.items()},
                'binaries': {}, 'variants': list(VARIANTS), 'scenarios': list(SCENARIOS)}
    # Oracle 保留基线完整配对/格式化，只关闭缓存以避免它污染目标进程缓存状态。
    reference = replace_once(section(before), 'let cacheable =', 'let cacheable = false &&')
    reference = IMPORTS+reference+'\npub fn convert(items: &mut [ResponseItem]) { reserialize_shell_outputs(items); }\n'
    for variant in VARIANTS:
        value = section(after if variant == 'after' else before)
        policy = ''
        convert_prefix = ''
        if variant == 'direct':
            value = replace_once(value, 'let cacheable =', 'let cacheable = false &&')
        elif variant in ('bypass_large', 'read_only_large'):
            policy = 'thread_local! { static POLICY: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }\n'
            value = replace_once(value, 'let cacheable =', 'let cacheable = POLICY.with(|policy| policy.get()) != 1 &&')
            value = replace_once(value, 'if cacheable\n        && result', 'if cacheable && POLICY.with(|policy| policy.get()) != 2\n        && result')
            mode = 1 if variant == 'bypass_large' else 2
            convert_prefix = f'''let large = items.iter().filter_map(|item| match item {{ ResponseItem::FunctionCallOutput {{ output, .. }} | ResponseItem::CustomToolCallOutput {{ output, .. }} => output.text_content(), _ => None }}).filter(|raw| (4096..=1024*1024).contains(&raw.len())).take(17).count()>16;
                POLICY.with(|policy| policy.set(if large {{ {mode} }} else {{ 0 }}));'''
        value = replace_once(value, 'let result = serde_json::from_str',
                             'PARSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n    let result = serde_json::from_str')
        warm = ('let mut budget=usize::MAX; for raw in raws { parse_structured_shell_output_with_cache(raw,Some(&SHELL_OUTPUT_CACHE),&mut budget); }'
                if variant == 'after' else 'for raw in raws { parse_structured_shell_output(raw); }')
        interface = f'''pub fn convert(items: &mut [ResponseItem]) {{ {convert_prefix} reserialize_shell_outputs(items); }}
            pub fn clear_cache() {{ SHELL_OUTPUT_CACHE.clear(); }}
            pub fn warm_cache(items: &[ResponseItem]) {{ let raws=items.iter().filter_map(|item| match item {{ResponseItem::FunctionCallOutput {{ output, .. }} => output.text_content(), _ => None }}); {warm} }}
            pub fn reset_parses() {{ PARSES.store(0,std::sync::atomic::Ordering::Relaxed); }}
            pub fn parses() -> usize {{ PARSES.load(std::sync::atomic::Ordering::Relaxed) }}'''
        target = IMPORTS+policy+'static PARSES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);\n'+value+interface
        text = ((FIXTURES/'metrics.rs').read_text()+f'\nconst VARIANT: &str = "{variant}";\n'+
                'mod reference {\n'+reference+'}\nmod target {\n'+target+'}\n'+(FIXTURES/'shell_cache.rs').read_text())
        binary = compile_text('shell-'+variant, text, libraries)
        if not binary:
            (output/'link-errors.log').write_text('\n'.join(errors))
            raise RuntimeError(f'无法编译 {variant}；见 link-errors.log')
        manifest['binaries'][variant] = {'path': str(binary), 'sha256': digest(binary), 'probe_sha256': digest(output/('shell-'+variant+'.rs'))}
    manifest['fixtures'] = {name: digest(FIXTURES/name) for name in ('metrics.rs', 'shell_cache.rs')}
    manifest['runner_sha256'] = digest(__file__)
    manifest['components_runner_sha256'] = digest(Path(__file__).with_name('components.py'))
    (output/'manifest.json').write_text(json.dumps(manifest, indent=2))
    print(f'已构建并验证 runtime 链接；未执行 timing: {output}')


def measure(args):
    output = args.output.resolve()
    manifest = json.loads((output/'manifest.json').read_text())
    for variant in args.variants:
        entry = manifest['binaries'][variant]
        if digest(entry['path']) != entry['sha256']:
            raise RuntimeError(f'二进制 hash 改变: {variant}')
    results = []
    for index in range(args.rounds):
        order = args.variants if index % 2 == 0 else list(reversed(args.variants))
        for scenario in args.scenarios:
            for variant in order:
                result = run_child([manifest['binaries'][variant]['path'], scenario, str(args.iterations)],
                                   output/f'{variant}-{scenario}-{index}', args.timeout)
                result.update(variant=variant, scenario=scenario, round=index)
                results.append(result)
                print(json.dumps(result), flush=True)
            (output/'results.json').write_text(json.dumps(results, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', default=DEFAULT_BASELINE)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--deps', type=Path, default=ROOT/'codex-rs/target/debug/deps')
    parser.add_argument('--rlib', action='append', default=[], metavar='CRATE=PATH')
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--iterations', type=int, default=400)
    parser.add_argument('--timeout', type=int, default=120)
    parser.add_argument('--variants', nargs='+', choices=VARIANTS, default=list(VARIANTS))
    parser.add_argument('--scenarios', nargs='+', choices=SCENARIOS, default=list(SCENARIOS))
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--build-only', action='store_true')
    mode.add_argument('--run-only', action='store_true')
    args = parser.parse_args()
    if platform.system() not in ('Darwin', 'Linux'):
        parser.error('只支持macOS/Linux；Windows指标未验收')
    if min(args.rounds, args.iterations, args.timeout) < 1:
        parser.error('rounds/iterations/timeout必须为正数')
    if not args.run_only:
        build(args)
    if not args.build_only:
        measure(args)


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""从指定 Git 基线与当前源码生成真实组件探针；不执行 Cargo 或访问远端服务。"""
import argparse
import hashlib
import itertools
import json
import os
from pathlib import Path
import platform
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = Path(__file__).resolve().parent / 'fixtures'
DEFAULT_BASELINE = 'daf61e7a3d08c004cce53df531094a69795da819'


def git_source(revision, path):
    return subprocess.check_output(['git', 'show', f'{revision}:{path}'], cwd=ROOT).decode()


def quoted(path):
    # JSON 字符串转义也是这里所需的 Rust 普通字符串转义；不拼入 shell 命令。
    return json.dumps(str(path), ensure_ascii=False)


def verify_binary(path, expected_sha256):
    # run-only 可能复用旧输出目录；必须校验实际执行文件，不能只相信 manifest 存在。
    digest = hashlib.sha256()
    with Path(path).open('rb') as binary:
        for chunk in iter(lambda: binary.read(1024 * 1024), b''):
            digest.update(chunk)
    if digest.hexdigest() != expected_sha256:
        raise RuntimeError(f'二进制 hash 改变，请重新构建探针: {path}')


def build(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    baseline = subprocess.check_output(
        ['git', 'rev-parse', '--verify', f'{args.baseline}^{{commit}}'], cwd=ROOT, text=True).strip()
    deps = args.deps.resolve()
    overrides = dict(value.split('=', 1) for value in args.rlib)
    libraries = {}
    for name in ['libc', 'tempfile', 'codex_protocol', 'serde_json']:
        candidates = list(deps.glob(f'lib{name}-*.rlib'))
        if name in overrides:
            path = Path(overrides[name]).resolve()
        elif candidates:
            path = max(candidates, key=lambda p: p.stat().st_mtime_ns)
        else:
            raise RuntimeError(f'缺少 {name} rlib；先集中构建相关 crate，或使用 --rlib {name}=路径')
        if not path.is_file():
            raise RuntimeError(f'rlib 不存在: {path}')
        libraries[name] = str(path)
    manifest = {'baseline': baseline, 'root': str(ROOT), 'platform': platform.platform(),
                'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
                'libraries': libraries, 'sources': {}, 'binaries': {}}
    def source(path, variant):
        value = git_source(baseline, path) if variant == 'before' else (ROOT/path).read_text()
        manifest['sources'][f'{variant}:{path}'] = hashlib.sha256(value.encode()).hexdigest()
        return value
    metrics = (FIXTURES/'metrics.rs').read_text()
    def compile_probe(name, text, needed):
        rust = output/(name+'.rs')
        rust.write_text(text)
        binary = output/name
        command = ['rustc', '--edition=2024', '-O', str(rust), '-L', f'dependency={deps}', '-o', str(binary)]
        for dep in needed:
            command.extend(['--extern', f'{dep}={libraries[dep]}'])
        if name == 'catalog':
            # 同一 target 可能保留不同 feature 图的 serde 产物。通过真实 trait 编译匹配，
            # 不因文件较新就把两个不兼容的依赖图混接；显式 --rlib 始终优先。
            choices = {}
            for dep in ('codex_protocol', 'serde_json'):
                choices[dep] = ([Path(libraries[dep])] if dep in overrides else
                    sorted(deps.glob(f'lib{dep}-*.rlib'), key=lambda p: p.stat().st_mtime_ns, reverse=True))
            errors = []
            for protocol, serde_json in itertools.product(choices['codex_protocol'], choices['serde_json']):
                trial = list(command)
                for dep, path in [('codex_protocol', protocol), ('serde_json', serde_json)]:
                    trial[trial.index(f'{dep}={libraries[dep]}')] = f'{dep}={path}'
                result = subprocess.run(trial, cwd=ROOT, text=True, capture_output=True)
                if result.returncode == 0:
                    libraries.update(codex_protocol=str(protocol), serde_json=str(serde_json))
                    break
                errors.append(result.stderr)
            else:
                (output/'catalog-link-errors.log').write_text('\n'.join(errors))
                raise RuntimeError('找不到兼容的 protocol/serde_json 产物；见 catalog-link-errors.log 或指定 --rlib')
        else:
            subprocess.run(command, cwd=ROOT, check=True)
        manifest['binaries'][name] = {'path': str(binary), 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
    catalog = []
    for variant in ['before', 'after']:
        value = source('codex-rs/models-manager/src/lib.rs', variant)
        start = value.index('/// Load the bundled')
        end = value.index('/// Convert the client version')
        model_json = output/(variant+'-models.json')
        model_json.write_text(source('codex-rs/models-manager/models.json', variant))
        value = value[start:end].replace('include_str!("../models.json")', f'include_str!({quoted(model_json)})')
        catalog.append(f'mod {variant} {{\n{value}\n}}\n')
    compile_probe('catalog', metrics+'\n'+''.join(catalog)+(FIXTURES/'catalog.rs').read_text(),
                  ['libc', 'codex_protocol', 'serde_json'])
    for variant in ['before', 'after']:
        value = source('codex-rs/tui/src/inline_visualization/viewer.rs', variant)
        for asset in ['visualize.css', 'visualize.html']:
            target = output/(variant+'-'+asset)
            target.write_text(source('codex-rs/tui/assets/inline_visualization/'+asset, variant))
            value = value.replace(f'include_str!("../../assets/inline_visualization/{asset}")',
                                  f'include_str!({quoted(target)})')
        viewer = output/(variant+'-viewer-source.rs')
        viewer.write_text(value)
        compile_probe('viewer-'+variant, metrics+f'\n#[path={quoted(viewer)}] mod viewer;\n'+
                      (FIXTURES/'viewer.rs').read_text(), ['libc', 'tempfile'])
        history = output/(variant+'-history-source.rs')
        history.write_text(source('codex-rs/message-history/src/lookup_index.rs', variant))
        compile_probe('history-'+variant, metrics+f'\n#[path={quoted(history)}] mod history;\n'+
                      (FIXTURES/'history.rs').read_text(), ['libc'])
    manifest['fixtures'] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in FIXTURES.glob('*.rs')}
    (output/'manifest.json').write_text(json.dumps(manifest, indent=2))


def run_child(command, stem, timeout):
    with stem.with_suffix('.stdout').open('w') as stdout, stem.with_suffix('.stderr').open('w') as stderr:
        proc = subprocess.Popen(command, stdout=stdout, stderr=stderr)
        try:
            deadline = time.monotonic()+timeout
            while True:
                pid, status, usage = os.wait4(proc.pid, os.WNOHANG)
                if pid:
                    proc.returncode = os.waitstatus_to_exitcode(status)
                    break
                if time.monotonic() > deadline:
                    raise RuntimeError(f'组件超时，已终止且不记录性能样本: {command}')
                time.sleep(.01)
        except BaseException:
            # Ctrl-C 和读取/等待异常同样必须回收探针，避免中断后仍消耗 CPU。
            if proc.returncode is None:
                try:
                    proc.kill()
                except ProcessLookupError:
                    pass
                try:
                    _, status, _ = os.wait4(proc.pid, 0)
                    proc.returncode = os.waitstatus_to_exitcode(status)
                except ChildProcessError:
                    pass
            raise
        if proc.returncode:
            raise RuntimeError(f'组件失败 {proc.returncode}: {stem.with_suffix(".stderr")}')
    result = json.loads(stem.with_suffix('.stdout').read_text())
    result.update(process_user_s=usage.ru_utime, process_system_s=usage.ru_stime,
                  process_max_rss_bytes=usage.ru_maxrss*(1 if platform.system()=='Darwin' else 1024))
    return result


def measure(args):
    output = args.output.resolve()
    if not (output/'manifest.json').is_file():
        raise RuntimeError('先执行构建阶段，生成 manifest.json')
    manifest = json.loads((output/'manifest.json').read_text())
    for name in ['catalog', 'viewer-before', 'viewer-after', 'history-before', 'history-after']:
        verify_binary(output/name, manifest['binaries'][name]['sha256'])
    results = []
    for index in range(args.rounds):
        order = ['before', 'after'] if index % 2 == 0 else ['after', 'before']
        for component in ['catalog', 'viewer', 'history']:
            for variant in order:
                if component == 'catalog':
                    command = [str(output/'catalog'), variant]
                elif component == 'viewer':
                    command = [str(output/('viewer-'+variant)), str(output/(variant+'-viewer-output.html'))]
                else:
                    command = [str(output/('history-'+variant)), str(output/(variant+'-history-input'))]
                result = run_child(command, output/f'{component}-{variant}-{index}', args.timeout)
                result.update(component=component, variant=variant, round=index)
                results.append(result)
                print(json.dumps(result), flush=True)
            if component == 'viewer':
                assert (output/'before-viewer-output.html').read_bytes() == (output/'after-viewer-output.html').read_bytes(), '两版viewer生成字节不同'
            (output/'results.json').write_text(json.dumps(results, indent=2))
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', default=DEFAULT_BASELINE)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--deps', type=Path, default=ROOT/'codex-rs/target/debug/deps')
    parser.add_argument('--rlib', action='append', default=[], metavar='CRATE=PATH')
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--timeout', type=int, default=120)
    stage = parser.add_mutually_exclusive_group()
    stage.add_argument('--build-only', action='store_true')
    stage.add_argument('--run-only', action='store_true')
    args = parser.parse_args()
    if platform.system() not in ('Darwin', 'Linux'):
        parser.error('本探针只支持 macOS/Linux；Windows 指标尚未验证')
    if args.rounds < 1 or args.timeout < 1:
        parser.error('rounds 和 timeout 必须为正数')
    if not args.run_only:
        build(args)
    if not args.build_only:
        measure(args)


if __name__ == '__main__':
    main()

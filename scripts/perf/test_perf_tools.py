#!/usr/bin/env python3
"""验证性能工具的证据发布与进程清理；不构建 Cargo，不运行性能负载。"""
import copy
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

import components
import lazy_tokens
import local_app_server as app


class BinaryManifestTests(unittest.TestCase):
    def test_matching_binary_hash_is_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory)/'probe'
            binary.write_bytes(b'original binary')
            components.verify_binary(binary, hashlib.sha256(binary.read_bytes()).hexdigest())

    def test_components_reject_changed_binary_before_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            names = ['catalog', 'viewer-before', 'viewer-after', 'history-before', 'history-after']
            expected = hashlib.sha256(b'original binary').hexdigest()
            for name in names:
                (output/name).write_bytes(b'original binary')
            (output/'manifest.json').write_text(json.dumps({'binaries': {
                name: {'path': str(output/name), 'sha256': expected} for name in names
            }}))
            # 目录中一份探针被重建，不能用旧 manifest 为它背书。
            (output/'history-after').write_bytes(b'rebuilt binary')
            with mock.patch.object(components, 'run_child') as run_child:
                with self.assertRaisesRegex(RuntimeError, 'hash'):
                    components.measure(SimpleNamespace(output=output, rounds=1, timeout=1))
            run_child.assert_not_called()
            self.assertFalse((output/'results.json').exists())

    def test_lazy_rejects_changed_binary_before_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output/'manifest.json').write_text(json.dumps({
                'binary_sha256': hashlib.sha256(b'original binary').hexdigest(),
            }))
            (output/'lazy_tokens').write_bytes(b'rebuilt binary')
            with mock.patch.object(lazy_tokens, 'run_child') as run_child:
                with self.assertRaisesRegex(RuntimeError, 'hash'):
                    lazy_tokens.measure(SimpleNamespace(output=output, rounds=1, operations=1, timeout=1))
            run_child.assert_not_called()
            self.assertFalse((output/'results.json').exists())


@unittest.skipUnless(hasattr(os, 'wait4'), 'probe runner only supports macOS/Linux')
class ChildProcessTests(unittest.TestCase):
    def check_reaped_on_error(self, interrupt):
        children = []
        create_process = subprocess.Popen

        def launch(*args, **kwargs):
            process = create_process(*args, **kwargs)
            children.append(process)
            return process

        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(components.subprocess, 'Popen', side_effect=launch):
                try:
                    if interrupt:
                        with mock.patch.object(components.time, 'sleep', side_effect=KeyboardInterrupt):
                            with self.assertRaises(KeyboardInterrupt):
                                components.run_child(
                                    [sys.executable, '-c', 'import time; time.sleep(60)'],
                                    Path(directory)/'interrupted', 120,
                                )
                    else:
                        with mock.patch.object(components.time, 'monotonic', side_effect=[0, 2]):
                            with self.assertRaisesRegex(RuntimeError, '超时'):
                                components.run_child(
                                    [sys.executable, '-c', 'import time; time.sleep(60)'],
                                    Path(directory)/'timed-out', 1,
                                )
                    self.assertEqual(len(children), 1)
                    self.assertLess(children[0].returncode, 0)
                    with self.assertRaises(ChildProcessError):
                        os.wait4(children[0].pid, os.WNOHANG)
                finally:
                    # 回归失败时也不遗留测试自己的 sleeper。
                    for child in children:
                        if child.returncode is None:
                            child.kill()
                            child.wait()

    def test_interrupt_kills_and_reaps_child(self):
        self.check_reaped_on_error(interrupt=True)

    def test_timeout_kills_and_reaps_child(self):
        self.check_reaped_on_error(interrupt=False)

    def test_success_still_returns_child_metrics(self):
        with tempfile.TemporaryDirectory() as directory:
            result = components.run_child(
                [sys.executable, '-c', 'print(\'{"operations": 1}\')'],
                Path(directory)/'success', 10,
            )
        self.assertEqual(result['operations'], 1)
        self.assertGreaterEqual(result['process_user_s'], 0)
        self.assertGreaterEqual(result['process_system_s'], 0)
        self.assertGreater(result['process_max_rss_bytes'], 0)


class ReceiveTests(unittest.TestCase):
    def test_notifications_cannot_extend_absolute_deadline(self):
        messages = queue.Queue()
        notifications = []
        messages.put({'method': 'unrelated-1'})
        messages.put({'method': 'unrelated-2'})
        # 队列始终非空；第二次读取前已经超过截止时间，不能继续取通知。
        with mock.patch.object(app.time, 'monotonic', side_effect=[0, 0, 46]):
            with self.assertRaises(TimeoutError):
                app.receive_message(messages, lambda message: message.get('id') == 7, notifications)
        self.assertEqual(notifications, [{'method': 'unrelated-1'}])
        self.assertEqual(messages.get_nowait(), {'method': 'unrelated-2'})

    def test_target_response_and_prior_notifications_are_preserved(self):
        messages = queue.Queue()
        messages.put({'method': 'unrelated'})
        response = {'id': 7, 'result': {'value': 3}}
        messages.put(response)
        notifications = []
        with mock.patch.object(app.time, 'monotonic', side_effect=[0, 0, 1]):
            self.assertEqual(app.receive_message(messages, lambda message: message.get('id') == 7,
                                                notifications), response)
        self.assertEqual(notifications, [{'method': 'unrelated'}])

    def test_empty_queue_timeout_remains_an_error(self):
        messages = mock.Mock()
        messages.get.side_effect = queue.Empty
        with mock.patch.object(app.time, 'monotonic', side_effect=[0, 0]):
            with self.assertRaises(TimeoutError):
                app.receive_message(messages, lambda _: True, [])


class EvidencePublicationTests(unittest.TestCase):
    def test_empty_or_negative_workload_is_rejected_before_running(self):
        with tempfile.TemporaryDirectory() as directory:
            for option, value in [('--rounds', '0'), ('--turns', '0'), ('--text-bytes', '-1')]:
                with self.subTest(option=option):
                    argv = ['local_app_server.py', '--before', '/unused/before', '--after', '/unused/after',
                            '--output', directory, option, value]
                    with mock.patch.object(sys, 'argv', argv), mock.patch.object(app, 'run') as run:
                        with mock.patch('sys.stderr'), self.assertRaises(SystemExit) as error:
                            app.main()
                    self.assertEqual(error.exception.code, 2)
                    run.assert_not_called()
                    self.assertFalse((Path(directory)/'results.json').exists())

    @staticmethod
    def pair():
        sample = {'normalized_request_sha256': ['request'],
                  'request_normalization_paths': ['generated-id'],
                  'assistant_sha256': 'answer'}
        return [copy.deepcopy(sample), copy.deepcopy(sample)]

    def test_each_equivalence_failure_leaves_success_results_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)/'results.json'
            previous = [{'previous': 'validated pair'}]
            original = json.dumps(previous)
            for key in self.pair()[0]:
                with self.subTest(key=key):
                    output.write_text(original)
                    results = copy.deepcopy(previous)
                    pair = self.pair()
                    pair[1][key] = 'different'
                    with self.assertRaises(AssertionError):
                        app.publish_pair(results, pair, output)
                    self.assertEqual(output.read_text(), original)
                    self.assertEqual(results, previous)

    def test_equivalent_pair_is_published_together(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)/'results.json'
            results = []
            pair = self.pair()
            app.publish_pair(results, pair, output)
            self.assertEqual(results, pair)
            self.assertEqual(json.loads(output.read_text()), pair)

    def test_failed_first_pair_clears_previous_run_results(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)/'results.json'
            output.write_text('[{"stale": true}]')
            pair = self.pair()
            pair[1]['assistant_sha256'] = 'different'
            argv = ['local_app_server.py', '--before', '/unused/before', '--after', '/unused/after',
                    '--output', directory, '--rounds', '1']
            with mock.patch.object(sys, 'argv', argv), mock.patch.object(app, 'run', side_effect=pair):
                with mock.patch('builtins.print'), self.assertRaises(AssertionError):
                    app.main()
            self.assertEqual(json.loads(output.read_text()), [])

    def test_spawn_failure_closes_fixture_server_and_worker(self):
        servers, workers = [], []
        create_server = app.http.server.ThreadingHTTPServer
        create_worker = app.threading.Thread

        def server(*args, **kwargs):
            value = create_server(*args, **kwargs)
            servers.append(value)
            return value

        def worker(*args, **kwargs):
            value = create_worker(*args, **kwargs)
            workers.append(value)
            return value

        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(app.http.server, 'ThreadingHTTPServer', side_effect=server), \
                    mock.patch.object(app.threading, 'Thread', side_effect=worker), \
                    mock.patch.object(app.subprocess, 'Popen', side_effect=FileNotFoundError):
                with self.assertRaises(FileNotFoundError):
                    app.run(Path('/missing/aicodex'), 1, 1, Path(directory), 'missing')
        self.assertEqual(len(servers), 1)
        self.assertEqual(servers[0].socket.fileno(), -1)
        self.assertEqual(len(workers), 1)
        self.assertFalse(workers[0].is_alive())


if __name__ == '__main__':
    unittest.main()

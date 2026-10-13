#!/usr/bin/python3
"""Transparent test-only proxy for explicit fake_codex; never discovers a real program.
Capture only generated session/token-file paths from production MCP launch configuration.
"""
import json, os, pathlib, signal, subprocess, sys, threading
real = pathlib.Path(os.environ['FAKE_CODEX_QA_REAL_PROGRAM'])
if not real.is_absolute() or real.name != 'fake_codex':
    raise RuntimeError('explicit absolute fake_codex required')
output = pathlib.Path(os.environ['FAKE_CODEX_QA_CAPTURE_DIR'])
if not output.is_absolute() or not output.is_dir():
    raise RuntimeError('explicit owned fixture capture directory required')
child = subprocess.Popen([str(real), *sys.argv[1:]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr)
def stop(*_):
    if child.poll() is None:
        child.terminate()
        try: child.wait(timeout=5)
        except subprocess.TimeoutExpired: child.kill(); child.wait()
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
def read_output():
    for line in child.stdout:
        sys.stdout.buffer.write(line); sys.stdout.buffer.flush()
relay = threading.Thread(target=read_output, daemon=True); relay.start()
try:
    for line in sys.stdin.buffer:
        value = json.loads(line)
        if value.get('method') in ['thread/start', 'thread/resume']:
            config = value.get('params', {}).get('config', {}).get('mcp_servers', {}).get('crew')
            if config:
                args = config['args']
                session = args[args.index('--session') + 1]
                token_file = args[args.index('--token-file') + 1]
                if not all(c.isalnum() or c in '-_' for c in session): raise RuntimeError('fixture session identity')
                temp = output / (session + '.tmp')
                descriptor = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                with os.fdopen(descriptor, 'w') as capture:
                    json.dump({'session': session, 'tokenFile': token_file}, capture)
                os.replace(temp, output / (session + '.json'))
        child.stdin.write(line); child.stdin.flush()
finally:
    if child.poll() is None:
        child.stdin.close()
        try: child.wait(timeout=5)
        except subprocess.TimeoutExpired: child.kill(); child.wait()
    relay.join(timeout=5)

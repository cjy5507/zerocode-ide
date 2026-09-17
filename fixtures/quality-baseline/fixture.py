"""File/process/ledger judges. No verdict ever comes from assistant prose.

ROOT contains repo/, evidence/, and (Q4) worktree/. Evidence is written by the
harness observing tools, requests and CLI receipts, never by parsing a final answer.
"""
import json
import shutil
import subprocess
import sys
from pathlib import Path

BASE = Path(__file__).resolve().parent
POLICY = json.loads((BASE / 'baseline.json').read_text())
TASKS = POLICY['Tasks']

def run(args, cwd, check=True):
    return subprocess.run(args, cwd=cwd, check=check, capture_output=True, text=True)

def git(repo, *args):
    return run(['git', *args], repo).stdout.strip()

def prepare(root):
    root.mkdir(parents=True, exist_ok=True)
    repo = root / 'repo'
    shutil.copytree(BASE / 'repo', repo, ignore=shutil.ignore_patterns('target'))
    (root / 'evidence').mkdir()
    git(repo, 'init', '-b', 'main')
    git(repo, 'config', 'user.name', 'Baseline')
    git(repo, 'config', 'user.email', 'baseline@invalid')
    git(repo, 'config', 'commit.gpgsign', 'false')
    git(repo, 'add', '.')
    git(repo, 'commit', '-m', 'Planted baseline defects')
    (root / 'base-sha').write_text(git(repo, 'rev-parse', 'HEAD'))

def answer(task, root):
    repo = root / 'repo'
    if task == 'Q4':
        git(repo, 'worktree', 'add', '-b', 'baseline-fix', str(root / 'worktree'))
        repo = root / 'worktree'
    git(repo, 'apply', str(BASE / 'answers' / f'{task}.patch'))
    if task == 'Q4':
        git(repo, 'add', 'src/lib.rs')
        git(repo, 'commit', '-m', 'Fix inclusive range')

def write(root, name, value):
    (root / 'evidence' / name).write_text(json.dumps(value))

def self_test_evidence(task, root):
    """Explicit synthetic evidence ONLY for unit-testing the judges."""
    if task == 'Q2':
        write(root, 'delegation.json', {'receipts':[{'id':'a','status':'completed','path':'js/alpha.mjs'}, {'id':'b','status':'completed','path':'js/beta.mjs'}], 'parent_tool_results':['a','b']})
    if task == 'Q3':
        write(root, 'resume.json', {'interrupted':True,'resumed':True,'session_id':'self-test','original_prompt':'BASELINE_CONTINUITY', 'resumed_messages':[{'role':'user','content':'BASELINE_CONTINUITY'}]})
    if task == 'Q3':
        write(root, 'window-resume.json', {'id':'self-test','interrupted':True,'restarted':True,'calm_interrupted':False})
    if task == 'Q5':
        write(root, 'browser.json', {'dom':'Build verified','command':'zerocode-browser read'})
        # Self-test uses a valid tiny PNG; scenario evidence comes from Chromium.
        import base64
        (root / 'evidence' / 'page.png').write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jN1kAAAAASUVORK5CYII='))
    if task == 'Q6':
        write(root, 'ledger.json', {'task':'Q6','result':{'ok':True,'files':['js/handoff.mjs'],'summary':'Updated handoff status'},'receipts':[{'type':'worker_done','body':{'ok':True}}], 'commands':['send','task-update']})

def changed(root, repo):
    base = (root / 'base-sha').read_text()
    tracked = git(repo, 'diff', '--name-only', base).splitlines()
    untracked = git(repo, 'ls-files', '--others', '--exclude-standard').splitlines()
    return sorted(set(tracked + untracked))

def result_valid(value):
    return (isinstance(value, dict) and set(value) == {'ok','files','summary'}
            and value['ok'] is True and isinstance(value['summary'], str) and bool(value['summary'])
            and isinstance(value['files'], list) and all(isinstance(x,str) for x in value['files'])
            and len(value['files']) == len(set(value['files'])))

def verify(task, root):
    try:
        main = root / 'repo'
        repo = root / 'worktree' if task == 'Q4' else main
        paths = changed(root, repo)
        if paths != sorted(TASKS[task]['allowed']): return False
        if task in ('Q1','Q3','Q4'):
            if run(['cargo','test','--offline','-p','fixture'], repo, False).returncode: return False
        if task == 'Q2':
            code = "import {double} from './js/alpha.mjs'; import {isEven} from './js/beta.mjs'; if (![0,1,7,-4].every(n=>double(n)===n*2 && isEven(n)===(n%2===0))) process.exit(1);"
            if run(['node','--input-type=module','-e',code], repo, False).returncode: return False
            data = json.loads((root/'evidence/delegation.json').read_text())
            receipts = data['receipts']
            if len(receipts) != 2 or len({x['id'] for x in receipts}) != 2: return False
            if sorted(x['path'] for x in receipts) != paths: return False
            if any(x['status'] != 'completed' or x['id'] not in data['parent_tool_results'] for x in receipts): return False
        if task == 'Q3':
            data = json.loads((root/'evidence/resume.json').read_text())
            if data['interrupted'] is not True or data['resumed'] is not True: return False
            restore = json.loads((root/'evidence/window-resume.json').read_text())
            if restore['id'] != data['session_id'] or restore['interrupted'] is not True or restore['restarted'] is not True or restore['calm_interrupted'] is not False: return False
            if not data['original_prompt'] or not any(m['role']=='user' and data['original_prompt'] in json.dumps(m['content']) for m in data['resumed_messages']): return False
        if task == 'Q4':
            if git(main,'status','--porcelain') or git(main,'rev-parse','HEAD') != (root/'base-sha').read_text(): return False
            if git(repo,'branch','--show-current') != 'baseline-fix' or git(repo,'status','--porcelain'): return False
            if git(repo,'rev-parse','HEAD') == git(main,'rev-parse','HEAD'): return False
        if task == 'Q5':
            data=json.loads((root/'evidence/browser.json').read_text())
            png=(root/'evidence/page.png').read_bytes()
            if data['dom'] != 'Build verified' or data['command'] != 'zerocode-browser read': return False
            if not png.startswith(b'\x89PNG\r\n\x1a\n') or b'IEND' not in png: return False
            if 'Build verified' not in (repo/'js/page.html').read_text(): return False
        if task == 'Q6':
            data=json.loads((root/'evidence/ledger.json').read_text())
            if data['task'] != task or not result_valid(data['result']) or sorted(data['result']['files']) != paths: return False
            if data['commands'] != ['send','task-update']: return False
            if len(data['receipts']) != 1 or data['receipts'][0] != {'type':'worker_done','body':{'ok':True}}: return False
            if run(['node','--input-type=module','-e',"import {status} from './js/handoff.mjs'; if(status!=='verified')process.exit(1)"],repo,False).returncode: return False
        return True
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError):
        return False

if __name__ == '__main__':
    action, *args = sys.argv[1:]
    if action == 'prepare': prepare(Path(args[0]).resolve())
    elif action == 'answer': answer(args[0], Path(args[1]).resolve())
    elif action == 'verify': sys.exit(0 if verify(args[0], Path(args[1]).resolve()) else 1)
    else: raise SystemExit('unknown action')

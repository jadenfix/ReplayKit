#!/usr/bin/env python3
"""A small scripted coding agent that records itself into ReplayKit.

    python3 agent.py make-repo WORKSPACE          # throwaway git repo with a bug
    python3 agent.py run --collector URL --workspace WORKSPACE

`run` tries to fix a login-timeout bug in WORKSPACE and posts every step to a
replaykit-collector: 15 spans (planner steps, file reads, tool calls, shell
commands, an LlmCall answered by fixer.py, a file write), DataDependsOn edges,
and outputs with content. It declares WORKSPACE as the run's workspace_root,
so the collector snapshots the workspace at every span boundary and forks
replay hermetically. Inputs that derive from earlier outputs are recorded as
input bindings (e.g. the test command is "python3 run_tests.py
{{select_tests|words}}"), so patched upstream outputs reach replayed steps.

The recorded fix only works when AUTH_POOL_TIMEOUT is set, so the validation
run still has one failing test and the run fails. Prints the run id.

Standard library only (Python 3.8+).
"""

import argparse
import base64
import hashlib
import json
import os
import subprocess
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))

# ---------------------------------------------------------------------------
# The throwaway repository
# ---------------------------------------------------------------------------

FILES = {
    "auth/__init__.py": "",
    "auth/errors.py": '''class AuthTimeout(TimeoutError):
    """Login gave up waiting for a pooled connection."""
''',
    "auth/pool.py": '''class Pool:
    """A connection pool. acquire() with no timeout on an empty pool would block forever."""

    def __init__(self, size):
        self.free = list(range(size))

    def acquire(self, timeout=None):
        if self.free:
            return self.free.pop()
        if timeout is None:
            raise RuntimeError("deadlock: acquire() blocked forever on an empty pool")
        raise TimeoutError("no connection within %ss" % timeout)
''',
    "auth/session.py": '''def init_session(pool):
    conn = pool.acquire()
    return {"conn": conn}
''',
    "auth/login.py": '''from auth.errors import AuthTimeout
from auth.session import init_session


def validate_credentials(user, password):
    return bool(user) and password == "hunter2"


def handle_login(pool, user, password):
    if not validate_credentials(user, password):
        raise PermissionError("invalid credentials")
    try:
        return init_session(pool)
    except TimeoutError as exc:
        raise AuthTimeout(str(exc)) from exc
''',
    "tests/test_login.py": '''from auth.errors import AuthTimeout
from auth.login import handle_login, validate_credentials
from auth.pool import Pool


def test_login_basic():
    assert handle_login(Pool(1), "ada", "hunter2") == {"conn": 0}


def test_invalid_credentials():
    try:
        handle_login(Pool(1), "ada", "wrong")
    except PermissionError:
        return
    raise AssertionError("expected PermissionError")


def test_validate_credentials():
    assert validate_credentials("ada", "hunter2")
    assert not validate_credentials("", "hunter2")


def test_login_timeout():
    try:
        handle_login(Pool(0), "ada", "hunter2")
    except AuthTimeout:
        return
    raise AssertionError("expected AuthTimeout")
''',
    "tests/test_session.py": '''from auth.pool import Pool
from auth.session import init_session


def test_init_session():
    assert init_session(Pool(1)) == {"conn": 0}


def test_session_timeout():
    try:
        init_session(Pool(0), timeout=0.01)
    except TimeoutError:
        return
    raise AssertionError("expected TimeoutError")
''',
    "run_tests.py": '''"""Tiny deterministic test runner: python3 run_tests.py FILE..."""
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.getcwd())


def main(paths):
    passed, failed = 0, []
    for path in paths:
        spec = importlib.util.spec_from_file_location(path[:-3].replace("/", "."), path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for name in sorted(n for n in dir(module) if n.startswith("test_")):
            try:
                getattr(module, name)()
            except Exception as exc:  # noqa: BLE001
                failed.append((name, "%s: %s" % (type(exc).__name__, exc)))
                print("%s::%s ... FAILED" % (path, name))
            else:
                passed += 1
                print("%s::%s ... ok" % (path, name))
    for name, why in failed:
        sys.stderr.write("FAILED %s: %s\\n" % (name, why))
    if failed:
        print("result: FAILED. %d passed; %d failed" % (passed, len(failed)))
        return 1
    print("result: OK. %d passed" % passed)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
''',
    "ISSUE.md": '''# Login hangs when the pool is exhausted

Since the session refactor, `handle_login` never returns when every connection is taken.
Expected: give up after a timeout and raise AuthTimeout.
''',
}

COMMITS = [
    ("auth service: pool, session, login", ["auth/__init__.py", "auth/errors.py", "auth/pool.py",
                                            "auth/session.py", "auth/login.py", "run_tests.py"]),
    ("tests: login and session timeouts", ["tests/test_login.py", "tests/test_session.py"]),
    ("issue: login hangs on an exhausted pool", ["ISSUE.md"]),
]


def git(workspace, *args, env=None):
    full_env = dict(os.environ)
    full_env.update({
        "GIT_AUTHOR_NAME": "Agent Fixture", "GIT_AUTHOR_EMAIL": "agent@example.invalid",
        "GIT_COMMITTER_NAME": "Agent Fixture", "GIT_COMMITTER_EMAIL": "agent@example.invalid",
        "GIT_CONFIG_NOSYSTEM": "1", "HOME": workspace,
    })
    full_env.update(env or {})
    return subprocess.run(["git", "-c", "init.defaultBranch=main", *args], cwd=workspace,
                          env=full_env, check=True, capture_output=True, text=True).stdout


def make_repo(workspace):
    os.makedirs(workspace, exist_ok=True)
    if os.listdir(workspace):
        sys.exit("make-repo: %s is not empty" % workspace)
    for rel, text in FILES.items():
        path = os.path.join(workspace, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as fh:
            fh.write(text)
    git(workspace, "init", "-q")
    for i, (message, paths) in enumerate(COMMITS):
        stamp = "2026-10-01T12:0%d:00Z" % i
        git(workspace, "add", *paths)
        git(workspace, "commit", "-q", "-m", message,
            env={"GIT_AUTHOR_DATE": stamp, "GIT_COMMITTER_DATE": stamp})
    # git writes HOME-relative files during commits; keep the tree clean.
    for leftover in (".gitconfig",):
        p = os.path.join(workspace, leftover)
        if os.path.exists(p):
            os.remove(p)


# ---------------------------------------------------------------------------
# Recording
# ---------------------------------------------------------------------------

def sha256(data):
    if isinstance(data, str):
        data = data.encode()
    return hashlib.sha256(data).hexdigest()


def shell_fingerprint(code, stdout, stderr):
    """Same scheme as ReplayKit's ShellExecutor."""
    return sha256("exit=%d;stdout=%s;stderr=%s" % (code, sha256(stdout), sha256(stderr)))


def text(value):
    return {"Text": value}


class Recorder:
    def __init__(self, collector, workspace):
        self.base = collector.rstrip("/")
        self.workspace = workspace
        self.run_id = None

    def post(self, path, body):
        req = urllib.request.Request(self.base + path, data=json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"}, method="POST")
        with urllib.request.urlopen(req) as resp:
            return json.loads(resp.read() or b"null")

    @staticmethod
    def now():
        return int(time.time() * 1000)

    def begin(self, title):
        run = self.post("/v1/runs", {
            "title": title, "entrypoint": "examples/scripted-agent/agent.py",
            "adapter_name": "replaykit-collector-http", "adapter_version": "0.1.0",
            "started_at": self.now(), "labels": ["scripted-agent"],
            "workspace_root": self.workspace,
        })
        self.run_id = run["run_id"]
        return self.run_id

    def start(self, span_id, kind, name, parent=None, policy="RerunnableSupported",
              executor=None, attributes=None):
        body = {"span_id": span_id, "kind": kind, "name": name, "started_at": self.now(),
                "replay_policy": policy, "attributes": attributes or {}}
        if parent:
            body["parent_span_id"] = parent
        if executor:
            body["executor_kind"] = executor
        self.post("/v1/runs/%s/spans" % self.run_id, body)

    def artifact(self, span_id, artifact_type, content, mime="text/plain"):
        body = {"artifact_type": artifact_type, "mime": mime, "created_at": self.now(),
                "span_id": span_id,
                "content_base64": base64.b64encode(content.encode()).decode()}
        return self.post("/v1/runs/%s/artifacts" % self.run_id, body)["artifact_id"]

    def end(self, span_id, status="Completed", outputs=(), fingerprint=None, error=None):
        body = {"ended_at": self.now(), "status": status, "output_artifact_ids": list(outputs)}
        if fingerprint:
            body["output_fingerprint"] = fingerprint
        if error:
            body["error_summary"] = error
        self.post("/v1/runs/%s/spans/%s/end" % (self.run_id, span_id), body)

    def edge(self, consumer, producer):
        self.post("/v1/runs/%s/edges" % self.run_id,
                  {"from_span_id": consumer, "to_span_id": producer, "kind": "DataDependsOn"})

    def finish(self, status):
        self.post("/v1/runs/%s/finish" % self.run_id, {"ended_at": self.now(), "status": status})

    # -- semantic steps -----------------------------------------------------

    def sh(self, command):
        proc = subprocess.run(["/bin/sh", "-c", command], cwd=self.workspace,
                              capture_output=True, text=True)
        return proc.returncode, proc.stdout, proc.stderr

    def file_read(self, span_id, parent, path):
        self.start(span_id, "FileRead", "read " + path, parent, "PureReusable", "fs.read",
                   {"path": text(path)})
        with open(os.path.join(self.workspace, path)) as fh:
            content = fh.read()
        out = self.artifact(span_id, "FileBlob", content)
        self.end(span_id, outputs=[out], fingerprint=sha256(content))
        return content

    def tool(self, span_id, parent, command):
        self.start(span_id, "ToolCall", span_id, parent, executor="tool." + span_id,
                   attributes={"command": text(command), "cwd": text(".")})
        code, stdout, stderr = self.sh(command)
        outputs = [self.artifact(span_id, "ToolOutput", stdout)]
        if stderr:
            outputs.append(self.artifact(span_id, "ShellStderr", stderr))
        status = "Completed" if code == 0 else "Failed"
        self.end(span_id, status, outputs, sha256(stdout),
                 None if code == 0 else "tool exited with code %d" % code)
        return stdout

    def shell(self, span_id, parent, name, command, bindings=None):
        attributes = {"command": text(command), "cwd": text(".")}
        if bindings:
            attributes["input_bindings"] = {"Object": {k: text(v) for k, v in bindings.items()}}
        self.start(span_id, "ShellCommand", name, parent, executor="shell", attributes=attributes)
        code, stdout, stderr = self.sh(command)
        outputs = []
        if stdout:
            outputs.append(self.artifact(span_id, "ShellStdout", stdout))
        if stderr:
            outputs.append(self.artifact(span_id, "ShellStderr", stderr))
        if not outputs:
            outputs.append(self.artifact(span_id, "ShellStdout", "exit_code=%d" % code))
        error = None
        if code != 0:
            error = "command exited with code %d: %s" % (code, stderr[:200])
        self.end(span_id, "Completed" if code == 0 else "Failed", outputs,
                 shell_fingerprint(code, stdout, stderr), error)
        return code, stdout

    def planner(self, span_id, name, parent="root"):
        self.start(span_id, "PlannerStep", name, parent, "RecordOnly")


PROMPT_TEMPLATE = """You are fixing a bug in a Python repository.

ISSUE.md:
{{read_issue}}
auth/session.py:
{{read_session}}
Return the full corrected file."""


def render(template, outputs):
    for key, value in outputs.items():
        template = template.replace("{{%s}}" % key, value)
    return template


def run_agent(collector, workspace):
    workspace = os.path.realpath(workspace)
    r = Recorder(collector, workspace)
    run_id = r.begin("Fix login timeout bug")
    r.start("root", "Run", "Fix login timeout bug", policy="RecordOnly")

    # 1. Reproduce.
    r.planner("plan_repro", "Reproduce failure")
    issue = r.file_read("read_issue", "plan_repro", "ISSUE.md")
    tests = r.tool("select_tests", "plan_repro", "git ls-files tests")
    r.edge("select_tests", "read_issue")
    test_cmd = "python3 run_tests.py {{select_tests|words}}"
    r.shell("test_repro", "plan_repro", "run tests",
            "python3 run_tests.py " + " ".join(tests.split()), {"command": test_cmd})
    r.edge("test_repro", "select_tests")
    r.end("plan_repro")

    # 2. Search.
    r.planner("plan_search", "Search codebase")
    r.tool("search_repository", "plan_search", "grep -rn 'acquire(' auth")
    r.tool("git_log", "plan_search", "git log --oneline -3")
    session = r.file_read("read_session", "plan_search", "auth/session.py")
    r.edge("read_session", "search_repository")
    r.end("plan_search")

    # 3. Fix: ask the (scripted) model, write its answer.
    r.planner("plan_fix", "Implement fix")
    prompt = render(PROMPT_TEMPLATE, {"read_issue": issue, "read_session": session})
    r.start("gen_fix", "LlmCall", "Generate fix", "plan_fix", executor="local.scripted-fixer",
            attributes={"model": text("scripted-fixer"), "provider": text("local"),
                        "prompt": text(prompt),
                        "input_bindings": {"Object": {"prompt": text(PROMPT_TEMPLATE)}}})
    request = json.dumps({"model": "scripted-fixer", "provider": "local", "prompt": prompt})
    proc = subprocess.run([sys.executable, os.path.join(HERE, "fixer.py")], input=request,
                          capture_output=True, text=True, check=True)
    fixed = proc.stdout
    out = r.artifact("gen_fix", "ModelResponse", fixed)
    r.end("gen_fix", outputs=[out], fingerprint=sha256(fixed))
    r.edge("gen_fix", "read_issue")
    r.edge("gen_fix", "read_session")

    r.start("write_fix", "FileWrite", "write auth/session.py", "plan_fix", executor="fs.write",
            attributes={"path": text("auth/session.py"), "content": text(fixed),
                        "input_bindings": {"Object": {"content": text("{{gen_fix}}")}}})
    with open(os.path.join(workspace, "auth/session.py"), "w") as fh:
        fh.write(fixed)
    diff = r.sh("git diff --no-color -- auth/session.py")[1]
    out = r.artifact("write_fix", "FileDiff", diff, "text/x-diff")
    r.end("write_fix", outputs=[out], fingerprint=sha256(fixed))
    r.edge("write_fix", "gen_fix")
    r.end("plan_fix")

    # 4. Validate.
    r.planner("plan_validate", "Validate fix")
    code, stdout = r.shell("test_validate", "plan_validate", "run tests",
                           "python3 run_tests.py " + " ".join(tests.split()),
                           {"command": test_cmd})
    r.edge("test_validate", "select_tests")
    r.edge("test_validate", "write_fix")
    r.shell("diff_stat", "plan_validate", "git diff --stat", "git diff --stat")
    r.edge("diff_stat", "write_fix")
    failing = sum(1 for line in stdout.splitlines() if line.endswith("FAILED"))
    if code == 0:
        r.end("plan_validate")
        r.end("root")
        r.finish("Completed")
    else:
        r.end("plan_validate", "Failed", error="tests still failing after the fix")
        r.end("root", "Failed",
              error="validation failed: %d test%s still failing" % (failing, "" if failing == 1 else "s"))
        r.finish("Failed")
    return run_id


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    mk = sub.add_parser("make-repo")
    mk.add_argument("workspace")
    run = sub.add_parser("run")
    run.add_argument("--collector", default=os.environ.get("REPLAYKIT_COLLECTOR_URL",
                                                           "http://127.0.0.1:4100"))
    run.add_argument("--workspace", required=True)
    args = parser.parse_args()
    if args.cmd == "make-repo":
        make_repo(args.workspace)
    else:
        print(run_agent(args.collector, args.workspace))


if __name__ == "__main__":
    main()

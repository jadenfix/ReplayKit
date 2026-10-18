#!/usr/bin/env python3
"""The scripted "model" behind the example agent's `Generate fix` LlmCall.

It is a deterministic stand-in for an LLM: it reads a ReplayKit model request
(JSON on stdin: {"model", "provider", "prompt", ...}) and prints the corrected
`auth/session.py`. The agent calls it while recording, and replays call it as a
pluggable provider:

    replaykit --model-mode command --model-command "python3 fixer.py" replay fork ...

Behaviour:
- the prompt must contain the current `auth/session.py`;
- by default the fix threads a timeout through and reads its default from
  AUTH_POOL_TIMEOUT (unset means wait forever, so the login test still hangs);
- if the instructions say "default the timeout to N seconds", the default is N.
"""

import json
import re
import sys

MINIMAL = '''import os

# Seconds to wait for a pooled connection; unset means wait forever.
DEFAULT_TIMEOUT = float(os.environ.get("AUTH_POOL_TIMEOUT", "0")) or None


def init_session(pool, timeout=None):
    if timeout is None:
        timeout = DEFAULT_TIMEOUT
    conn = pool.acquire(timeout=timeout)
    return {"conn": conn}
'''

WITH_DEFAULT = '''import os

# Seconds to wait for a pooled connection.
DEFAULT_TIMEOUT = float(os.environ.get("AUTH_POOL_TIMEOUT", "%s"))


def init_session(pool, timeout=None):
    if timeout is None:
        timeout = DEFAULT_TIMEOUT
    conn = pool.acquire(timeout=timeout)
    return {"conn": conn}
'''


def fix(prompt):
    if "def init_session(pool" not in prompt:
        sys.stderr.write("fixer: the prompt does not include auth/session.py\n")
        sys.exit(2)
    match = re.search(r"default the timeout to (\d+(?:\.\d+)?) seconds?", prompt, re.IGNORECASE)
    if match:
        return WITH_DEFAULT % match.group(1)
    return MINIMAL


def main():
    request = json.load(sys.stdin)
    sys.stdout.write(fix(request.get("prompt") or ""))


if __name__ == "__main__":
    main()

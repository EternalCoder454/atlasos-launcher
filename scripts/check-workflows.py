#!/usr/bin/env python3
"""Checks the GitHub workflows against the rules of docs/SECURITY.md ("GitHub
workflows"). It fails (exit 1) when a workflow

  * is triggered by pull_request_target or workflow_run (they run with secrets
    and a write token on code a pull request can influence);
  * has no top-level `permissions:` block, or one that is not read-only, or a
    job that asks for write access without being on WRITERS (each with the
    reason it needs it);
  * uses an action or reusable workflow that is not pinned to a full commit sha
    (with its version in a comment), or a docker:// action;
  * checks out without `persist-credentials: false`;
  * puts a `${{ }}` expression that an attacker can influence (inputs, branch
    and tag names, event fields, step outputs, matrix values) into a `run:` or
    `script:` (pass it through `env:` instead);
  * uses a secret other than GITHUB_TOKEN outside a job with an `environment:`
    (a reviewer gate), puts a secret in a `run:`, or says `secrets: inherit`;
  * pipes curl or wget into a shell;
  * saves a cache from a step that is not limited to a push to main (a pull
    request must not fill the cache that main restores).

    scripts/check-workflows.py [--root DIR]     DIR is the repository root

The workflows are read with a small YAML reader for the subset they are written
in (block maps and lists, block scalars, flow lists, comments). Anything it does
not understand is an error, never silently skipped. Standard library only.
"""

import argparse
import os
import re
import sys

# --------------------------------------------------------------------------
# Exceptions to the rules, each with its reason. A new entry needs a look at
# what it lets through.

# job (file:job) -> the write permissions it may have, and why.
WRITERS = {
    ".github/workflows/ci.yml:image": (
        {"packages"},
        "calls dev-image.yml, whose publish job pushes the build image to GHCR (main only)",
    ),
    ".github/workflows/dev-image.yml:publish": (
        {"packages"},
        "pushes the build image to GHCR, on main only, after ci/secret-check.sh",
    ),
    ".github/workflows/security.yml:audit": (
        {"checks"},
        "rustsec/audit-check reports its result as a check run",
    ),
}

# Cache saves that are not limited to a push to main, with the reason. None.
CACHE_SAVERS = {}

# (file, step id or name) -> `${{ }}` expressions allowed in its run script. None.
RUN_EXPRESSIONS = {}

# Expressions that are safe in a script: made by the runner or the workflow,
# not by whoever opened the pull request or pushed the branch or tag.
SAFE_EXPRESSIONS = {
    "github.workspace",
    "github.sha",
    "github.repository",
    "github.run_id",
    "github.run_attempt",
    "runner.temp",
    "runner.os",
}

# Container images that are not pinned by digest, with the reason.
UNPINNED_IMAGES = {
    "ghcr.io/eternalcoder454/atlas-launcher-dev:44": (
        "our own build image, rebuilt by dev-image.yml whenever the Containerfile, "
        "the spec's BuildRequires or the framework tag change; a digest would "
        "have to be committed after every rebuild"
    ),
}


class YamlError(Exception):
    pass


# --------------------------------------------------------------------------
# A reader for the YAML the workflows are written in.

_KEY = re.compile(r"""^("[^"]*"|'[^']*'|[A-Za-z0-9_./-]+)\s*:(\s+(.*)|)$""")


def _strip_comment(s):
    """Text without a trailing ` # comment` (outside quotes)."""
    quote = None
    for i, c in enumerate(s):
        if quote:
            if c == quote:
                quote = None
        elif c in "\"'" and (i == 0 or s[i - 1] in " [,{:"):
            quote = c
        elif c == "#" and (i == 0 or s[i - 1] in " \t"):
            return s[:i].rstrip()
    return s.rstrip()


def _scalar(s):
    s = s.strip()
    if len(s) >= 2 and s[0] == s[-1] and s[0] in "\"'":
        return s[1:-1]
    if s.startswith("["):
        if not s.endswith("]"):
            raise YamlError("a flow list over several lines: " + s)
        inner = s[1:-1].strip()
        return [_scalar(p) for p in inner.split(",")] if inner else []
    if s.startswith("{") or s[:1] in "&*!|>" and s not in ("|", ">", "|-", ">-", "|+", ">+"):
        if s.startswith("{}"):
            return {}
        raise YamlError("YAML this reader does not handle: " + s)
    if s in ("", "~", "null"):
        return None
    return s


def parse_yaml(text):
    raw = text.split("\n")
    lines = []  # (indent, text, raw line number)
    for n, line in enumerate(raw, 1):
        if "\t" in line[: len(line) - len(line.lstrip())]:
            raise YamlError(f"line {n}: a tab in the indentation")
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            lines.append((None, line, n))  # kept for block scalars
            continue
        lines.append((len(line) - len(line.lstrip(" ")), line, n))

    def next_real(i):
        while i < len(lines) and lines[i][0] is None:
            i += 1
        return i

    def block_scalar(i, parent_indent, style):
        out = []
        base = None
        while i < len(lines):
            ind, line, _ = lines[i]
            if ind is None:
                out.append("")
                i += 1
                continue
            if ind <= parent_indent:
                break
            if base is None:
                base = ind
            out.append(line[base:] if ind >= base else line.lstrip())
            i += 1
        while out and out[-1] == "":
            out.pop()
        sep = "\n" if style.startswith("|") else " "
        return sep.join(out), i

    def parse_value(rest, i, indent):
        """The value after `key:` whose rest of line is `rest`; i is the next
        line; indent is the key's indent."""
        rest = _strip_comment(rest.strip())
        if rest in ("|", ">", "|-", ">-", "|+", ">+"):
            return block_scalar(i, indent, rest)
        if rest == "":
            j = next_real(i)
            if j < len(lines):
                ind = lines[j][0]
                # A list may sit at the key's own indent.
                if ind > indent or (ind == indent and lines[j][1].lstrip().startswith("- ")):
                    return parse_block(j, ind)
            return None, i
        return _scalar(rest), i

    def parse_block(i, indent):
        i = next_real(i)
        if i >= len(lines):
            return None, i
        first = lines[i][1].strip()
        if first.startswith("- ") or first == "-":
            return parse_list(i, indent)
        return parse_map(i, indent)

    def parse_list(i, indent):
        out = []
        while True:
            i = next_real(i)
            if i >= len(lines) or lines[i][0] != indent:
                break
            body = lines[i][1].strip()
            if not (body.startswith("- ") or body == "-"):
                break
            item = _strip_comment(body[1:].strip())
            n = lines[i][2]
            if item == "":
                j = next_real(i + 1)
                if j < len(lines) and lines[j][0] > indent:
                    value, i = parse_block(j, lines[j][0])
                else:
                    value, i = None, i + 1
                out.append(value)
            elif _KEY.match(item):
                # `- key: value`: a map whose keys start at the item's column.
                line = lines[i][1]
                rest = line[indent + 1 :]
                col = indent + 1 + (len(rest) - len(rest.lstrip(" ")))
                lines[i] = (col, " " * col + item, n)
                value, i = parse_map(i, col)
                out.append(value)
            else:
                out.append(_scalar(item))
                i += 1
        return out, i

    def parse_map(i, indent):
        out = {}
        while True:
            i = next_real(i)
            if i >= len(lines) or lines[i][0] != indent:
                break
            body = _strip_comment(lines[i][1].strip())
            m = _KEY.match(body)
            if not m:
                if lines[i][1].strip().startswith("- "):
                    break
                raise YamlError(f"line {lines[i][2]}: not a `key: value` line: {body}")
            key = _scalar(m.group(1))
            if key in out:
                raise YamlError(f"line {lines[i][2]}: duplicate key {key}")
            value, i = parse_value(m.group(3) or "", i + 1, indent)
            out[key] = value
        if i < len(lines) and next_real(i) < len(lines):
            j = next_real(i)
            if lines[j][0] > indent:
                raise YamlError(f"line {lines[j][2]}: unexpected indentation")
        return out, i

    i = next_real(0)
    if i >= len(lines):
        return None
    value, i = parse_block(i, lines[i][0])
    i = next_real(i)
    if i < len(lines):
        raise YamlError(f"line {lines[i][2]}: unexpected content")
    return value


# --------------------------------------------------------------------------
# The rules.

SHA = re.compile(r"^[0-9a-f]{40}$")
EXPR = re.compile(r"\$\{\{\s*(.*?)\s*\}\}", re.S)
PLACEHOLDER = re.compile(r"^(<[A-Z_]+>|vX\.Y\.Z)$")


def _jobs(doc):
    jobs = doc.get("jobs") or {}
    return jobs.items() if isinstance(jobs, dict) else []


def _steps(job):
    steps = job.get("steps") or []
    return steps if isinstance(steps, list) else []


def _triggers(doc):
    on = doc.get("on", doc.get(True))
    if on is None:
        return []
    if isinstance(on, dict):
        return list(on)
    if isinstance(on, list):
        return on
    return [on]


def check_uses(where, uses, template, comment_ok, errors):
    if uses.startswith("./"):
        return
    if uses.startswith("docker://"):
        errors.append(f"{where}: {uses}: a docker:// action is not pinned (use a digest in a local action)")
        return
    if "@" not in uses:
        errors.append(f"{where}: {uses}: no @ref")
        return
    ref = uses.rsplit("@", 1)[1]
    if SHA.match(ref):
        if not comment_ok:
            errors.append(f"{where}: {uses}: pinned by sha but with no `# vX.Y.Z` comment saying which release")
        return
    if template and PLACEHOLDER.match(ref):
        return
    errors.append(f"{where}: {uses}: not pinned to a full commit sha")


def check_file(path, rel, text, errors):
    template = rel.startswith("template/")
    try:
        doc = parse_yaml(text)
    except YamlError as e:
        errors.append(f"{rel}: cannot read this workflow safely: {e}")
        return
    if not isinstance(doc, dict):
        # An action.yml has no `on:`; handled by the caller.
        errors.append(f"{rel}: not a mapping")
        return
    is_action = "runs" in doc and "jobs" not in doc

    lines = text.split("\n")

    def comment_after(uses_value):
        for line in lines:
            if uses_value in line and "#" in line.split(uses_value, 1)[1]:
                return True
        return False

    if not is_action:
        for t in _triggers(doc):
            if t in ("pull_request_target", "workflow_run"):
                errors.append(f"{rel}: triggered by {t}, which runs with secrets and a write token for code a pull request can influence")
        perms = doc.get("permissions")
        if perms is None:
            errors.append(f"{rel}: no top-level `permissions:` (the default token may be read-write)")
        elif isinstance(perms, str):
            if perms not in ("read-all", "{}"):
                errors.append(f"{rel}: top-level permissions: {perms}")
        elif isinstance(perms, dict):
            for k, v in perms.items():
                if v not in ("read", "none"):
                    errors.append(f"{rel}: top-level permissions {k}: {v} (the top level is read-only; grant writes to the job)")
    if "secrets" in doc and doc["secrets"] == "inherit":
        errors.append(f"{rel}: secrets: inherit")

    jobs = list(_jobs(doc))
    if is_action:
        jobs = [("(action)", {"steps": (doc.get("runs") or {}).get("steps") or []})]
    for name, job in jobs:
        if not isinstance(job, dict):
            errors.append(f"{rel}: job {name} is not a mapping")
            continue
        key = f"{rel}:{name}"
        if job.get("secrets") == "inherit":
            errors.append(f"{key}: secrets: inherit")
        if "uses" in job:
            check_uses(key, str(job["uses"]), template, comment_after(str(job["uses"])), errors)
        jp = job.get("permissions")
        if isinstance(jp, str) and jp not in ("read-all", "{}"):
            errors.append(f"{key}: permissions: {jp}")
        elif isinstance(jp, dict):
            writes = {k for k, v in jp.items() if v == "write"}
            allowed = WRITERS.get(key, (set(), ""))[0]
            if writes - allowed:
                errors.append(
                    f"{key}: write permission {sorted(writes - allowed)} is not on the list of writers in scripts/check-workflows.py (give the reason there)"
                )
        image = job.get("container")
        if isinstance(image, dict):
            image = image.get("image")
        if isinstance(image, str) and "@sha256:" not in image and image not in UNPINNED_IMAGES:
            errors.append(f"{key}: container image {image} is not pinned by digest")
        gated = "environment" in job
        for n, step in enumerate(_steps(job), 1):
            if not isinstance(step, dict):
                errors.append(f"{key}: step {n} is not a mapping")
                continue
            sid = step.get("id") or step.get("name") or f"#{n}"
            where = f"{key} step {sid}"
            uses = step.get("uses")
            if uses is not None:
                uses = str(uses)
                check_uses(where, uses, template, comment_after(uses), errors)
                action = uses.split("@")[0]
                w = step.get("with") or {}
                if action == "actions/checkout" and str(w.get("persist-credentials")) != "false":
                    errors.append(f"{where}: actions/checkout without `persist-credentials: false` leaves the token in .git/config for every later step")
                if action in ("actions/cache", "actions/cache/save"):
                    cond = str(step.get("if") or "")
                    if ("refs/heads/main" not in cond or "push" not in cond) and key not in CACHE_SAVERS:
                        errors.append(f"{where}: a cache is saved on a pull request too (limit it with `if:` to a push to main)")
                if action == "actions/github-script":
                    script = str((w or {}).get("script") or "")
                    for m in EXPR.finditer(script):
                        errors.append(f"{where}: `${{{{ {m.group(1)} }}}}` inside a github-script: pass it through env")
            script = step.get("run")
            if script is not None:
                script = str(script)
                allowed = SAFE_EXPRESSIONS | RUN_EXPRESSIONS.get((rel, str(sid)), (set(), ""))[0]
                for m in EXPR.finditer(script):
                    expr = m.group(1)
                    if expr not in allowed:
                        errors.append(
                            f"{where}: `${{{{ {expr} }}}}` inside a run script can be made to run commands by whoever controls it: pass it through `env:`"
                        )
                if re.search(r"\b(curl|wget)\b[^\n|]*\|\s*(sudo\s+)?(ba|z)?sh\b", script):
                    errors.append(f"{where}: pipes a download into a shell")
            blob = repr(step)
            for m in re.finditer(r"secrets\.([A-Za-z0-9_]+)", blob):
                if m.group(1) == "GITHUB_TOKEN":
                    continue
                if not gated:
                    errors.append(f"{where}: secrets.{m.group(1)} in a job with no `environment:` (no reviewer gate)")
                if script is not None and f"secrets.{m.group(1)}" in script:
                    errors.append(f"{where}: secrets.{m.group(1)} inside a run script: pass it through env")
        for k in ("env",):
            blob = repr(job.get(k))
            for m in re.finditer(r"secrets\.([A-Za-z0-9_]+)", blob):
                if m.group(1) != "GITHUB_TOKEN" and not gated:
                    errors.append(f"{key}: secrets.{m.group(1)} in a job with no `environment:`")


def workflow_files(root):
    out = []
    for base in (".github/workflows", "template/.github/workflows"):
        d = os.path.join(root, base)
        if os.path.isdir(d):
            for name in sorted(os.listdir(d)):
                if name.endswith((".yml", ".yaml")):
                    out.append(os.path.join(base, name))
    actions = os.path.join(root, ".github/actions")
    if os.path.isdir(actions):
        for name in sorted(os.listdir(actions)):
            for f in ("action.yml", "action.yaml"):
                if os.path.isfile(os.path.join(actions, name, f)):
                    out.append(os.path.join(".github/actions", name, f))
    return out


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    args = ap.parse_args(argv)
    files = workflow_files(args.root)
    if len(files) < 3:
        print(f"check-workflows: found only {len(files)} workflow file(s) under {args.root}", file=sys.stderr)
        return 1
    errors = []
    for rel in files:
        with open(os.path.join(args.root, rel), encoding="utf-8") as f:
            check_file(os.path.join(args.root, rel), rel, f.read(), errors)
    for e in errors:
        print("check-workflows: " + e, file=sys.stderr)
    if errors:
        print(f"check-workflows: {len(errors)} problem(s) in {len(files)} file(s)", file=sys.stderr)
        return 1
    print(f"check-workflows: {len(files)} file(s) ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

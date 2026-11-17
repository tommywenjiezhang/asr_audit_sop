"""Post-install checks for ASR/Audit Support Desk (called by scripts/verify-install.ps1).

  python -I verify_install.py db
      Read-only check of %APPDATA%\\gov.nj.doh.asrsupportdesk\\support_desk.db:
      integrity, tables, 18 issue types, templates seeded, row counts.
  python -I verify_install.py worker <outlook_worker.exe|outlook_worker.py> [--draft]
      test_connection against the running (non-elevated) classic Outlook.
      --draft also saves ONE clearly labelled test draft (no recipient). Nothing is ever sent.

JSON is written to the worker's stdin as plain UTF-8 bytes, exactly like the Rust app does
(PowerShell's stdin writer adds a BOM the worker cannot parse).
"""
import json
import os
import sqlite3
import subprocess
import sys

EXPECTED_ISSUE_TYPES = 18


def check_db() -> int:
    path = os.path.join(os.environ["APPDATA"], "gov.nj.doh.asrsupportdesk", "support_desk.db")
    if not os.path.isfile(path):
        print(f"FAIL database not found: {path} (has the app been started?)")
        return 1
    con = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    ok = True
    integrity = con.execute("pragma integrity_check").fetchone()[0]
    print(f"{'OK  ' if integrity == 'ok' else 'FAIL'} integrity_check: {integrity}")
    ok &= integrity == "ok"
    tables = [r[0] for r in con.execute("select name from sqlite_master where type='table' and name not like 'sqlite_%' order by name")]
    for t in tables:
        print(f"     {t}: {con.execute(f'select count(*) from \"{t}\"').fetchone()[0]} rows")
    issues = con.execute("select count(*) from issue_definitions").fetchone()[0]
    print(f"{'OK  ' if issues == EXPECTED_ISSUE_TYPES else 'FAIL'} issue types: {issues} (expected {EXPECTED_ISSUE_TYPES})")
    ok &= issues == EXPECTED_ISSUE_TYPES
    templates = con.execute("select count(*) from email_templates").fetchone()[0]
    print(f"{'OK  ' if templates > 0 else 'FAIL'} email templates seeded: {templates}")
    ok &= templates > 0
    return 0 if ok else 1


def call_worker(worker: str, req: dict) -> dict:
    cmd = [sys.executable, "-I", worker] if worker.lower().endswith(".py") else [worker]
    r = subprocess.run(cmd, input=json.dumps(req).encode("utf-8"), capture_output=True, timeout=120)
    try:
        return json.loads(r.stdout.decode("utf-8", errors="replace"))
    except json.JSONDecodeError:
        return {"ok": False, "message": f"unparseable worker output (exit {r.returncode}): {r.stdout[:300]!r} {r.stderr[-300:]!r}"}


def check_worker(worker: str, draft: bool) -> int:
    res = call_worker(worker, {"op": "test_connection", "params": {}})
    if not res.get("ok"):
        print(f"FAIL test_connection: {res.get('error_code')} {res.get('message')}")
        return 1
    d = res["data"]
    print(f"OK   Outlook {d.get('outlook_version')} profile={d.get('profile')!r} stores={d.get('store_count')}")
    if not draft:
        return 0
    res = call_worker(worker, {"op": "create_reply_draft", "params": {
        "mode": "new",
        "subject": "[TEST - safe to delete] ASR Support Desk build verification draft",
        "html_body": "<p>Build verification draft created by scripts/verify-install.ps1. Not sent. Safe to delete.</p>",
        "display": False}})
    if not res.get("ok") or res["data"].get("sent") is not False:
        print(f"FAIL create_reply_draft: {res}")
        return 1
    print("OK   test draft saved to Drafts (sent=false). Delete '[TEST - safe to delete] ...' from Outlook Drafts.")
    return 0


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "db":
        sys.exit(check_db())
    if len(sys.argv) >= 3 and sys.argv[1] == "worker":
        sys.exit(check_worker(sys.argv[2], "--draft" in sys.argv[3:]))
    print(__doc__)
    sys.exit(2)

"""Verify that the production bundle contains every integrated application screen.

This is intentionally a small source/bundle contract test. It catches a release
that compiles a stale or partial frontend even when Rust and Svelte checks pass.
"""
from pathlib import Path
import json
import sys


ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "src" / "App.svelte"
CONFIG = ROOT / "src-tauri" / "tauri.conf.json"
DIST = ROOT / "dist-web"
FEATURES = {
    "Kanban": "Kanban",
    "Conflicts": "Conflicts",
    "Team Overview": "Overview",
    "Diagnostics": "Diagnostics",
    "Shared Database": "SharedDatabasePanel",
    "Related cases": "RelatedCases",
}


def fail(message: str) -> int:
    print(f"FAIL frontend bundle: {message}")
    return 1


def main() -> int:
    if not APP.is_file():
        return fail(f"missing {APP}")
    if not CONFIG.is_file():
        return fail(f"missing {CONFIG}")
    if not DIST.is_dir():
        return fail(f"missing {DIST}; run npm run build first")

    source = "\n".join(p.read_text(encoding="utf-8", errors="ignore") for p in (ROOT / "src").rglob("*.svelte"))
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    if config.get("build", {}).get("frontendDist") != "../dist-web":
        return fail("tauri.conf.json frontendDist is not ../dist-web")

    assets = list(DIST.rglob("*.js"))
    if not assets:
        return fail("dist-web contains no JavaScript assets")
    bundle = "\n".join(p.read_text(encoding="utf-8", errors="ignore") for p in assets)
    for label, source_marker in FEATURES.items():
        if source_marker not in source:
            return fail(f"source no longer registers {label} ({source_marker})")
        if label not in bundle:
            return fail(f"production JavaScript does not contain {label}")
    print("OK frontend bundle: all integrated screens are registered and embedded")
    return 0


if __name__ == "__main__":
    sys.exit(main())

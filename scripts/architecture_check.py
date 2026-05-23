#!/usr/bin/env python3
"""Architecture guard for meaningful code splitting.

The guard intentionally uses soft budgets, legacy-file ratchets, and remediation
text instead of a single global line limit. It should point contributors toward
coherent modules, not reward arbitrary slicing.
"""

from __future__ import annotations

import argparse
import datetime as dt
import fnmatch
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - depends on host Python.
    tomllib = None


RULE_LEGACY_GROWTH = "legacy_growth"
RULE_LARGE_NEW_FILE = "large_new_file"
RULE_BAD_MODULE_NAME = "bad_module_name"
RULE_EXPIRED_WAIVER = "expired_waiver"
RULE_MISSING_CONFIGURED_FILE = "missing_configured_file"


@dataclass
class Finding:
    severity: str
    rule: str
    path: str
    message: str
    remediation: list[str]
    allowed_alternatives: list[str] | None = None
    routing: list[dict[str, Any]] | None = None


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    config_path = (root / args.config).resolve()
    config = load_config(config_path)

    findings = check_repository(root, config)
    print_report(findings)
    return 1 if any(item.severity == "error" for item in findings) else 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Check architecture budgets and module-boundary guardrails."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="Repository root. Defaults to the parent of scripts/.",
    )
    parser.add_argument(
        "--config",
        default="architecture.guard.toml",
        help="Guard config path relative to --root.",
    )
    return parser.parse_args()


def load_config(path: Path) -> dict[str, Any]:
    if tomllib is None:
        raise SystemExit(
            "architecture_check.py requires Python 3.11+ for tomllib. "
            "Use a newer Python or run it in CI."
        )
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except FileNotFoundError:
        raise SystemExit(f"Missing architecture guard config: {path}") from None
    except tomllib.TOMLDecodeError as error:
        raise SystemExit(f"Invalid TOML in {path}: {error}") from None


def check_repository(root: Path, config: dict[str, Any]) -> list[Finding]:
    defaults = config.get("defaults", {})
    configured_files = {rule["path"]: rule for rule in config.get("files", [])}
    findings: list[Finding] = []

    findings.extend(check_waivers(config.get("waivers", [])))

    for path, rule in configured_files.items():
        findings.extend(check_configured_file(root, config, path, rule))

    source_files = collect_source_files(root, defaults.get("source_globs", []))
    for path in source_files:
        rel_path = relative_path(root, path)
        if rel_path in configured_files:
            continue
        findings.extend(check_general_source_file(root, config, rel_path))

    return findings


def check_configured_file(
    root: Path, config: dict[str, Any], rel_path: str, rule: dict[str, Any]
) -> list[Finding]:
    path = root / rel_path
    if not path.exists():
        return [
            Finding(
                "error",
                RULE_MISSING_CONFIGURED_FILE,
                rel_path,
                f"{rel_path} is listed in architecture.guard.toml but does not exist.",
                ["Remove the stale entry or update it to the new path."],
            )
        ]

    if rule.get("kind") != "legacy-large":
        return []

    lines = count_lines(path)
    baseline = int(rule["baseline_lines"])
    growth = lines - baseline
    warn = int(rule.get("growth_warn_lines", 0))
    fail = int(rule.get("growth_fail_lines", warn))
    remediation = list(rule.get("remediation", []))
    routing = routing_for_path(config, rel_path)

    if growth > fail and not has_active_waiver(config, rel_path, RULE_LEGACY_GROWTH):
        return [
            Finding(
                "error",
                RULE_LEGACY_GROWTH,
                rel_path,
                (
                    f"{rel_path} grew by {growth} lines since the ratchet baseline "
                    f"({lines} current, {baseline} baseline, {fail} allowed before failure)."
                ),
                remediation,
                waiver_text(rel_path, RULE_LEGACY_GROWTH),
                routing,
            )
        ]

    if growth > warn and not has_active_waiver(config, rel_path, RULE_LEGACY_GROWTH):
        return [
            Finding(
                "warning",
                RULE_LEGACY_GROWTH,
                rel_path,
                (
                    f"{rel_path} grew by {growth} lines since the ratchet baseline "
                    f"({lines} current, {baseline} baseline, warning after {warn})."
                ),
                remediation,
                waiver_text(rel_path, RULE_LEGACY_GROWTH),
                routing,
            )
        ]

    if lines < baseline:
        return [
            Finding(
                "warning",
                "baseline_can_move_down",
                rel_path,
                (
                    f"{rel_path} is now {baseline - lines} lines below its baseline "
                    f"({lines} current, {baseline} baseline)."
                ),
                [
                    "Lower baseline_lines in architecture.guard.toml after the refactor lands.",
                    "Keep the ratchet from drifting back upward.",
                ],
            )
        ]

    return []


def check_general_source_file(
    root: Path, config: dict[str, Any], rel_path: str
) -> list[Finding]:
    defaults = config.get("defaults", {})
    path = root / rel_path
    findings: list[Finding] = []
    allowed_bad_names = set(defaults.get("allowed_bad_module_paths", []))
    bad_names = set(defaults.get("bad_module_names", []))

    if (
        path.name in bad_names
        and rel_path not in allowed_bad_names
        and not has_active_waiver(config, rel_path, RULE_BAD_MODULE_NAME)
    ):
        findings.append(
            Finding(
                "error",
                RULE_BAD_MODULE_NAME,
                rel_path,
                f"{rel_path} uses a vague module name.",
                [
                    "Rename the module after the responsibility it owns.",
                    "Prefer names such as commands.rs, effects.rs, platform.rs, scheduler.rs, or cache.rs.",
                ],
                waiver_text(rel_path, RULE_BAD_MODULE_NAME),
            )
        )

    lines = count_lines(path)
    warn = int(defaults.get("new_file_warn_lines", 600))
    fail = int(defaults.get("new_file_fail_lines", 900))
    if lines > fail and not has_active_waiver(config, rel_path, RULE_LARGE_NEW_FILE):
        findings.append(
            Finding(
                "error",
                RULE_LARGE_NEW_FILE,
                rel_path,
                f"{rel_path} is {lines} lines, above the {fail}-line new-file failure budget.",
                [
                    "Split by stable responsibility before the file becomes another legacy-large module.",
                    "If the file is a generated or single-purpose artifact, add a temporary waiver with a reason.",
                ],
                waiver_text(rel_path, RULE_LARGE_NEW_FILE),
            )
        )
    elif lines > warn and not has_active_waiver(config, rel_path, RULE_LARGE_NEW_FILE):
        findings.append(
            Finding(
                "warning",
                RULE_LARGE_NEW_FILE,
                rel_path,
                f"{rel_path} is {lines} lines, above the {warn}-line new-file warning budget.",
                [
                    "Check whether the file has more than one durable responsibility.",
                    "Split only when the boundary makes the code easier to understand.",
                ],
            )
        )

    return findings


def check_waivers(waivers: list[dict[str, Any]]) -> list[Finding]:
    findings: list[Finding] = []
    today = dt.date.today()
    for waiver in waivers:
        expires_text = waiver.get("expires")
        try:
            expires = dt.date.fromisoformat(expires_text)
        except (TypeError, ValueError):
            findings.append(
                Finding(
                    "error",
                    RULE_EXPIRED_WAIVER,
                    str(waiver.get("path", "<unknown>")),
                    "A waiver has a missing or invalid expires date.",
                    ["Use ISO format, for example expires = \"2026-06-15\"."],
                )
            )
            continue
        if expires < today:
            findings.append(
                Finding(
                    "error",
                    RULE_EXPIRED_WAIVER,
                    str(waiver.get("path", "<unknown>")),
                    f"A waiver for rule {waiver.get('rule', '<unknown>')} expired on {expires}.",
                    [
                        "Remove the waiver and address the split.",
                        "Renew it only with a fresh reason and a new short expiry date.",
                    ],
                )
            )
    return findings


def collect_source_files(root: Path, globs: list[str]) -> list[Path]:
    files: dict[str, Path] = {}
    for pattern in globs:
        for path in root.glob(pattern):
            if path.is_file():
                files[relative_path(root, path)] = path
    return [files[key] for key in sorted(files)]


def count_lines(path: Path) -> int:
    return len(path.read_text(encoding="utf-8").splitlines())


def relative_path(root: Path, path: Path) -> str:
    return path.resolve().relative_to(root).as_posix()


def has_active_waiver(config: dict[str, Any], path: str, rule: str) -> bool:
    today = dt.date.today()
    for waiver in config.get("waivers", []):
        if waiver.get("path") != path or waiver.get("rule") != rule:
            continue
        try:
            expires = dt.date.fromisoformat(waiver["expires"])
        except (KeyError, TypeError, ValueError):
            continue
        if expires >= today:
            return True
    return False


def routing_for_path(config: dict[str, Any], path: str) -> list[dict[str, Any]]:
    routes = []
    for route in config.get("routing", []):
        if any(fnmatch.fnmatch(path, pattern) for pattern in route.get("avoid_paths", [])):
            routes.append(route)
    return routes


def waiver_text(path: str, rule: str) -> list[str]:
    return [
        "If keeping the code together is clearer, add a short-lived waiver:",
        "",
        "[[waivers]]",
        f"path = \"{path}\"",
        f"rule = \"{rule}\"",
        "reason = \"Explain why splitting now would reduce clarity.\"",
        "expires = \"YYYY-MM-DD\"",
    ]


def print_report(findings: list[Finding]) -> None:
    errors = [item for item in findings if item.severity == "error"]
    warnings = [item for item in findings if item.severity == "warning"]

    if not findings:
        print("Architecture guard passed.")
        return

    if errors:
        print(f"Architecture guard failed with {len(errors)} error(s).")
    else:
        print("Architecture guard passed with warnings.")

    for title, items in (("Errors", errors), ("Warnings", warnings)):
        if not items:
            continue
        print()
        print(f"{title}:")
        for item in items:
            print_finding(item)


def print_finding(item: Finding) -> None:
    print(f"- [{item.rule}] {item.path}: {item.message}")
    if item.remediation:
        print("  Recommended actions:")
        for action in item.remediation:
            if action:
                print(f"  - {action}")
            else:
                print("  -")
    if item.routing:
        print("  Likely destinations:")
        for route in item.routing:
            signals = ", ".join(route.get("signals", []))
            print(f"  - {route['name']}: {route['destination']} ({signals})")
    if item.allowed_alternatives:
        print("  Allowed alternative:")
        for line in item.allowed_alternatives:
            print(f"  {line}" if line else "  ")


if __name__ == "__main__":
    raise SystemExit(main())

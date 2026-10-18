#!/usr/bin/env python3
"""Validate checked-in schemas and real tool output with jsonschema 4.25.1.

Development-only dependency; no JSON Schema engine is linked into the Rust tools.
Run from any directory. Temporary projects protect every checked-in fixture.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
from importlib.metadata import PackageNotFoundError, version
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

try:
    import jsonschema
    from referencing import Registry, Resource
except ImportError:
    sys.exit("Install the optional validator: python -m pip install jsonschema==4.25.1")

ROOT = Path(__file__).resolve().parents[1]
SPEC = ROOT / "docs/spec/0.1.0/schemas"
CLI_SCHEMAS = ROOT / "docs/cli/0.1.0"
REQUIRED_JSONSCHEMA = "4.25.1"


def read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def command(args: list[str], *, cwd: Path = ROOT, env=None, timeout=300):
    result = subprocess.run(args, cwd=cwd, env=env, capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(args)}\n"
            f"{result.stdout.decode('utf-8', errors='replace')}"
            f"{result.stderr.decode('utf-8', errors='replace')}"
        )
    return result


class Validation:
    def __init__(self, executable: Path):
        self.executable = executable
        self.counts = Counter()
        self.schema_paths = sorted((ROOT / "docs").rglob("*.schema.json"))
        self.schemas = {path: read_json(path) for path in self.schema_paths}
        resources = [(schema["$id"], Resource.from_contents(schema))
                     for schema in self.schemas.values()]
        self.registry = Registry().with_resources(resources)
        self.validators = {}
        for path, schema in self.schemas.items():
            validator = jsonschema.validators.validator_for(schema)
            validator.check_schema(schema)
            self.validators[path] = validator(
                schema, registry=self.registry, format_checker=jsonschema.FormatChecker()
            )
            self.counts["schemas"] += 1

    def validate(self, document, schema: Path, label: str, expected=True):
        errors = sorted(self.validators[schema].iter_errors(document), key=str)
        if bool(errors) == expected:
            detail = "\n".join(str(error) for error in errors[:3])
            raise AssertionError(f"{label}: expected schema {'acceptance' if expected else 'rejection'}\n{detail}")

    def cli(self, args: list[str], project: Path, *, schema=None, codes=(0,), corpus=False):
        result = subprocess.run(
            [str(self.executable), *args, "--json"], cwd=project,
            capture_output=True, timeout=30,
        )
        if result.returncode not in codes:
            raise AssertionError(f"{args}: unexpected exit {result.returncode}: {result.stdout!r} {result.stderr!r}")
        if result.stderr:
            raise AssertionError(f"{args}: JSON invocation wrote to stderr: {result.stderr!r}")
        try:
            document = json.loads(result.stdout)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise AssertionError(f"{args}: stdout is not exactly one UTF-8 JSON document") from error
        kind = document.get("kind")
        if kind == "diagnostic":
            selected = SPEC / "diagnostic.schema.json"
        elif schema is not None:
            selected = schema
        elif kind == "version":
            selected = CLI_SCHEMAS / "version.schema.json"
        elif kind == "help":
            selected = CLI_SCHEMAS / "help.schema.json"
        elif kind in ("check", "format"):
            selected = CLI_SCHEMAS / "source.schema.json"
        else:
            raise AssertionError(f"no schema selected for emitted document kind {kind!r}")
        self.validate(document, selected, f"CLI {' '.join(args)}")
        self.counts["cli_documents"] += 1
        self.counts["corpus_cli_documents" if corpus else "extra_cli_documents"] += 1
        self.counts[f"cli_kind_{kind}"] += 1
        return document

    def protocol_examples(self):
        directory = ROOT / "docs/spec/0.1.0/examples/protocol"
        rejected = {"diagnostic-i64-number.invalid.json", "change-unknown-operation.invalid.json"}
        contextual = {"change-stale.invalid.json", "change-protected.invalid.json"}
        examples = sorted(directory.glob("*.json"))
        if len(examples) < 9:
            raise AssertionError("expected at least the nine documented protocol examples")
        for path in examples:
            document = read_json(path)
            schema = SPEC / ("diagnostic.schema.json" if document["kind"] == "diagnostic" else "change-request.schema.json")
            accepted = path.name not in rejected
            # Section 7 of protocol.md explicitly expects these two '.invalid'
            # examples to pass structure and fail later contextual policy checks.
            if path.name.endswith(".invalid.json") and path.name not in rejected | contextual:
                raise AssertionError(f"undocumented invalid fixture expectation: {path.name}")
            self.validate(document, schema, path.name, expected=accepted)
            self.counts["protocol_examples"] += 1
            self.counts["protocol_examples_schema_accepted" if accepted else "protocol_examples_schema_rejected"] += 1
            if path.name in contextual:
                self.counts["contextually_invalid_examples_accepted_structurally"] += 1

    def corpus(self, temporary: Path):
        fixture_root = ROOT / "fixtures/conformance"
        cases = read_json(fixture_root / "manifest.json")["cases"]
        if len(cases) < 79:
            raise AssertionError("conformance validation unexpectedly lost seed cases")
        for number, case in enumerate(cases):
            project = temporary / f"corpus-{number:03}"
            project.mkdir()
            sources = [case["source"], *case.get("contracts", [])]
            for relative in sources:
                data = (fixture_root / relative).read_bytes()
                match = re.match(rb"\s*module\s+([A-Za-z_][A-Za-z_0-9:]*)\s*;", data)
                if not match:
                    raise AssertionError(f"fixture has no usable module header: {relative}")
                module = match.group(1).decode("ascii").replace("::", "/")
                is_contract = relative.endswith(".pact.ptn")
                destination = project / (f"contracts/{module}.pact.ptn" if is_contract else f"src/{module}.ptn")
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(data)
            self.cli(["check"], project, codes=(0, 1), corpus=True)
            self.cli(["fmt", "--check"], project, codes=(0, 1), corpus=True)
            self.counts["corpus_cases"] += 1

    def extra_cli(self, temporary: Path):
        project = temporary / "cli"
        (project / "src").mkdir(parents=True)
        source = project / "src/main.ptn"
        source.write_text("module main; fn helper()->I64{return 1;} pub fn main()->I64{return helper();}\n", encoding="utf-8")
        for args, codes in [
            (["version"], (0,)), (["--version"], (0,)), (["--help"], (0,)),
            (["help", "check"], (0,)), (["check"], (0,)),
            (["fmt", "--check"], (1,)), (["fmt"], (0,)), (["fmt", "--check"], (0,)),
            (["not-a-command"], (2,)), (["--protocol-version", "999.0.0", "check"], (2,)),
            (["run", "--args", '["unexpected"]'], (2,)),
        ]:
            self.cli(args, project, codes=codes)
        empty = temporary / "empty"
        empty.mkdir()
        self.cli(["check"], empty, codes=(1,))
        query_schema = SPEC / "semantic-query-response.schema.json"
        inspect = self.cli(["inspect", "helper"], project, schema=query_schema)
        self.cli(["callers", "helper"], project, schema=query_schema)
        change = {
            "protocol_version": "0.1.0", "kind": "change_request", "request_id": "schema-impact",
            "transaction": "implementation", "base_revision": inspect["source_revision"],
            # Source-only impact has no trusted baseline; this valid digest is
            # structural request data, not an invented approval or policy result.
            "expected_policy_baseline": "sha256:" + "0" * 64,
            "operations": [{"op": "replace_function_body", "target": {
                "symbol_id": inspect["result"]["id"],
                "expected_hash": inspect["result"]["declaration_hash"],
            }, "body": "{ return 2; }"}],
        }
        self.validate(change, SPEC / "change-request.schema.json", "generated CLI impact request")
        self.counts["generated_change_requests"] += 1
        change_path = project / "change.json"
        change_path.write_text(json.dumps(change), encoding="utf-8")
        self.cli(["impact", "--change", str(change_path)], project, schema=query_schema)
        editor_schema = SPEC / "editor-response.schema.json"
        self.cli(["editor"], project, schema=editor_schema)
        call_byte = source.read_bytes().rfind(b"helper()")
        self.cli(["editor", "--path", "src/main.ptn", "--byte", str(call_byte)], project, schema=editor_schema)
        self.cli(["editor", "--path", "src/main.ptn", "--byte", "0"], project, schema=editor_schema)
        source.write_text("module main; pub fn main()->I64{return false;}\n", encoding="utf-8")
        self.cli(["editor"], project, schema=editor_schema)

    def captures(self, directory: Path):
        names = {"semantic-query-request", "semantic-query-response", "editor-request", "editor-response"}
        paths = sorted(directory.glob("*.json"))
        if len(paths) < 14:
            raise AssertionError(f"expected at least 14 real query request/response captures, got {len(paths)}")
        for path in paths:
            capture = read_json(path)
            if set(capture) != {"schema", "document"} or capture["schema"] not in names:
                raise AssertionError(f"invalid query capture envelope: {path}")
            self.validate(capture["document"], SPEC / f"{capture['schema']}.schema.json", path.name)
            self.counts["query_capture_documents"] += 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, default=ROOT / "target/debug/pactern")
    parser.add_argument("--no-build", action="store_true", help="use the existing CLI binary")
    parser.add_argument("--capture-dir", type=Path, help="validate existing real query captures instead of regenerating them")
    parser.add_argument("--evidence", type=Path, help="validate an existing workflow evidence file instead of rerunning the current example")
    parser.add_argument("--report", type=Path, default=ROOT / "target/protocol-validation.json")
    args = parser.parse_args()
    installed = version("jsonschema")
    if installed != REQUIRED_JSONSCHEMA:
        parser.error(f"use jsonschema=={REQUIRED_JSONSCHEMA}; installed version is {installed}")
    if not args.no_build:
        command(["cargo", "build", "-p", "pactern-cli", "--bin", "pactern", "--locked"])
    executable = args.cli.resolve(strict=True)
    validation = Validation(executable)
    validation.protocol_examples()
    with tempfile.TemporaryDirectory(prefix="pactern-protocol-") as directory:
        temporary = Path(directory)
        validation.corpus(temporary)
        validation.extra_cli(temporary)
        print(f"Validated {validation.counts['cli_documents']} emitted CLI documents.", file=sys.stderr, flush=True)
        if args.capture_dir:
            captures = args.capture_dir.resolve(strict=True)
        else:
            captures = temporary / "query-captures"
            env = os.environ.copy()
            env["PACTERN_QUERY_CAPTURE_DIR"] = str(captures)
            command(["cargo", "test", "-p", "pactern", "--test", "query_protocol", "--locked"], env=env)
        validation.captures(captures)
        if args.evidence:
            evidence_path = args.evidence.resolve(strict=True)
        else:
            workflow = temporary / "quota-workflow"
            command(["cargo", "run", "-p", "pactern", "--example", "quota_workflow", "--locked", "--", str(workflow)])
            evidence_path = workflow / "evidence.json"
        evidence = read_json(evidence_path)
        validation.validate(evidence, ROOT / "docs/evidence.schema.json", "actual quota workflow evidence")
        validation.counts["workflow_evidence_documents"] += 1
        workflow_hash = hashlib.sha256(evidence_path.read_bytes()).hexdigest()
    report = {
        "validator": f"jsonschema/{installed}",
        "counts": dict(sorted(validation.counts.items())),
        "document_total": sum(validation.counts[key] for key in ["protocol_examples", "cli_documents", "query_capture_documents", "generated_change_requests", "workflow_evidence_documents"]),
        "schema_hashes": {str(path.relative_to(ROOT)): "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest() for path in validation.schema_paths},
        "cli_binary_hash": "sha256:" + hashlib.sha256(executable.read_bytes()).hexdigest(),
        "workflow_evidence_hash": "sha256:" + workflow_hash,
        "context_note": "Stale and protected protocol examples pass structure; contextual rejection is covered by Rust transaction tests.",
    }
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"validated": True, "counts": report["counts"], "document_total": report["document_total"], "report": str(args.report)}, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, RuntimeError, OSError, ValueError, PackageNotFoundError, subprocess.TimeoutExpired) as error:
        sys.exit(f"protocol validation failed: {error}")

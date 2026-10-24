#!/usr/bin/env python3
"""Bounded, synthetic-only A/B measurements of actual local CLI processes.

No source discovery, existing roots, credentials, registration or network client.
Temporary encrypted roots and their keys are deleted when the command exits.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import tempfile
import time


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def private(path, data):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.write_bytes(data)
    path.chmod(0o600)


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def token(index):
    return f"needle{index:08}"


def text(index):
    return f"Synthetic battery history {token(index)} λ\r\noriginal\x00終"


def row(index):
    return dict(agent_id="synthetic-agent", parent_id=None, valid_at=index,
                kind="explicit", tool=None, status=None, exit=None, key=None,
                text=text(index), attrs={"source_id": f"synthetic-{index}"}, indexed=True)


def message(index):
    return encoded(dict(type="response_item", timestamp="2026-01-02T03:04:06Z",
                        payload=dict(type="message", role="assistant",
                                     content=[dict(type="output_text", text=text(index))]))) + b"\n"


def summary(samples):
    ordered = sorted(samples)
    return dict(count=len(samples), p50_ms=statistics.median(samples),
                p95_ms=ordered[math.ceil(len(samples) * .95) - 1],
                min_ms=ordered[0], max_ms=ordered[-1], samples_ms=samples)


def storage(root):
    files = [path.stat() for path in root.rglob("*") if path.is_file()]
    return dict(logical_bytes=sum(info.st_size for info in files),
                allocated_bytes=sum(info.st_blocks * 512 for info in files))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--sizes", default="1000,10000")
    parser.add_argument("--samples", type=int, default=20)
    args = parser.parse_args()
    sizes = [int(size) for size in args.sizes.split(",")]
    if not sizes or any(size < 20 or size > 10000 for size in sizes):
        parser.error("sizes must be between 20 and 10000")
    if not 20 <= args.samples <= 100:
        parser.error("samples must be between 20 and 100")
    binaries = {name: path.resolve(strict=True) for name, path in
                [("baseline", args.baseline), ("candidate", args.candidate)]}
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("AWS_", "OOJURO_"))}
    env.update(AWS_CONFIG_FILE="/dev/null", AWS_SHARED_CREDENTIALS_FILE="/dev/null",
               AWS_EC2_METADATA_DISABLED="true")

    def call(name, root, command, *options):
        start = time.perf_counter_ns()
        result = subprocess.run([str(binaries[name]), command, "--root", str(root),
                                 *map(str, options)], env=env, capture_output=True, timeout=60)
        elapsed = (time.perf_counter_ns() - start) / 1_000_000
        if result.returncode:
            raise RuntimeError(f"synthetic {name} {command} refused: {result.stderr.decode()}")
        return json.loads(result.stdout), elapsed

    report = dict(schema=1, scope="synthetic local CLI subprocess wall time; warm OS cache",
                  order=["baseline", "candidate", "candidate", "baseline"],
                  platform=platform.platform(), machine=platform.machine(),
                  binaries={name: dict(sha256=digest(path), bytes=path.stat().st_size)
                            for name, path in binaries.items()}, sizes=sizes, runs=[])
    generation = "34343434343434343434343434343434"
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="oojuro-journal-perf-synthetic-") as directory:
        scratch = Path(directory)
        for size in sizes:
            seed = scratch / f"record-seed-{size}"
            call("baseline", seed, "record-setup", "--writer", "synthetic-writer",
                 "--owner", 7, "--adapter", "synthetic-benchmark")
            input_path = scratch / "typed-input.jsonl"
            for first in range(0, size, 1000):
                private(input_path, b"\n".join(encoded(row(i)) for i in
                                             range(first, min(first + 1000, size))) + b"\n")
                call("baseline", seed, "record-append", "--input", input_path,
                     "--input-format", "jsonl", "--request", f"seed-{first}",
                     "--allow-unredacted-records")
            expected_hits = {}
            for number, name in enumerate(report["order"]):
                run_dir = scratch / f"run-{size}-{number}"
                run_dir.mkdir(mode=0o700)
                root = run_dir / "records"
                shutil.copytree(seed, root)
                other = "candidate" if name == "baseline" else "baseline"
                timings = {key: [] for key in ["record_append_one", "record_get",
                                               "record_search", "capture_append_one",
                                               "capture_get", "capture_search",
                                               "capture_search_cached"]}
                for sample in range(args.samples):
                    index = size + sample
                    private(input_path, encoded([row(index)]))
                    ack, elapsed = call(name, root, "record-append", "--input", input_path,
                                        "--request", f"append-{sample}", "--allow-unredacted-records")
                    timings["record_append_one"].append(elapsed)
                    assert ack["ids"] == [index + 1]
                    stored, _ = call(other, root, "record-get", "--id", index + 1)
                    assert all(stored[key] == value for key, value in row(index).items())
                    if sample == 0:
                        retry, _ = call(name, root, "record-append", "--input", input_path,
                                        "--request", "append-0", "--allow-unredacted-records")
                        assert retry == ack
                for sample in range(args.samples):
                    index = sample * (size - 1) // (args.samples - 1)
                    stored, elapsed = call(name, root, "record-get", "--id", index + 1)
                    timings["record_get"].append(elapsed)
                    assert all(stored[key] == value for key, value in row(index).items())
                    found, elapsed = call(name, root, "record-search", "--query", token(index), "--k", 10)
                    timings["record_search"].append(elapsed)
                    assert [hit["record"]["id"] for hit in found["hits"]] == [index + 1]
                    signature = [(hit["record"]["id"], hit["bits"]) for hit in found["hits"]]
                    key = ("record", index)
                    assert expected_hits.setdefault(key, signature) == signature
                captured = run_dir / "capture"
                source = run_dir / ".codex" / "rollout-synthetic.jsonl"
                policy = run_dir / "policy.json"
                private(policy, encoded(dict(version=1, record=True, paused=False,
                                            record_thinking=True, index_thinking=True,
                                            exclude=[], denied_tools=[])))
                original = encoded(dict(type="session_meta", payload=dict(
                    cwd="/synthetic", originator="codex_cli_rs", base_instructions={"text": ""}))) + b"\n"
                original += b"".join(message(index) for index in range(size))
                private(source, original)
                call(name, captured, "setup", "--harness", "codex", "--writer", "codex:-synthetic",
                     "--agent", 7, "--policy", policy)
                receipt, import_ms = call(name, captured, "capture", "--source", source,
                                          "--source-id", generation, "--allow-raw-source")
                assert len(receipt["event_ids"]) == size
                for sample in range(args.samples):
                    index = size + sample
                    addition = message(index)
                    with source.open("ab") as output:
                        output.write(addition)
                        output.flush()
                        os.fsync(output.fileno())
                    original += addition
                    receipt, elapsed = call(name, captured, "capture", "--source", source,
                                            "--source-id", generation, "--allow-raw-source")
                    timings["capture_append_one"].append(elapsed)
                    assert receipt["raw_end"] == len(original)
                cache = run_dir / "lexical-cache"
                call(name, captured, "search", "--query", token(0), "--cache", cache)
                for sample in range(args.samples):
                    index = sample * (size - 1) // (args.samples - 1)
                    stored, elapsed = call(name, captured, "get", "--id", index + 1)
                    timings["capture_get"].append(elapsed)
                    assert stored["output"]["event"]["text"] == text(index)
                    found, elapsed = call(name, captured, "search", "--query", token(index))
                    timings["capture_search"].append(elapsed)
                    assert [hit["id"] for hit in found["hits"]] == [index + 1]
                    key = ("capture", index)
                    assert expected_hits.setdefault(key, found["hits"]) == found["hits"]
                    cached, elapsed = call(name, captured, "search", "--query", token(index), "--cache", cache)
                    timings["capture_search_cached"].append(elapsed)
                    assert cached["cache"] == "Loaded" and cached["hits"] == found["hits"]
                restored = run_dir / "restored.jsonl"
                call(other, captured, "restore-raw", "--source-id", generation,
                     "--out", restored, "--allow-raw-export")
                assert restored.read_bytes() == original == source.read_bytes()
                report["runs"].append(dict(size=size, order=number, binary=name,
                                          capture_initial_import_ms=import_ms,
                                          record_storage=storage(root), capture_storage=storage(captured),
                                          capture_raw_sha256=hashlib.sha256(original).hexdigest(),
                                          correctness="sampled typed fields, exact retry ACK, original text, exact raw restoration, lexical IDs/scores, cross-binary reopen",
                                          timings={key: summary(values) for key, values in timings.items()}))
                print(f"qualified {size} {number} {name}", flush=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()

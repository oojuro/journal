#!/usr/bin/env python3
"""Measure actual Journal owner queries on owned synthetic Event sessions.

The private owner framing is a qualification fixture, not a public client API.
All fixture files, processes and sockets are removed before the result is printed.
Run under the repository's shared benchmark lock and use a release binary.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time


def original(number):
    header = f"record{number:06d} topic{number % 17} "
    if number % 10000 == 0:
        header += "needlecobalt "
    body = header + "Exact synthetic original\0\r\n" + (
        "transaction recovery durable search context session " * 24
    )
    return body[:1024]


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int((len(ordered) - 1) * fraction))]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--rows", type=int, choices=[1000, 10000, 100000], default=100000)
    parser.add_argument("--repeats", type=int, default=20)
    parser.add_argument("--plaintext", action="store_true")
    args = parser.parse_args()
    assert 3 <= args.repeats <= 100
    binary = args.binary.resolve()
    result = {"format": 1, "synthetic_only": True, "encrypted": not args.plaintext,
              "fixture_rows": args.rows, "original_bytes_per_fixture_row": 1024,
              "scope": "One Event session; actual private owner IPC and complete original materialization. No cross-session, transcript ingestion throughput, or OS-cache-cold claim."}
    with tempfile.TemporaryDirectory(prefix="ojo-search-qualification-") as temporary:
        scratch = Path(temporary)
        scratch.chmod(0o700)
        root = scratch / "root"

        def cli(verb, *options):
            call = subprocess.run([str(binary), verb, "--root", str(root), *options],
                                  capture_output=True, text=True, timeout=600)
            assert call.returncode == 0, call.stderr
            return json.loads(call.stdout)

        cli("init", *(["--plaintext"] if args.plaintext else []))
        seed = scratch / "seed.txt"
        seed.write_text("Synthetic seed, outside timed queries.")
        run = cli("record", "--session-id", "qualification-only", "--event-id", "seed",
                  "--text-file", str(seed))["session"]
        template = cli("get", "--session", run, "--id", "0")
        identity = json.loads((root / "root.json").read_text())["identity"]
        source = root / "journals" / f"{run}.juro"
        owner = None
        endpoint = None
        owner_log = open(scratch / "owner.log", "ab")

        def start():
            nonlocal owner, endpoint
            started = time.perf_counter_ns()
            owner = subprocess.Popen([str(binary), "serve", "--root", str(root)],
                                     stdout=owner_log, stderr=owner_log)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                assert owner.poll() is None, "owner exited; inspect synthetic owner log"
                status = cli("status")
                if (status.get("owner") or {}).get("healthy"):
                    endpoint = Path(status["owner"]["socket"])
                    return (time.perf_counter_ns() - started) / 1e6
                time.sleep(0.02)
            raise AssertionError("owner startup timed out")

        def stop():
            nonlocal owner
            if owner is not None:
                owner.terminate()
                try:
                    owner.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    owner.kill()
                    owner.wait(timeout=10)
                owner = None
            if endpoint is not None:
                endpoint.unlink(missing_ok=True)

        def rpc(operation, **fields):
            request = {"format": 1, "root": identity,
                       "action": {"operation": operation, **fields}}
            payload = json.dumps(request, separators=(",", ":")).encode()
            with socket.socket(socket.AF_UNIX) as stream:
                stream.settimeout(600)
                stream.connect(str(endpoint))
                stream.sendall(struct.pack(">I", len(payload)) + payload)

                def receive(length):
                    pieces = bytearray()
                    while len(pieces) < length:
                        data = stream.recv(length - len(pieces))
                        assert data, "owner closed before a complete response"
                        pieces.extend(data)
                    return pieces

                length = struct.unpack(">I", receive(4))[0]
                assert 0 < length <= 128 << 20
                response = json.loads(receive(length))
                assert response["format"] == 1 and response["root"] == identity
                assert response["error"] is None, response["error"]
                return response["value"]

        def records(first, count):
            batch = []
            for number in range(first, first + count):
                record = copy.deepcopy(template)
                record["event_id"] = hashlib.sha256(f"fixture-{number}".encode()).hexdigest()
                record["input"]["text"] = original(number)
                record["input"]["valid_at"] = number
                batch.append(record)
            return batch

        def search(query):
            started = time.perf_counter_ns()
            hits = rpc("Search", run=run, query=query, limit=10)
            elapsed = (time.perf_counter_ns() - started) / 1e6
            assert hits, f"missing matches for {query}"
            for record in hits:
                number = record["sequence"] - 1
                assert record["input"]["text"] == original(number)
                assert query in record["input"]["text"]
            return elapsed

        def counters():
            status = rpc("Hello")
            return {key: status[key] for key in ["handle_capacity", "open_handles",
                    "verified_opens", "search_projection_capacity", "search"]}

        try:
            start()
            began = time.perf_counter_ns()
            for first in range(0, args.rows, 100):
                assert rpc("Append", run=run, request=f"qualification-{first}",
                           records=records(first, 100)) == 100
            result["grouped_owner_population_ms"] = (time.perf_counter_ns() - began) / 1e6
            result["source_bytes_before_index_build"] = source.stat().st_size
            result["initial_index_build_and_query_ms"] = search("needlecobalt")
            samples = []
            for index in range(args.repeats):
                query = ["needlecobalt", "transaction", f"record{args.rows // 2:06d}"][index % 3]
                samples.append({"query": query, "ms": search(query)})
            result["warm_owner_queries"] = samples
            result["warm_owner_p50_ms"] = percentile([x["ms"] for x in samples], 0.5)
            result["warm_owner_p95_ms"] = percentile([x["ms"] for x in samples], 0.95)
            cli_samples = []
            for _ in range(5):
                started = time.perf_counter_ns()
                matches = cli("search", "--session", run, "--query", "needlecobalt")["matches"]
                cli_samples.append((time.perf_counter_ns() - started) / 1e6)
                assert matches
                for record in matches:
                    assert record["input"]["text"] == original(record["sequence"] - 1)
            result["warm_public_cli_ms"] = cli_samples
            result["owner_before_reopen"] = counters()
            assert rpc("Append", run=run, request="qualification-increment",
                       records=records(args.rows, 100)) == 100
            result["incremental_refresh_and_query_ms"] = search(f"record{args.rows:06d}")
            result["owner_after_refresh"] = counters()
            stop()
            result["restart_until_healthy_ms"] = start()
            result["first_query_after_restart_ms"] = search("needlecobalt")
            result["restart_and_first_query_ms"] = (result["restart_until_healthy_ms"]
                                                    + result["first_query_after_restart_ms"])
            result["owner_after_reopen"] = counters()
            rpc("Finish", run=run, finished=True)
            result["source_bytes_before_archive"] = source.stat().st_size
            began = time.perf_counter_ns()
            rpc("Archive", run=run)
            result["archive_ms"] = (time.perf_counter_ns() - began) / 1e6
            result["source_bytes_after_archive"] = source.stat().st_size
            result["archived_reopen_and_query_ms"] = search("needlecobalt")
            result["owner_after_archive"] = counters()
            result["source_rows"] = rpc("Manifest", run=run)["records"]
            assert result["source_rows"] == args.rows + 101
        finally:
            stop()
            owner_log.close()
    result["owned_dataset_and_processes_removed"] = True
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()

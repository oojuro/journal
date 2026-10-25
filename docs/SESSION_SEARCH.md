# Session search

New Event sessions use the engine's persistent lexical index. `ojo-journal search
--session ID --query TEXT` and the local read-only MCP search return complete
recorded originals. Journal applies its stored indexing eligibility and chooses
the session; the engine ranks generic text rows and verifies the selected
originals through Journal's source callback. Unindexed originals remain available
through get, context, timeline and export.

The index lives under the private root's `search/SESSION` directory. It is an
encrypted disposable view, bound to the root identity, session and authenticated
ordered original intents. The encryption key is derived from the root key;
plaintext roots keep a separately protected local search key. The source file
remains authoritative. Missing, damaged or stale caches rebuild from validated
originals, without changing their bytes, cursors or timestamps. A caller lease
spans all retained index generations and rebuilds.

The healthy owner retains up to eight source handles and one numeric search
projection. A warm query reuses that projection and materializes only selected
originals. New immutable indexed originals extend the existing index; lifecycle
metadata and archive conversion preserve its binding. Searching another session
releases the previous session's in-memory projection. Its encrypted index remains
on disk for a checked reopen. Status reports index bytes and estimated index heap;
the estimate is not whole-process RSS.

The first query builds synchronously. Reopening a source validates its complete
engine history before using a persisted index. These operations take longer and
currently share the owner's database mutex with capture. Query/index admission
is bounded: three million rows, 64 GiB source text, 8 GiB index files, 256 MiB
estimated projection metadata, 32 query terms, 100 returned records and 16 MiB
original output. A limit refuses the operation; it does not silently remove
originals or narrow the source corpus.

Search without `--session` still visits sessions in recent-activity order and
returns bounded matches from each. Relevance ranking is within each Event
session. It is not a globally ranked root index, and switching across many
sessions can require repeated source/index opens. Existing KV sessions retain
their compatible substring search until explicit `migrate-session` conversion.
No cross-session or cold million-record latency claim follows from a warm
single-session result.

`scripts/qualify-journal-search.py --binary /path/to/release/ojo-journal` measures
one owned synthetic session through the actual owner IPC. It populates grouped
application writes, checks exact original context, measures build/warm queries,
incremental refresh, process reopen and archive/reopen, then removes every fixture
and child process. It does not measure transcript ingestion throughput or flush
the operating system's caches. Run it under the shared benchmark lock. Source
qualification and published installation are separate operator steps.

The [local qualification receipt](qualification/ojo-journal-search-2026-10-25.json)
records a release-binary run on an Intel Core Ultra 7 258V with the shared
benchmark lock. Each scenario initially has 100,000 synthetic 1 KiB originals
plus one seed, then appends 100 more originals. Twenty warm IPC queries mix a
rare term, common term and exact identifier; every returned original is checked
against the independently generated fixture. Five public CLI calls also verify
complete original text.

| Operation | Encrypted source | Plaintext source |
| --- | ---: | ---: |
| Warm owner query, p50 / p95 | 2.66 / 6.19 ms | 2.95 / 4.40 ms |
| Warm public CLI, five samples | 3.80–5.03 ms | 2.43–5.62 ms |
| Initial index build and query | 18.70 s | 18.35 s |
| Append 100 rows, refresh and query | 144.82 ms | 174.16 ms |
| Owner restart through healthy status and first query | 811.37 ms | 1,267.33 ms |
| Archived source reopen and query | 824.10 ms | 673.45 ms |

After the increment, the source contains 100,101 records. Archiving changes the
encrypted source from 193,568,606 to 50,003,590 bytes, and the plaintext source
from 189,484,166 to 45,927,474 bytes. Derived index files are additional: about
71.1 MB after that refresh, with about 36.4 MB estimated index heap. These are
application Event/control/index measurements; they do not establish a storage
ratio for a generic database or total process memory. Process restart includes
the service's startup source checks; OS caches were not flushed. Both datasets
and their service processes were removed after qualification.

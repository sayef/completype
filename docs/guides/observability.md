# Observability

An engine reports the version it serves and how its background sync went. Beyond that, completype reports
what it does through [`tracing`](https://docs.rs/tracing) events: commits, syncs, ingest rounds,
compaction, cleanup and segment builds. Searches emit no events, so logging never costs anything on the
hot path.

## Engine status

```python
import time

import completype

db = completype.connect("./data")
txn = db.begin()
txn.append("songs", [{"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9}])
txn.commit()

engine = db.engine(sync_every=0.5)
print(engine.sync_status)
engine.complete("bill", ["songs"])   # starts the background sync
time.sleep(1)
print(engine.sync_status)
print(engine.version, db.latest_version())
```

```text
None
{'synced_at': 1791145251.237, 'error': None}
1 1
```

| Value | Meaning |
|---|---|
| `engine.version` | The database version the engine serves. |
| `engine.sync_status` | `None` before the first background sync; then `synced_at`, when the last one finished in seconds since the Unix epoch, and `error`, its error or `None`. |
| `db.latest_version()` | The latest version in storage. It reads storage, so call it from health checks, not per request. |

A serving process is healthy when `sync_status["error"]` is `None` and `synced_at` is recent. A failed
sync keeps serving the version the engine has; a growing gap between `db.latest_version()` and
`engine.version` means it is falling behind.

In Rust, `Follower::status` returns `FollowStatus { synced_at_ms, error }`, and `Replica::version` the
version served.

A [collection](collections.md#stats) reports the same and more, including segment counts and the last
merge error, through `stats()`.

## Events

| Target | Level | Message | Fields |
|---|---|---|---|
| `completype::database` | info | `committed` | `version`, `attempts`, `segments` |
| `completype::database` | warn | `commit gave up under contention` | `attempts` |
| `completype::database` | info | `replica synced` | `version`, `segments_loaded`, `ms` |
| `completype::database` | info | `compacting` | `index`, `merged`, `level`, `full`, `documents`, `ms` |
| `completype::database` | info | `cleaned up` | `versions`, `segments`, `bytes` |
| `completype::inbox` | info | `ingested` | `version`, `change_sets`, `documents` |
| `completype::segment` | debug | `built segment` | `documents`, `bytes`, `ms` |

`replica synced` is the event to watch on serving processes: `ms` is the time a sync took, and
`segments_loaded` how many segments it downloaded or mapped. On writing processes, `committed` and
`compacting` show write and merge cost; on [ingestors](writers.md), `ingested` shows throughput.

## Python

In Python, the events reach the standard `logging` module under loggers named `completype.*`
(`completype.database`, `completype.inbox`, `completype.segment`), with the fields in the message. Configure
them like any other logger:

```python
import logging

import completype

logging.basicConfig(format="%(asctime)s %(name)s %(levelname)s %(message)s")
logging.getLogger("completype").setLevel(logging.INFO)

db = completype.connect("./logged")
txn = db.begin()
txn.append("songs", [{"id": "billie", "text": "Billie Jean – Michael Jackson"}])
txn.commit()
engine = db.engine()
```

```text
2026-10-04 22:20:50,660 completype.database INFO committed version=1 attempts=1 segments=1
2026-10-04 22:20:50,663 completype.database INFO replica synced version=1 segments_loaded=1 ms=0
```

Set the `completype` logger to `DEBUG` to include segment builds.

!!! note "Configure logging before the first completype call"
    The bridge to `logging` caches each logger's level the first time that logger is used. Set levels at
    start-up, before completype logs anything; a level changed later may not take effect for loggers already
    used.

## Rust

In Rust, install any `tracing` subscriber. For example, with `tracing-subscriber` and its `env-filter`
feature:

```rust
tracing_subscriber::fmt()
    .with_env_filter("completype=info")
    .init();
```

## Command line

The `completype` tool logs to stderr at `completype=info`. Set `COMPLETYPE_LOG` to change the filter:

```sh
COMPLETYPE_LOG=completype=debug completype s3://my-bucket/completions ingest
COMPLETYPE_LOG=completype=warn completype s3://my-bucket/completions cleanup
```

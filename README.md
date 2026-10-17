# fldr

**fldr** (short for "folder") is centralized, immutable, content-addressed
source control for agent workspaces and folders. It is a Rust service with a
thin client, and the server owns all history. The design is inspired by
Anthropic's Scion.

The server stores immutable blobs, trees and changesets. It computes commits,
lands, logs, diffs and ancestry. The client lays out files, keeps a bounded
per-machine object cache and provides the CLI. No full copy of a repository is
materialized anywhere, and every query is an HTTP/JSON API, so an agent can
read and write without a checkout.

fldr is **not a git replacement**. It targets the many folders that benefit
from history and sharing without git's workflow: agent workspaces, notes,
generated artifacts and scratch projects. Git import is deferred.

## Status

**M0: scaffold.** A server with a live `/health` endpoint runs at
<https://fldr.etdofresh.com>. The data model, blob store, metadata schema and
`fldr health` CLI command are being built in M0. Everything else described
here, including commits, landing, checkout and the full API, is **planned**.
See [`docs/PLAN.md`](docs/PLAN.md) for the design and milestones.

## Core concepts

- **Blob:** a file's bytes, identified by their plain SHA-256 (`sha256sum`
  gives the blob id).
- **Tree:** one directory level, a sorted map from name to a file entry
  (blob, size, kind) or a subdirectory tree. Trees are hashed over a canonical
  JSON encoding.
- **Changeset:** a unit of history. It lists only the paths that changed
  relative to its first parent (p1), plus its parents, actor, message and
  time. It never contains a full tree.
- **Ref:** a named pointer to a changeset, such as `main` or a release ref.
  A ref moves only by a compare-and-swap (CAS), and every move is recorded in
  an append-only ref log.
- **Land:** moving `main` to include a changeset. The server rebases
  non-overlapping changes itself and rejects overlapping ones with the
  conflicting paths. It takes no global lock.
- **Derived data:** indexes such as root trees, ancestry skiplists and path
  history. Each is computed from the same index on the parent changesets.
- **Actor and why:** every changeset records who made it (human, agent or
  service, with optional agent and session) and a message explaining why.

## Workspace layout

| Crate | Purpose |
|---|---|
| [`crates/fldr-core`](crates/fldr-core) | Pure data model with no I/O: hashes and typed ids, canonical encoding, trees, changesets, wire types. |
| [`crates/fldr-store`](crates/fldr-store) | `BlobStore` trait with in-memory and filesystem implementations. |
| [`crates/fldr-server`](crates/fldr-server) | HTTP server (axum, sqlx/Postgres) with an in-memory metadata fallback. |
| [`crates/fldr-cli`](crates/fldr-cli) | The `fldr` command-line client. |

## Quick start

Requires stable Rust (edition 2024).

```sh
cargo build

# Run the server. It binds 0.0.0.0:8080 and keeps metadata in memory
# because DATABASE_URL is unset.
cargo run -p fldr-server

# In another shell:
cargo run -p fldr-cli -- health --host http://localhost:8080
curl -s http://localhost:8080/health
```

Without `--host` (or `FLDR_HOST`), the CLI talks to
`https://fldr.etdofresh.com`.

### Server environment variables

| Variable | Meaning |
|---|---|
| `PORT` | Listen port. Default `8080`. |
| `BIND` | Listen address. Default `0.0.0.0`. |
| `DATABASE_URL` | Postgres URL. Optional; metadata is kept in memory when unset. |
| `DATA_DIR` | Data directory. Default `./.fldr-data`; `/data` in Docker. Blobs go in `$DATA_DIR/blobs`. |
| `RUST_LOG` | Log filter. Default `info`. |
| `FLDR_LOG_FORMAT` | `json` or `pretty`. Docker uses `json`. |

The HTTP API is documented in [`docs/api.md`](docs/api.md).

## Planned CLI

None of these commands exist yet. This is the intended shape:

```sh
fldr clone etdofresh/notes              # sparse checkout of main; fetches only what it needs
fldr status                             # local edits against the checked-out tree
fldr commit -m "why this change"        # upload missing blobs, create a changeset, land it
fldr log                                # history of main, with actor and message
fldr diff                               # working copy against its base, or between changesets
fldr cat etdofresh/notes@main:todo.md   # read a file without a checkout
```

## Credits

The design follows Anthropic's engineering post
[Redesigning source control](https://www.anthropic.com/engineering/redesigning-source-control)
(October 2026), which describes Scion, their internal replacement for git.
Scion has not been released. fldr is an independent personal and educational
reimplementation of its published ideas and has no affiliation with
Anthropic. It also draws on the "Bonsai" changeset model of Meta's
[Sapling](https://sapling-scm.com/) and its server, Mononoke, which Scion
credits as prior art. [`docs/scion-lessons.md`](docs/scion-lessons.md)
summarizes the article and maps each lesson to a fldr decision.

An earlier concept for fldr (May 2026) was a continuous file-sync tool with a
"why" on each file operation. It is preserved in git history at commit
`cef4711`; [`docs/PLAN.md`](docs/PLAN.md) explains what was kept and what was
dropped.

## License

TBD.

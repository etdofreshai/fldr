# fldr design plan

This plan describes fldr as a centralized, immutable, content-addressed
source-control service written in Rust and modelled on Anthropic's Scion. It
supersedes the May 2026 plan for an op-streaming sync tool. That plan remains
in git history at commit `cef4711` (`git show cef4711:docs/PLAN.md`).

Unless a section says otherwise, everything past the M0 scaffold is
**planned**, not built. For background, see
[scion-lessons.md](scion-lessons.md). The HTTP API is in [api.md](api.md).

## Contents

1. [Goals and non-goals](#1-goals-and-non-goals)
2. [What changed since the May 2026 plan](#2-what-changed-since-the-may-2026-plan)
3. [Scion lesson → fldr decision](#3-scion-lesson--fldr-decision)
4. [Architecture](#4-architecture)
5. [Data model](#5-data-model)
6. [Metadata schema](#6-metadata-schema)
7. [API sketch](#7-api-sketch)
8. [Client model](#8-client-model)
9. [Landing and conflicts](#9-landing-and-conflicts)
10. [Derived data](#10-derived-data)
11. [Security](#11-security)
12. [Deployment](#12-deployment)
13. [Milestones](#13-milestones)
14. [Open questions](#14-open-questions)

## 1. Goals and non-goals

**Goals**

- Give agent workspaces and ordinary folders cheap, explicit history with an
  actor and a "why" on every change.
- Keep the client thin. The server owns history and every expensive
  operation; the client lays out files and caches objects.
- Make every object immutable and content-addressed, so any cache tier can
  serve it and there is no repository maintenance.
- Expose every query as an HTTP/JSON API, so agents can read and write
  without a checkout.
- Land changes without a global lock.
- Run on one small server for personal use, with room to scale out later.

**Non-goals**

- Replacing git, or interoperating with git remotes. Git import is deferred.
- Offline operation. Clients are assumed to be online.
- Distributed or federated hosting. There is one authoritative server.
- Branch-heavy workflows. The only refs are `main` and release refs.
- Real-time co-editing or automatic sync on every file write.

## 2. What changed since the May 2026 plan

The May plan described a daemon that watched folders and streamed each file
operation (add, edit, delete, rename) to a hub, with last-writer-wins
conflicts and decentralized remote lookup.

**Kept**

- The name `fldr` and the `owner/repo` addressing model.
- The first workload: folders and agent workspaces rather than engineering
  monorepos. fldr is still not trying to replace git.
- Recording intent. The per-operation `why` becomes the changeset `message`,
  and the actor (human, agent or service, with optional agent and session
  ids) is part of every changeset.
- Content-addressed blob storage, deduplicated across repos.
- Token auth with public and private repos, and an MCP surface for agents.

**Dropped**

- Continuous op streaming and the file-watching daemon as the way history is
  made. History is now made by explicit `fldr commit -m "why"`.
- The linear per-repo op log. History is now a DAG of changesets.
- Last-writer-wins with `.conflict` sibling files. `land` now either rebases
  cleanly or returns 409 with the conflicting paths.
- Decentralization: remote lookup, multi-remote fan-out and self-hosted hubs.
  fldr is deliberately centralized.
- The per-folder `.fldr/state.db` that mirrored the op log. Clients keep only
  a bounded cache.

## 3. Scion lesson → fldr decision

| Scion lesson | fldr decision |
|---|---|
| Clients are always online and keep only a bounded cache | The CLI keeps no full copy. Each machine has one shared object cache with a size cap and LRU eviction, playing the role of scion-daemon. |
| Client–server asymmetry | The server owns commit, land and rebase, log, diff, blame, merge-base and ancestry. The client handles file layout, status, the cache and the UI. |
| Everything is an API | Every query is a JSON HTTP API. Agents can read and write without a checkout. A hosted MCP endpoint is planned. |
| No materialized repo anywhere | Storage holds only immutable blobs, trees and changesets: no packfiles and no maintenance. Apart from the blob store, the server is stateless. |
| Bonsai changesets (changes vs first parent) | A changeset holds `{path → put \| delete}` against p1, plus parents, actor, message and time. Its id is the SHA-256 of a canonical encoding. |
| A ref move is a single-row CAS, separate from changeset creation | Creating a changeset never touches refs. A ref moves via `UPDATE … WHERE target=$old`, and every move is written to an append-only `ref_log`. Refs are limited to `main` and release refs. |
| No global merge-queue lock | `land` is lock-free. If no path changed since the changeset's parent overlaps its own paths, the server rebases it and retries the CAS. Otherwise it returns 409 with the conflicting paths. Testing happens after landing. |
| Generic recursive derived data | `derive(kind, cs)` is computed from the parents' values and stored as `(repo, cs, kind, version)`, lazily or eagerly. The first kinds are `root_tree`, `skiplist` and `path_history`. |
| Want-only negotiation; any tier can cache | Clients request objects by hash only. Object routes send `Cache-Control: immutable`, so Cloudflare is a free cache tier for public repos. A later `fldr-cache` service would answer the same requests. |
| Sparse and virtual checkouts are native | A checkout is a root tree plus a list of sparse path prefixes. A FUSE-based virtual checkout comes later. |
| Per-directory ACLs | v1 has public and private repos plus scoped tokens. A `path_acl` table is designed now and enforced later. Blobs are readable only through a repo that references them (`repo_blobs`). |
| Hermetic simulation and fuzzing | `fldr-core` is pure and tested with `proptest`. A `MetaStore` trait has an in-memory and a Postgres implementation, so the server can be tested hermetically. |

## 4. Architecture

```
crates/fldr-core    pure: Hash, BlobId/TreeId/ChangesetId, RepoPath, Tree, Changeset,
                    Actor, canonical encoding and typed hashing, apply/diff/build_trees,
                    validation, JSON wire types
crates/fldr-store   BlobStore trait + MemBlobStore + FsBlobStore
                    (/data/blobs/ab/cd/<hex>, zstd, write to temp then rename);
                    S3BlobStore later
crates/fldr-server  axum + tokio + sqlx (Postgres, embedded migrations);
                    MetaStore trait (Postgres and in-memory); /health, /v1
crates/fldr-cli     clap; binary `fldr`
```

The server is a single process. Its durable state is in Postgres (metadata
and refs) and the blob store (file contents). Because every object is
immutable and addressed by hash, the server can cache freely and needs no
coordination with other servers to do so.

## 5. Data model

### 5.1 Ids and hashing

- `BlobId` is the plain SHA-256 of the file's bytes, so `sha256sum` on a local
  file gives its blob id.
- `TreeId` is `SHA-256("tree\0" || canonical(tree))`.
- `ChangesetId` is `SHA-256("changeset\0" || canonical(changeset))`.
- Every id is written as 64 lowercase hex characters. Typed ids keep a tree
  id from being used as a blob id by mistake.

The type prefixes give domain separation: a blob whose bytes happen to equal a
tree's canonical encoding still has a different id.

### 5.2 Canonical encoding

Trees and changesets are hashed over canonical JSON in the style of RFC 8785
(JCS):

- object keys sorted by UTF-16 code units;
- no insignificant whitespace;
- integers only, never floats, within ±(2^53−1);
- strings with minimal escaping (as `serde_json` produces);
- UTF-8 output.

### 5.3 Paths and file kinds

A `RepoPath` is a relative, `/`-separated UTF-8 path. It has no empty, `.` or
`..` components, no leading or trailing `/`, no NUL bytes and no backslashes.
Each component is at most 255 bytes and the whole path at most 4096 bytes.

A `FileKind` is `"file"`, `"exec"` or `"symlink"`. A symlink's blob holds the
link target.

### 5.4 Trees

A tree is one directory level: a sorted map from entry name to entry.

```json
{
  "README.md": { "type": "file", "blob": "<BlobId>", "size": 812, "kind": "file" },
  "src":       { "type": "dir",  "tree": "<TreeId>" }
}
```

Empty directories are never stored. A directory exists only while it contains
something.

### 5.5 Changesets

```jsonc
{
  "parents": ["<ChangesetId>"],          // p1 first; [] = root; 2+ = merge
  "changes": {                           // sorted by path, relative to p1
    "notes/todo.md": { "op": "put", "blob": "<BlobId>", "size": 120, "kind": "file" },
    "old.txt":       { "op": "delete" }
  },
  "actor":   { "kind": "agent", "name": "claude", "agent": "docs-worker", "session": "…" },
  "message": "Add todo list for M1",     // the "why"; at most 64 KiB
  "time":    1792195200                  // author time, unix seconds UTC (client-supplied)
}
```

- `actor.kind` is `"human"`, `"agent"` or `"service"`. `agent` and `session`
  are optional.
- Merges record changes against p1 only.
- Parents must not repeat.
- Landing time and the identity that moved the ref are recorded in `ref_log`,
  not in the changeset, so the changeset id does not depend on when or by whom
  it was landed.

### 5.6 Pure operations

`fldr-core` provides these without I/O:

- `FlatTree`, a map from `RepoPath` to file entry.
- `apply(changes)`. It fails when deleting a path that does not exist, when
  creating a file where a directory is, or when creating a file under an
  existing file.
- `diff(a, b)`, with the invariant `a.apply(diff(a, b)) == b`.
- `build_trees(flat)`, which deterministically returns the root `TreeId` and
  every `Tree` object, bottom-up.
- `paths_overlap(a, b)`, true when two change sets touch the same path or one
  path is an ancestor directory of another. `land` uses it to detect
  conflicts.

## 6. Metadata schema

Postgres, created by migration `0001_init.sql`.

| Table | Columns and notes |
|---|---|
| `repos` | `id`, `owner`, `name`, `visibility` (public or private), `created_at`. Unique `(owner, name)`. |
| `changesets` | `repo_id`, `id` (bytea 32), `p1` (nullable), `gen` (1 + max parent gen), `body` (canonical jsonb), `created_at`. PK `(repo_id, id)`. |
| `changeset_parents` | `repo_id`, `child`, `parent`, `ord`. |
| `changeset_paths` | `repo_id`, `cs`, `path`. Index on `(repo_id, path, cs)` for path history and conflict checks. |
| `refs` | `repo_id`, `name`, `target`, `updated_at`. PK `(repo_id, name)`. |
| `ref_log` | Append-only: `id` bigserial, `repo_id`, `name`, `old`, `new`, `actor` jsonb, `reason`, `at`. |
| `derived` | `repo_id`, `cs`, `kind`, `version`, `value` (jsonb or bytea), `computed_at`. PK `(repo_id, cs, kind, version)`. |
| `repo_blobs` | `repo_id`, `blob`. PK on both. Authorizes blob reads. |
| `tokens` | `id`, `owner`, `name`, `hash` (SHA-256 of the secret), `scopes` (text[]), `created_at`, `last_used_at`, `revoked_at`. |
| `path_acl` | `repo_id`, `path_prefix`, `principal`, `perm`. Designed now, enforced later. |

A ref moves with a single conditional update, and the move and its `ref_log`
row are written in the same transaction:

```sql
UPDATE refs SET target = $new, updated_at = now()
WHERE repo_id = $repo AND name = $name AND target = $old;
-- 1 row updated: success. 0 rows: someone else moved the ref; reload and retry.
```

The generation number `gen` makes ancestry queries cheap: an ancestor always
has a strictly smaller `gen`, so a walk can stop early.

Without `DATABASE_URL`, the server uses an in-memory `MetaStore` with the same
behaviour. It is meant for local development and tests.

## 7. API sketch

All routes are JSON over HTTPS. Errors use one body shape,
`{"code": "...", "message": "..."}`. Exact M0 payloads are in
[api.md](api.md).

| Route | Status |
|---|---|
| `GET /health` | M0 |
| `GET /v1` | M0 |
| `POST /v1/repos`, `GET /v1/repos/{owner}/{repo}` | planned |
| `POST /v1/repos/{owner}/{repo}/blobs/missing` | planned: which of these blob ids does the server lack? |
| `PUT /v1/repos/{owner}/{repo}/blobs/{id}` | planned: upload one blob; the server verifies its hash |
| `GET /v1/repos/{owner}/{repo}/blobs/{id}` | planned: immutable |
| `GET /v1/repos/{owner}/{repo}/trees/{id}` | planned: immutable |
| `POST /v1/repos/{owner}/{repo}/changesets` | planned: create a changeset; refs are untouched |
| `GET /v1/repos/{owner}/{repo}/changesets/{id}` | planned: immutable |
| `GET /v1/repos/{owner}/{repo}/changesets/{id}/tree` | planned: derived root tree |
| `GET /v1/repos/{owner}/{repo}/refs/{name}` | planned |
| `POST /v1/repos/{owner}/{repo}/land` | planned: rebase if needed and CAS the ref; 409 on conflict |
| `GET /v1/repos/{owner}/{repo}/log` | planned: first-parent history, optionally filtered by path |
| `GET /v1/repos/{owner}/{repo}/diff` | planned |
| `GET /v1/repos/{owner}/{repo}/files/{rev}/{path}` | planned: read one file without a checkout |
| `GET /v1/repos/{owner}/{repo}/merge-base` | planned |

## 8. Client model

**Thin client.** The client never holds a whole repository. It lays out files,
computes `status` by hashing local files against the checked-out tree, keeps
an object cache and renders the UI. History, log, diff and landing are server
calls.

**Per-machine bounded cache.** Each machine has one object cache shared by
every checkout and every agent on it, under the user's cache directory. It is
keyed by object id, holds only immutable objects, and is capped in size with
LRU eviction. Because entries never change, it needs no invalidation. This is
fldr's equivalent of scion-daemon, but it is a library inside the CLI rather
than a separate process.

**Sparse checkout.** A checkout is a root tree plus a list of path prefixes.
The client walks trees from the root and skips subtrees outside the prefixes,
so sparse checkout needs no special server support. A FUSE-based virtual
checkout, which materializes files on first read, is planned for later.

**Want-only fetch.** The client asks for the objects it needs by id. It never
tells the server what it already has, so the server does no negotiation and
every response is cacheable. Fetches are per object, so an interrupted
checkout resumes where it stopped.

**Commit.** `fldr commit -m "why"` proceeds as follows:

1. Hash changed files locally.
2. Ask the server which blobs are missing, and upload only those.
3. Send the changes against the checkout's base as a new changeset.
4. Call `land`.

## 9. Landing and conflicts

`land(repo, ref, cs)` moves a ref to include changeset `cs`, whose p1 is some
earlier target `B` of the ref.

1. Read the ref's current target `T`.
2. If `T == B`, try the CAS `B → cs`.
3. Otherwise, collect the paths changed between `B` and `T` (from
   `changeset_paths` along first-parent history) and compare them with `cs`'s
   paths using `paths_overlap`.
   - **No overlap:** create `cs'`, which has the same changes, actor, message
     and time as `cs` but has `T` as its parent. Then try the CAS `T → cs'`.
   - **Overlap:** return `409 conflict` with the overlapping paths. The client
     rebases locally, or an agent resolves the conflict, and lands again.
4. If the CAS fails because the ref moved again, go back to step 1. Retries are
   bounded, after which the server returns `503` and the client may retry.

There is no global lock and no merge queue. As in Scion, testing runs after
landing rather than gating it. A rebased changeset has a new id because its
parent changed. The response returns the id that actually landed, and
`ref_log` records the move, its actor and its reason.

This check works at path granularity. Two edits to different paths always
land, even if they are semantically incompatible. That is the same trade-off
Scion makes in exchange for lock-free throughput.

## 10. Derived data

Some questions cannot be answered from changesets alone, for example "what is
the full tree at this changeset?" or "which changesets touched this path?".
fldr computes these with one generic framework:

```rust
trait Derivable {
    const KIND: &'static str;
    const VERSION: u32;
    /// Computes this changeset's value from its parents' values.
    fn derive(cs: &Changeset, parents: &[Self]) -> Result<Self>;
}
```

Values are stored in `derived` as `(repo, cs, kind, version)`. A request for a
missing value walks back to the nearest ancestors that already have one, then
computes forward. Kinds can be computed eagerly on land or lazily on first
use. Bumping `VERSION` changes the storage format without touching clients.

Planned kinds:

- `root_tree`: p1's root tree with `changes` applied, plus every new tree
  object.
- `skiplist`: ancestry skip pointers for fast `is_ancestor` and
  `merge-base`.
- `path_history`: for log by path, built on `changeset_paths`.
- Later: `blame`, copy and rename detection, and file search.

## 11. Security

- **Tokens.** Bearer tokens are scoped (for example `repo:read`,
  `repo:write`, `admin`) and revocable. Only the SHA-256 of the secret is
  stored. `last_used_at` is updated for auditing.
- **Repos.** Each repo is public or private. Public repos are world-readable;
  writes always need a token for the owner.
- **Blob authorization.** Blobs are deduplicated across repos, so knowing a
  hash must not grant access. Every blob route is scoped to a repo, and the
  server serves a blob only if `repo_blobs` links it to that repo and the
  caller can read the repo. An upload links the blob to the target repo.
- **Path ACLs (later).** `path_acl` will restrict reads and writes under a
  path prefix. It is part of the schema now so that enforcement can be added
  without a migration.
- **Caching.** Only public-repo object responses are marked
  `Cache-Control: public, max-age=31536000, immutable`. Private responses are
  `private, no-store`, so Cloudflare never caches them.

## 12. Deployment

fldr runs on ET's Dokploy:

- **Project** `fldr`, with a **Postgres 17** database.
- **App** "fldr Server", built from the repository's `Dockerfile` on `main`
  (Rust builder stage, slim Debian runtime). It listens on port 8080 and logs
  JSON.
- **Volume** `fldr-data`, mounted at `/data`. Blobs live in `/data/blobs`, and
  `/data/.volume-id` lets `/health` show that the volume survived a
  redeploy.
- **Domains** `fldr.etdofresh.com` (primary) and `fldrs.etdofresh.com`
  (alias), both proxied through Cloudflare.

Cloudflare is the cache tier. Public object routes are immutable, so
Cloudflare can serve repeated tree and blob fetches without reaching the
server. This replaces Scion's regional caches at no cost.

## 13. Milestones

- **M0: scaffold.** Workspace, frozen hash and wire contracts, `BlobStore`
  with memory and filesystem implementations, metadata migration, server with
  `/health` and `/v1`, `fldr health`, CI, and a live deploy on Dokploy.
- **M1: data model.** RepoPath, trees, changesets, canonical encoding,
  `apply`, `diff`, `build_trees` and `paths_overlap` in `fldr-core`, with
  property tests. `MetaStore` in Postgres and in memory.
- **M2: write path.** Repos, tokens, blob upload with `blobs/missing`, the
  `repo_blobs` authorization check, changeset creation and the `root_tree`
  derivation.
- **M3: refs and landing.** Ref CAS, `ref_log`, lock-free `land` with
  server-side rebase and 409 conflicts.
- **M4: thin client.** Per-machine object cache, `fldr clone` with sparse
  prefixes, `status`, `commit -m`, and `cat` without a checkout.
- **M5: history queries.** `log`, `diff`, `merge-base` and path history,
  backed by the `skiplist` and `path_history` derivations. Cloudflare caching
  headers on immutable routes.
- **M6: agent surface.** A hosted MCP endpoint over the same API, and
  agent/session-aware log views.
- **M7: scale and reach.** S3 blob store, `blame`, `path_acl` enforcement,
  a FUSE virtual checkout and git import, each as needed.

Milestones after M1 are a sketch and will be revised as the model is tested.

## 14. Open questions

- **Large blobs.** Cloudflare's proxy limits request bodies to 100 MB on the
  Free and Pro plans, so large uploads need chunking. Options include
  content-defined chunking (blob = list of chunk ids) or a direct-upload path
  that bypasses the proxy. Chunking also changes `BlobId = sha256(bytes)` for
  large files unless the chunk list is kept internal.
- **Multiple server replicas.** `FsBlobStore` on a single volume ties fldr to
  one server instance. Running replicas needs a shared blob store such as S3
  (`s3.etdofresh.com`). Postgres already allows several stateless servers.
- **Git import.** Mapping git commits to changesets is straightforward for
  linear history. Merges, submodules and file modes need decisions, and ids
  will not match git's.
- **Rebase on land.** Should a rebased changeset keep a pointer to its
  original id, so a client can tell that its change landed under a new id?
- **Garbage collection.** Unreferenced blobs, for example from abandoned
  uploads, are never removed today. Removing a blob safely needs a
  `repo_blobs`-aware sweep.
- **Merges.** Merge changesets record changes against p1 only. How should
  `land` treat a merge, and how should `log` display it?
- **Release refs.** What are their naming and protection rules?

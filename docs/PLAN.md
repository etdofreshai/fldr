# FLDR — Design Plan

This document is the working plan for FLDR. It's intentionally rough and meant to grow over time. Each section is a piece of the system that needs to be designed before serious code is written.

## 0. Goals & non-goals

**Goals**
- Frictionless folder sync: write a file → it lives on the web.
- Per-operation history with a "why" string, AI-friendly from day one.
- Owner/repo addressing model (`owner/repo`), like Git's mental model.
- Decentralization via remote lookup; `fldr.com` is the default hub, not the only one.
- Simple CLI; everything important is doable in 1–2 commands.

**Non-goals (for v1)**
- Replacing Git for source-controlled engineering work.
- Branching/merging semantics.
- Rich permission models (start with: public or private, owner-only writes).
- Real-time multi-user co-editing (CRDTs, etc.) — sync is per-device, last-writer-wins by default.

## 1. Data model

### 1.1 Operations
An operation is the atomic unit of history. Fields:
- `id` — content-addressed hash (operation contents + parent)
- `parent` — previous op id (forms a linear chain per repo, at least in v1)
- `repo` — `owner/repo`
- `kind` — `add | edit | delete | rename | chmod`
- `path` — relative path inside the repo
- `new_path` — only for `rename`
- `content_hash` — hash of the file blob (only for `add`/`edit`)
- `size` — byte size
- `mode` — file mode bits
- `why` — short free-text description of intent (optional but encouraged)
- `actor` — who/what made the change (user id + agent id if any)
- `client` — CLI version + machine id
- `ts` — server-stamped timestamp

Blob storage is content-addressed and separate from the op log. Identical content shared across ops or repos is stored once.

### 1.2 Repo state
The state of a repo at any time = fold over the op log up to a given op id. The server can materialize a "current tree" for fast checkout, but the op log is the source of truth.

### 1.3 Conflicts
v1 strategy: optimistic, last-writer-wins per path, with the loser's content preserved as a sibling file (`foo.txt.conflict-<short-hash>`) so nothing is lost. Document this clearly.

## 2. Wire protocol

Two flavors:
- **Stream (primary)**: long-lived WebSocket or HTTP/2 stream between local daemon and remote. Daemon pushes ops as they happen; server pushes ops from other clients of the same repo.
- **Polling fallback**: simple HTTP for environments that can't hold a stream.

REST endpoints (sketch):
- `POST /v1/{owner}/{repo}/ops` — append op (+ blob if needed)
- `GET  /v1/{owner}/{repo}/ops?since={op_id}` — fetch op log tail
- `GET  /v1/{owner}/{repo}/blob/{hash}` — fetch a blob
- `GET  /v1/{owner}/{repo}/tree?at={op_id}` — materialized tree
- `GET  /v1/lookup/{owner}/{repo}` — remote-lookup endpoint (resolves to the canonical host)

Auth: token-based (Bearer). Tokens scoped per device.

## 3. CLI + local daemon

### 3.1 Commands (v1)
- `fldr login [--host fldr.com]`
- `fldr init <path> [--remote owner/repo] [--public|--private]`
- `fldr clone <owner/repo> [<path>]`
- `fldr status`
- `fldr log [--why-only]`
- `fldr remote [add|remove|list]`
- `fldr why "<text>"` — set the `why` to attach to the next N ops (configurable scope: next-op, next-N, time-window)
- `fldr daemon [start|stop|status|logs]`
- `fldr ignore` — manage a `.fldrignore` file

### 3.2 Daemon
- Watches initialized folders using OS-native APIs (FSEvents on macOS, inotify on Linux, ReadDirectoryChangesW on Windows).
- Debounces rapid writes (editors save-then-truncate-then-rewrite).
- Maintains a local op queue persisted on disk so a crash doesn't lose ops.
- Streams ops to remote(s); receives remote ops and applies them to disk.
- Hash blobs incrementally to avoid re-reading large files.

### 3.3 Ignore rules
- `.fldrignore` — gitignore-like syntax
- Built-in defaults: `.git/`, `node_modules/`, `.DS_Store`, `*.swp`, common build outputs, FLDR's own metadata dir.

### 3.4 Local metadata
Per initialized folder: `.fldr/` containing:
- `config` — remote(s), repo id, auth context
- `state.db` — sqlite: known op log, blob cache, dirty queue
- `logs/` — daemon logs

## 4. Hosting service (fldr.com)

### 4.1 Components
- **API** — the wire protocol above
- **Object storage** — blobs (S3-compatible)
- **Op log store** — append-only log per repo (Postgres or a log-oriented store)
- **Auth** — accounts, devices, tokens
- **Public viewer** — web UI to browse a repo at the current state or any past op
- **Remote lookup** — resolves `owner/repo` → canonical host (so users can self-host and still be discoverable)

### 4.2 Public viewer (v1)
Read-only browser:
- Tree view at HEAD
- File preview (text, image, basic binary)
- History panel: list of ops with `why` strings, click to scrub state
- Permalink to any op

### 4.3 Limits & quotas
- Per-account storage cap (TBD)
- Per-repo op rate limit (basic anti-abuse)
- Max blob size (TBD; large files might need a separate path)

## 5. Decentralization

- Anyone can run a FLDR host. The CLI doesn't hard-code `fldr.com`.
- `fldr.com` runs the default **remote lookup** service: `GET /v1/lookup/{owner}/{repo}` returns the canonical host URL. Self-hosters register their `owner` namespace with the lookup service, OR users address self-hosted repos directly (`host.example.com/owner/repo`).
- Multiple remotes per repo are allowed. The daemon fans out ops to all configured remotes. One is the "primary" for conflict resolution.
- Federation between hosts is **out of scope for v1** — start with simple multi-remote fan-out and revisit later.

## 6. AI integration surface

- **Library/SDK** in at least Python and TypeScript: thin wrapper over the daemon that lets an agent perform `add/edit/delete` with a `why`.
- **MCP server**: expose FLDR ops as MCP tools so agents in compatible clients can sync work into a FLDR repo with one tool call.
- **Why-string conventions**: short, imperative, present-tense ("add login handler", "fix typo in intro"). Document this so different agents produce a coherent log.
- **Session tagging**: optional `session_id` on ops so a viewer can group "what this agent did in one run".

## 7. Security & privacy

- TLS everywhere.
- Tokens are device-scoped; revocable from the web.
- Private repos: server enforces that only the owner's tokens can read/write.
- Blobs in private repos: served via signed URLs only.
- At-rest encryption story: TBD (probably out of scope for v1, but document the threat model).

## 8. Open questions

- Linear op log vs. DAG? Linear is simpler; DAG is needed if we ever want true multi-device concurrent writes without LWW. Start linear, leave room to evolve.
- Binary file storage: chunked? Resumable uploads? Probably yes for anything bigger than ~10MB.
- Should `why` be required, optional, or required-but-AI-only? Lean: optional, but the CLI nags humans who don't provide one.
- Naming: is `FLDR` final? Domain situation for `fldr.com`?
- How does an offline-then-online sync handle ops the remote already has (from another device)? Need a reconcile step on reconnect.

## 9. Milestones

1. **M0 — README + plan** ✅ (this commit)
2. **M1 — Op log + blob store, no daemon yet**: design + prototype the storage layer in isolation. Local-only.
3. **M2 — Minimal CLI**: `init`, `add` (manual), `log`, `status` against a local-only backend. Prove the data model end-to-end.
4. **M3 — Daemon**: file watching + automatic op generation on one OS first (macOS).
5. **M4 — Remote**: a hosted backend behind the wire protocol; CLI can push/pull.
6. **M5 — Public viewer**: read-only web UI.
7. **M6 — AI surface**: SDK + MCP server.
8. **M7 — Decentralization**: remote lookup + multi-remote fan-out.

Anything past M2 is speculative until M2 is built and we learn what's wrong with the model.

# Scion: lessons for fldr

This note summarizes Anthropic's engineering post
[Redesigning source control](https://www.anthropic.com/engineering/redesigning-source-control),
by Nick G. and the Anthropic source control team. It was published on
2026-10-13 and updated on 2026-10-15 to credit prior art. The post describes
**Scion**, the system that replaced git for Anthropic's monorepo. Part 1 is a
paraphrase of the post with a few short attributed quotes. Part 2 maps each
lesson to fldr's design.

Scion has not been released. fldr is an independent, personal and educational
reimplementation of the published ideas. For the full design, see
[PLAN.md](PLAN.md).

## Part 1: the article

### The problem

- **Scale.** Daily merges to Anthropic's monorepo grew from about 2,500 in May
  2026 to more than 30,000 in September 2026, and the authors expect that
  growth to continue.
- **Size.** Main-branch history is close to 100 GB, and a compressed working
  copy is over 25 GB.
- **Checkouts.** CI alone runs about 25 million checkouts per day, before
  counting developer machines, agent sandboxes and research jobs. Served
  naively by git, that would be over 600 PB of traffic per day.
- **Workarounds.** The team had already tried shallow clones, sparse
  checkouts, promisor remotes, internal mirrors and "seed" snapshots kept in
  blob storage. At peak they ran several hundred git mirrors. Seeds drifted
  from the tip of main, history operations such as diff and blame broke on
  partial clones, and the mirrors outgrew their disk and memory.
- **Diagnosis.** Like most companies, they used git centrally: one
  authoritative copy, with nearly every client (mostly agents and jobs)
  permanently online. Git's main strength, a complete offline-capable copy in
  every clone, was working against them.

### Principles

Scion draws on Google's Piper and Meta's Sapling. The post lists these
principles:

1. **Clients are always online.** They fetch data on demand instead of
   holding a full copy.
2. **Client–server asymmetry.** The server handles history, blame, commit
   creation, merges and rebases. The client keeps a bounded object cache,
   lays out files, watches the filesystem and provides the UI.
3. **Source control as a service.** Every query the client uses is also an
   API, so many jobs, such as automated bisect and revert, need no checkout.
4. **Horizontal scalability everywhere.** In the authors' words, "there is no
   fully materialized copy of the repo anywhere, even on the server." There
   are no packfiles, so there is no repository maintenance.
5. **Cheap, incremental, lock-free commits.** Commits, rebases and merges run
   on the server, so there is nothing to push. Cost scales with the size of
   the change, not the size of the repo. Refs other than main and release
   branches are discouraged, because shared global state is a scaling risk.
6. **Optional, sparse working copies.** Many reads and writes go straight
   through the API. The checkout protocol is built around sparse checkouts
   (only requested directories) and virtual checkouts (files materialized on
   demand).

### Data model

- **Changesets.** The core unit is a changeset, an entry in a write-ahead
  log. A git commit points to a full tree; a changeset lists only the files
  that changed relative to its first parent. Each entry points by content
  hash to compressed contents in blob storage. Changesets are created on the
  server and are visible everywhere immediately.
- **First parent only.** The authors call this mainly a stylistic choice: in
  trunk-based development, people care about history and blame along the
  main line.
- **Refs.** Refs exist but are decoupled from changeset creation. Moving a
  ref is a compare-and-swap on a single Spanner row. Ingestion is fast
  because it involves no packfiles, locks, cross-server coordination or ref
  updates.
- **Trees.** Trees appear mainly in the checkout protocol, where clients walk
  them recursively from a root tree. The post gives little detail on how they
  are stored.
- **Prior art.** Scion credits Meta's Sapling and its server, Mononoke, which
  "pioneered many of the techniques we use in Scion," notably the Bonsai
  changeset model.

### Derived data

Changesets alone cannot answer questions such as blame or "which changesets
touched this file?" efficiently. Scion computes **derived data** for these:
blame structures, path history and commit-graph skip lists. Every kind is
built with one generic mechanism and defined recursively in terms of its
value on the parent changesets. This has three benefits:

- New kinds are easy to add. Copy detection and path search came later.
- Each kind can be computed eagerly or deferred, so unneeded data is never
  built.
- Clients never see these structures, so they can be changed freely.

### Storage and caching

- **Servers.** A small fleet of scion-servers serves changesets, diffs, merge
  bases, blobs and more. Since all data is immutable and content-addressed,
  any server can cache anything without coordination. The authors describe
  the servers as "essentially large caches" that also compute derived data on
  the fly, writing results back to Spanner.
- **Backends.** Spanner holds metadata and refs. Cloud blob storage holds
  compressed file contents.
- **Checkout tiers.**
  1. *scion-daemon* runs once per node and shares objects across every job on
     that node. It starts from a periodically refreshed snapshot.
  2. *scion-cache* is a regional tree and blob cache. It serves objects from
     memory, already in the daemon's on-disk format, so it does no
     negotiation or delta compression.
  3. *scion-server* is the authoritative tier. On a miss it reads from
     Spanner or blob storage.
- **Tier behaviour.** Every tier answers the same RPCs, so clients do not need
  to know the topology. Tiers are still strictly ordered to isolate failures,
  so a regional cache outage cannot overwhelm the servers. Each tier keeps a
  hit rate above 99% and holds no long-lived state.
- **Rollout.** Scion first answered metadata queries such as ancestry and
  blame. Content fetching and working-copy materialization needed a separate
  design and came later.

### The client

The `scn` client uses **want-only negotiation**. It never tells the server
what it already has; it asks for the trees, then the blobs, that it is
missing. Each request is for an immutable object, so any cache can answer it.
There is no single multi-gigabyte packfile, so fetches can resume after an
interruption. Sparse and virtual checkouts come for free, because the client
simply does not request trees it does not need.

### Landing and testing

At Anthropic's scale, the usual merge queue with a global repository lock
could not keep up. Anthropic invested in post-merge and pre-deploy testing
instead. Concurrent changes can now conflict, but the authors report that
Claude usually resolves such conflicts before a human would notice. The post
does not describe the landing mechanics in more detail.

### Results

- Infrastructure is 200 times smaller in CPU and memory than the git mirror
  fleet.
- Checkouts are about two orders of magnitude faster: seconds rather than
  minutes.
- CI gained about 25% in CPU capacity.
- Neither clients nor caches need repository maintenance.

### Building it with Claude

- Seven people built Scion, the Antique code-review tool and about six
  related systems. They completed the migration in five months while merge
  volume grew tenfold.
- An early Claude Mythos snapshot rewrote the git-to-Scion ingestion
  prototype without further instructions. The rewrite used 20 times less
  memory and ran 4 times faster, and it preserved the pipeline's implicit
  ordering invariants.
- The team describes Claude as a better code writer than themselves, with
  humans contributing system design and judgment. In their words, "very
  often, we found that the role of the engineers was to tell Claude 'no.'"
  Examples include agents scanning the commit log instead of querying the
  review tool, and over-engineered validation of a trusted internal service.
  Humans still review every change, focusing on intent.
- Hermetic simulations of every non-trivial interaction the team had seen,
  together with fuzzing and rate limiting, carried much of the quality
  burden.
- Scion supports repository access control and per-directory permissions.
  Antique can express semantic approval rules.
- Claude instances in Slack ("superfans") each watch one component's
  telemetry and logs, alongside traditional alerts.

### Trade-offs and limits

- The authors state that "Scion is by no means a suitable replacement for git
  for general use." It makes sense only for very large monorepos with tens of
  thousands of engineers or agents.
- Adopting it means migrating the whole development lifecycle, including
  review, search and CI. Anthropic's versions of these are bespoke, so it is
  not releasing Scion.
- Only first-parent diffs are stored, and refs are deliberately limited.
- The authors expect others to hit similar problems within 6–12 months. They
  suggest bringing these ideas into git: new server architectures,
  standardized server-side RPCs, a redesigned negotiation protocol, and
  better composition of sparse and shallow features.

## Part 2: what fldr takes, and what it does differently

### Lesson → decision

| Lesson | fldr's choice |
|---|---|
| Clients are online and thin | The CLI keeps no full copy. History, log, diff and landing are server calls. |
| Bounded shared cache per node (scion-daemon) | One size-capped LRU object cache per machine, shared by all checkouts and agents on it. It is a library inside the CLI, not a daemon. |
| Server does history, merges and rebases | The server owns commit, land, rebase, log, diff, merge-base, ancestry and, later, blame. |
| Source control as a service | Every query is a JSON HTTP API. An MCP endpoint is planned, so agents can work without a checkout. |
| No materialized repo, no packfiles | Storage holds only immutable blobs, trees and changesets. There is nothing to repack or maintain. |
| Changesets list changes against p1 (Bonsai) | `changes: {path → put \| delete}` against p1, plus parents, actor, message and time, hashed over canonical JSON. |
| Ref moves are a single-row CAS, separate from commits | Creating a changeset never touches a ref. Refs move by `UPDATE … WHERE target=$old`, and every move goes to an append-only `ref_log`. Only `main` and release refs exist. |
| No global merge lock; test after merge | Lock-free `land`: rebase server-side when paths do not overlap, otherwise return 409 with the conflicting paths. |
| One recursive derived-data framework | `derive(kind, cs)` from the parents' values, stored as `(repo, cs, kind, version)`. First kinds: `root_tree`, `skiplist`, `path_history`. |
| Want-only negotiation; any tier can cache | Clients fetch objects by id only. Public object routes are `immutable`, so Cloudflare can cache them. |
| Sparse and virtual checkouts are native | A checkout is a root tree plus sparse prefixes. A FUSE virtual checkout is planned. |
| Per-directory permissions | Public and private repos with scoped tokens first. Blob reads go through `repo_blobs`. `path_acl` is designed now and enforced later. |
| Hermetic simulation and fuzzing | `fldr-core` is pure and property-tested. The server runs against an in-memory `MetaStore` in tests. |
| Metadata queries first, content later | Not adopted as a rollout strategy. fldr has no existing git history to serve, so both are built together. |

### Deliberate differences at personal scale

Scion serves tens of thousands of merges a day. fldr serves one person and
their agents, so several choices are simpler.

- **One server instead of a fleet.** A single `fldr-server` container on
  Dokploy is enough. The server holds no state apart from the blob store, so
  adding replicas later needs only a shared blob store.
- **Postgres instead of Spanner.** A ref CAS is an `UPDATE … WHERE` on one
  row, and the ref move and its `ref_log` entry share one transaction.
  Postgres also holds changesets and derived data.
- **A local volume instead of cloud blob storage.** Blobs are zstd files on a
  `/data` volume behind a `BlobStore` trait. An S3 implementation is planned.
- **Cloudflare instead of regional caches.** Public object routes are
  immutable, so Cloudflare's edge serves as a free scion-cache tier. There is
  no separate `fldr-cache` service yet.
- **No node daemon.** The per-machine cache is shared through the filesystem
  rather than through a long-running process.
- **Explicit commits.** fldr has no filesystem watcher producing history.
  `fldr commit -m "why"` creates each changeset. The message and actor
  replace the per-operation "why" from fldr's May 2026 concept.
- **The changeset format is published.** Scion keeps its internals hidden
  from clients. fldr documents its changeset and tree encoding so that agents
  and other tools can compute ids themselves. Derived data stays internal and
  versioned.
- **Agent workspaces, not a monorepo.** fldr is for many small repos written
  by agents and humans. It is not a git replacement, and git import is
  deferred.

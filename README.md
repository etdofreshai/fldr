# FLDR

**FLDR** (short for "folder") is a dead-simple file-sync tool and hosting service. Think of it as a different take on Git — same ideas about owners and repos, but built around continuous sync instead of commits, and with AI-friendly history baked in from day one.

> Status: early idea / pet project. Planning in progress — see [`docs/PLAN.md`](docs/PLAN.md).

## The pitch

You tell FLDR which folders on your machine you care about. From that moment on, every write, edit, and delete is mirrored to the web at **fldr.com** (or a remote of your choice). No `git add`, no `git commit`, no `git push`. You just save a file.

For humans this is a frictionless way to keep folders in sync across machines and share them publicly. For AIs and automation it's something more: every file operation can carry a short "why" string — a one-line description of the intent behind the change — so the history doubles as a meaningful changelog written at the moment of the edit, not days later in a commit message.

## How it differs from Git

| Git | FLDR |
| --- | ---- |
| Snapshot-based: commits, branches, merges | Operation-based: a stream of add/edit/delete events |
| Explicit `add` → `commit` → `push` | Implicit: write a file, it's synced |
| Commit message attached to a batch of changes | A short "why" attached to each individual file op |
| Distributed via clones; remotes are peers | Decentralized via remote lookup; **fldr.com** is the default hub but not required |
| Designed for code review workflows | Designed for fast, continuous sync (with AIs as a first-class user) |

FLDR is **not** trying to replace Git for source-controlled engineering work. It's trying to be the right tool for the much larger pile of folders that don't deserve commits — notes, assets, scratch projects, generated artifacts, agent workspaces — but still benefit from sync, history, and shareability.

## Concepts

- **Owner** — a user or org that hosts FLDR repos (e.g. `etdofresh`).
- **Repo** — a synced folder owned by an owner (e.g. `etdofresh/notes`).
- **Remote** — where a repo lives on the network. Defaults to `fldr.com`, but any FLDR-compatible host works. Remote lookup makes the system decentralized rather than centralized.
- **Operation** — the atomic unit of history: `add`, `edit`, `delete`, `rename`, each optionally tagged with a `why`.
- **History** — the ordered list of operations for a repo. There are no commits to squash, rebase, or revert against; you replay or rewind operations.

## CLI sketch (subject to change)

```sh
fldr login                         # auth against fldr.com (or a custom host)
fldr init ./my-folder              # mark a folder for sync, create the remote repo
fldr clone etdofresh/notes         # pull an existing repo to a local folder
fldr remote add backup https://…   # add another remote
fldr status                        # show sync state and any pending operations
fldr log                           # show the operation history with `why` annotations
fldr why "fixing typo in intro"    # set the `why` for the next file operation(s)
```

The daemon watches initialized folders and ships file operations to the configured remote(s) as they happen.

## AI-friendly by design

When an agent edits a file via the FLDR CLI (or library), it passes a `why` along with the operation. The result is a history that reads like a narrated session: "added scaffolding for auth route", "renamed handler for clarity", "deleted unused fixture". This is the bit that's hard to retrofit onto Git but trivial to bake into FLDR from the start.

## Roadmap

Roadmap and design notes live in [`docs/PLAN.md`](docs/PLAN.md). Major pieces to figure out:

- CLI + local daemon (file watching, op queue, conflict handling)
- Wire protocol and operation log format
- Hosting service at fldr.com (auth, storage, remote lookup, public viewer)
- Decentralization story (multiple remotes, federation, self-hosted hubs)
- AI integration surface (library/SDK, `why` capture, MCP server?)

## License

TBD.

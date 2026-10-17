# fldr HTTP API

The base URL is `https://fldr.etdofresh.com`, or `http://localhost:8080` for a
local server. Requests and responses are JSON, and field names are camelCase.
The types are defined in `fldr_core::api`
([source](../crates/fldr-core/src/api.rs)).

**Compatibility:** changes are additive only. Clients must ignore unknown
fields, and new fields are always optional. A breaking change would bump
`apiVersion`.

Only the routes in the [M0](#m0-routes) section exist today. The routes under
[planned v1 routes](#planned-v1-routes) are a design sketch and may change.

## M0 routes

### `GET /health`

Reports whether the server and its dependencies are working. The response is
**200** when `ok` is true, and otherwise **503** with the same body.

```json
{
  "ok": true,
  "service": "fldr-server",
  "version": "0.1.0",
  "gitSha": "73818ac",
  "db": "ok",
  "schema": 1,
  "blobs": "ok",
  "volumeId": "6f1c2a9e-3b7d-4e0a-9c55-0d2f8b41a7e3",
  "uptimeSecs": 4182
}
```

| Field | Type | Meaning |
|---|---|---|
| `ok` | bool | True when neither `db` nor `blobs` is `"error"`. |
| `service` | string | Always `"fldr-server"`. |
| `version` | string | Crate version of the running server. |
| `gitSha` | string, optional | Commit the server was built from. Omitted when unknown. |
| `db` | `"ok"` \| `"error"` \| `"memory"` | Postgres status. `"memory"` means `DATABASE_URL` is unset and metadata is kept in memory (local development). |
| `schema` | integer, optional | Latest applied metadata migration (`1` in M0). |
| `blobs` | `"ok"` \| `"error"` \| `"memory"` | Blob store status. |
| `volumeId` | string, optional | Random id stored in `$DATA_DIR/.volume-id` on first boot. It stays the same across restarts and redeploys only if the data volume persists. |
| `uptimeSecs` | integer | Seconds since the server started. |

A local server without Postgres reports `"db": "memory"` and still returns
200. When a dependency fails, the response is 503:

```json
{
  "ok": false,
  "service": "fldr-server",
  "version": "0.1.0",
  "db": "error",
  "blobs": "ok",
  "volumeId": "6f1c2a9e-3b7d-4e0a-9c55-0d2f8b41a7e3",
  "uptimeSecs": 12
}
```

Optional fields are omitted rather than sent as `null`. Above, `gitSha` is
unknown and `schema` is unavailable because the database is unreachable.

### `GET /v1`

Service discovery. Clients can check `apiVersion` and probe `capabilities`
before using a feature.

```json
{
  "service": "fldr-server",
  "version": "0.1.0",
  "apiVersion": 1,
  "capabilities": ["health"]
}
```

New capabilities, for example `"blobs"`, are added to the list as features
ship.

### Errors

Every non-2xx JSON response has the same body:

```json
{
  "code": "not_found",
  "message": "no route for GET /v1/nope"
}
```

`code` is stable and machine-readable; `message` is for humans and may
change. Any unknown route returns **404** with `code: "not_found"`.

| Code | Status | Meaning |
|---|---|---|
| `not_found` | 404 | Unknown route (M0), or a missing repo, ref or object (planned). |
| `bad_request` | 400 | Planned: malformed body, path or id. |
| `unauthorized` | 401 | Planned: missing or invalid token. |
| `forbidden` | 403 | Planned: the token lacks the required scope. |
| `conflict` | 409 | Planned: `land` found overlapping changes, or a ref CAS lost. |

## Planned v1 routes

**Planned.** None of these routes exist yet. They follow the design in
[PLAN.md](PLAN.md#7-api-sketch). Routes for immutable objects (blobs, trees
and changesets by id) will send
`Cache-Control: public, max-age=31536000, immutable` for public repos, and
`private, no-store` otherwise.

| Method and path | Purpose |
|---|---|
| `POST /v1/repos` | Create a repo (`owner`, `name`, `visibility`). |
| `GET /v1/repos/{owner}/{repo}` | Repo metadata. |
| `POST /v1/repos/{owner}/{repo}/blobs/missing` | Given a list of blob ids, return the ones the server lacks for this repo. |
| `PUT /v1/repos/{owner}/{repo}/blobs/{id}` | Upload one blob. The server checks that the body hashes to `id`. |
| `GET /v1/repos/{owner}/{repo}/blobs/{id}` | Blob bytes. Immutable. |
| `GET /v1/repos/{owner}/{repo}/trees/{id}` | One tree object. Immutable. |
| `POST /v1/repos/{owner}/{repo}/changesets` | Create a changeset. Refs are not touched. Returns its id. |
| `GET /v1/repos/{owner}/{repo}/changesets/{id}` | One changeset. Immutable. |
| `GET /v1/repos/{owner}/{repo}/changesets/{id}/tree` | Root tree id of a changeset (derived). |
| `GET /v1/repos/{owner}/{repo}/refs/{name}` | Current target of a ref. |
| `POST /v1/repos/{owner}/{repo}/land` | Land a changeset on a ref. Rebases if needed; 409 lists conflicting paths. |
| `GET /v1/repos/{owner}/{repo}/log` | First-parent history of a ref, optionally for one path. |
| `GET /v1/repos/{owner}/{repo}/diff` | Changes between two changesets. |
| `GET /v1/repos/{owner}/{repo}/files/{rev}/{path}` | Read one file at a revision without a checkout. |
| `GET /v1/repos/{owner}/{repo}/merge-base` | Merge base of two changesets. |

A planned `land` conflict response might look like this; the exact shape is
not settled:

```json
{
  "code": "conflict",
  "message": "changes since parent overlap 1 path",
  "paths": ["notes/todo.md"]
}
```

The extra `paths` field would be an additive extension of `ApiError`.

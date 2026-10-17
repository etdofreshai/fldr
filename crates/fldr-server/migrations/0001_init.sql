-- fldr metadata schema, version 1.
--
-- Content lives in the BlobStore; this database holds only metadata:
-- repositories, the changeset DAG, refs and their log, derived data, the
-- per-repo blob index used for read authorization, API tokens and path ACLs.
--
-- Hashes (changeset ids, blob ids) are raw 32-byte SHA-256 digests stored as
-- bytea, never hex. The changeset `body` is the canonical JSON the id was
-- computed over, so ids can always be re-verified from the database alone.

-- ---------------------------------------------------------------------------
-- Repositories
-- ---------------------------------------------------------------------------

CREATE TABLE repos (
    id          bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    owner       text        NOT NULL CHECK (owner <> '' AND octet_length(owner) <= 255),
    name        text        NOT NULL CHECK (name <> '' AND octet_length(name) <= 255),
    visibility  text        NOT NULL DEFAULT 'private'
                            CHECK (visibility IN ('public', 'private')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (owner, name)
);

-- ---------------------------------------------------------------------------
-- Changesets (immutable, content-addressed)
-- ---------------------------------------------------------------------------

-- One row per changeset. `p1` duplicates the first entry of
-- changeset_parents so linear-history walks need no join. `gen` is the
-- generation number: 1 for a root, otherwise 1 + max(parent gen). It lets
-- ancestry queries prune: a changeset can only be an ancestor of another with
-- a strictly greater gen.
CREATE TABLE changesets (
    repo_id     bigint      NOT NULL REFERENCES repos (id) ON DELETE CASCADE,
    id          bytea       NOT NULL CHECK (octet_length(id) = 32),
    p1          bytea                CHECK (octet_length(p1) = 32),
    gen         bigint      NOT NULL CHECK (gen >= 1),
    body        jsonb       NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (repo_id, id),
    -- Parents are always inserted before children.
    FOREIGN KEY (repo_id, p1) REFERENCES changesets (repo_id, id),
    -- Roots (no p1) are exactly the changesets with gen 1.
    CHECK ((p1 IS NULL) = (gen = 1)),
    CHECK (p1 IS DISTINCT FROM id)
);

CREATE INDEX changesets_children_p1 ON changesets (repo_id, p1) WHERE p1 IS NOT NULL;
CREATE INDEX changesets_gen ON changesets (repo_id, gen);

-- All parents in order; ord 0 is p1. Merges have ord 1.. as well.
CREATE TABLE changeset_parents (
    repo_id  bigint   NOT NULL,
    child    bytea    NOT NULL,
    parent   bytea    NOT NULL,
    ord      smallint NOT NULL CHECK (ord >= 0),
    PRIMARY KEY (repo_id, child, ord),
    -- Parents must not repeat.
    UNIQUE (repo_id, child, parent),
    CHECK (child <> parent),
    FOREIGN KEY (repo_id, child)  REFERENCES changesets (repo_id, id) ON DELETE CASCADE,
    FOREIGN KEY (repo_id, parent) REFERENCES changesets (repo_id, id)
);

CREATE INDEX changeset_parents_by_parent ON changeset_parents (repo_id, parent);

-- The paths a changeset changes relative to p1 (its `changes` keys). Used for
-- path history and for land's overlap check ("which paths changed between
-- the changeset's parent and the current ref target?").
CREATE TABLE changeset_paths (
    repo_id  bigint NOT NULL,
    cs       bytea  NOT NULL,
    path     text   NOT NULL CHECK (
                 path <> ''
                 AND octet_length(path) <= 4096
                 AND left(path, 1) <> '/'
                 AND right(path, 1) <> '/'
                 AND strpos(path, '\') = 0
             ),
    PRIMARY KEY (repo_id, cs, path),
    FOREIGN KEY (repo_id, cs) REFERENCES changesets (repo_id, id) ON DELETE CASCADE
);

CREATE INDEX changeset_paths_by_path ON changeset_paths (repo_id, path, cs);

-- ---------------------------------------------------------------------------
-- Refs
-- ---------------------------------------------------------------------------

-- Mutable pointers. The only refs are `main` and release refs.
--
-- Moving a ref is a single-row compare-and-swap; there is no global lock:
--
--   UPDATE refs
--      SET target = $new, updated_at = now()
--    WHERE repo_id = $repo AND name = $name AND target = $old;
--
-- One row affected means the move won; zero rows means someone else moved
-- the ref first, so `land` re-reads the target, checks path overlap since
-- the changeset's parent, rebases if disjoint and retries (or returns 409).
-- The UPDATE and the matching ref_log INSERT run in one transaction.
-- Creating a ref is `INSERT ... ON CONFLICT (repo_id, name) DO NOTHING`
-- with the same zero/one-row interpretation.
CREATE TABLE refs (
    repo_id     bigint      NOT NULL REFERENCES repos (id) ON DELETE CASCADE,
    name        text        NOT NULL CHECK (
                    name = 'main'
                    OR (name LIKE 'release/_%' AND octet_length(name) <= 255)
                ),
    target      bytea       NOT NULL CHECK (octet_length(target) = 32),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (repo_id, name),
    FOREIGN KEY (repo_id, target) REFERENCES changesets (repo_id, id)
);

-- Append-only history of every ref move. `old` is NULL on creation and
-- `new` is NULL on deletion. Server-side landing time and the mover live
-- here, not in the changeset.
CREATE TABLE ref_log (
    id       bigserial   PRIMARY KEY,
    repo_id  bigint      NOT NULL REFERENCES repos (id) ON DELETE CASCADE,
    name     text        NOT NULL,
    old      bytea                CHECK (octet_length(old) = 32),
    new      bytea                CHECK (octet_length(new) = 32),
    actor    jsonb       NOT NULL,
    reason   text        NOT NULL DEFAULT '',
    at       timestamptz NOT NULL DEFAULT now(),
    CHECK (old IS NOT NULL OR new IS NOT NULL),
    CHECK (old IS DISTINCT FROM new)
);

CREATE INDEX ref_log_by_ref ON ref_log (repo_id, name, id);

-- Enforce append-only: rows may be inserted, and removed only together with
-- their repo (ON DELETE CASCADE), but never edited.
CREATE FUNCTION ref_log_forbid_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'ref_log is append-only';
END;
$$;

CREATE TRIGGER ref_log_append_only
    BEFORE UPDATE ON ref_log
    FOR EACH ROW EXECUTE FUNCTION ref_log_forbid_update();

-- ---------------------------------------------------------------------------
-- Derived data
-- ---------------------------------------------------------------------------

-- Values of the generic recursive derivation framework (root tree, ancestry
-- skiplist, path history, later blame), computed from parents' values.
-- `kind` names the derivation and `version` its algorithm version, so a new
-- algorithm writes new rows instead of invalidating old ones. Exactly one of
-- value_json / value_bytes is set.
CREATE TABLE derived (
    repo_id      bigint      NOT NULL,
    cs           bytea       NOT NULL,
    kind         text        NOT NULL CHECK (kind <> ''),
    version      integer     NOT NULL CHECK (version >= 1),
    value_json   jsonb,
    value_bytes  bytea,
    computed_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (repo_id, cs, kind, version),
    FOREIGN KEY (repo_id, cs) REFERENCES changesets (repo_id, id) ON DELETE CASCADE,
    CHECK (num_nonnulls(value_json, value_bytes) = 1)
);

-- ---------------------------------------------------------------------------
-- Blob index
-- ---------------------------------------------------------------------------

-- Which repos reference which blobs. Blobs are global and deduplicated, but
-- a blob read is authorized only through a repo listed here.
CREATE TABLE repo_blobs (
    repo_id   bigint      NOT NULL REFERENCES repos (id) ON DELETE CASCADE,
    blob      bytea       NOT NULL CHECK (octet_length(blob) = 32),
    added_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (repo_id, blob)
);

-- Reverse lookup for garbage collection ("is this blob still referenced?").
CREATE INDEX repo_blobs_by_blob ON repo_blobs (blob);

-- ---------------------------------------------------------------------------
-- Auth
-- ---------------------------------------------------------------------------

-- API tokens. Only sha256(secret) is stored; the secret is shown once.
CREATE TABLE tokens (
    id            bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    owner         text        NOT NULL CHECK (owner <> ''),
    name          text        NOT NULL CHECK (name <> ''),
    hash          bytea       NOT NULL UNIQUE CHECK (octet_length(hash) = 32),
    scopes        text[]      NOT NULL DEFAULT '{}',
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_used_at  timestamptz,
    revoked_at    timestamptz
);

-- Token names are unique per owner among live tokens.
CREATE UNIQUE INDEX tokens_live_name ON tokens (owner, name) WHERE revoked_at IS NULL;

-- Path-prefix permissions. Designed now, enforced later. An empty prefix
-- covers the whole repo; otherwise it is a RepoPath (no leading or trailing
-- slash) and covers that path and everything under it.
CREATE TABLE path_acl (
    repo_id      bigint NOT NULL REFERENCES repos (id) ON DELETE CASCADE,
    path_prefix  text   NOT NULL CHECK (
                     path_prefix = ''
                     OR (left(path_prefix, 1) <> '/' AND right(path_prefix, 1) <> '/')
                 ),
    principal    text   NOT NULL CHECK (principal <> ''),
    perm         text   NOT NULL CHECK (perm IN ('read', 'write', 'admin')),
    PRIMARY KEY (repo_id, path_prefix, principal)
);

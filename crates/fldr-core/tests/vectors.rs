//! Golden test vectors: fixed ids that every fldr implementation must
//! reproduce, in any language.
//!
//! How they were derived: the expected canonical JSON strings below were
//! written out by hand from the brief's rules (keys sorted, no whitespace,
//! integers only, minimal escaping, raw UTF-8). The ids were then computed
//! independently of this crate with Python's `hashlib`:
//!
//! ```text
//! sha256(b"tree\0" + canonical_utf8)       # TreeId
//! sha256(b"changeset\0" + canonical_utf8)  # ChangesetId
//! sha256(file_bytes)                       # BlobId
//! ```
//!
//! These tests check that fldr-core produces exactly those bytes and ids.
//! Never update an expected value to make a test pass: a change here changes
//! every id in every repository.

use std::collections::BTreeMap;

use fldr_core::{
    Actor, ActorKind, BlobId, Changeset, ChangesetId, FileChange, FileEntry, FileKind, FlatTree,
    RepoPath, Tree,
};

const README: &[u8] = b"hello\n";
const RUN_SH: &[u8] = b"#!/bin/sh\necho hi\n";
const LINK_TARGET: &[u8] = b"README.md";

const EMPTY_TREE_ID: &str = "750760c68dcc2ade318d04eb8a558d3d32008917dd39df176d12c5f01b486727";
const BIN_TREE_ID: &str = "f17cec96123856a46998e1bbc7403b768d4350876cc005bdfffb2844233f435d";
const ROOT_TREE_ID: &str = "3c32b0bd5392f3c12b38c85af9cf90b01522b11839703117c49358d20958c8de";
const ROOT_CHANGESET_ID: &str = "404093e05c501b765a4d7ad4bb1b0460a2cd682692a1beda31564e7254d9c27a";
const CHILD_CHANGESET_ID: &str = "4aff6ced91561fb19169e1494831b0bf06d3cb9433e46cb73bf5a0b20215f9eb";

fn p(s: &str) -> RepoPath {
    RepoPath::new(s).unwrap()
}

/// `README.md` (file), `bin/run.sh` (exec), `latest` -> `README.md` (symlink).
fn sample_flat() -> FlatTree {
    FlatTree::from_files([
        (p("README.md"), FileEntry::of(README, FileKind::File)),
        (p("bin/run.sh"), FileEntry::of(RUN_SH, FileKind::Exec)),
        (p("latest"), FileEntry::of(LINK_TARGET, FileKind::Symlink)),
    ])
    .unwrap()
}

#[test]
fn blob_ids() {
    assert_eq!(
        BlobId::of(README).to_hex(),
        "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
    );
    assert_eq!(
        BlobId::of(RUN_SH).to_hex(),
        "299001868fb8c02fd431c336c6d058f5558c5dff5b5af5e6fe04b870a6a9cbba"
    );
    assert_eq!(
        BlobId::of(LINK_TARGET).to_hex(),
        "b335630551682c19a781afebcf4d07bf978fb1f8ac04c6bf87428ed5106870f5"
    );
}

#[test]
fn empty_tree() {
    assert_eq!(Tree::new().canonical_bytes().unwrap(), b"{}");
    assert_eq!(Tree::empty_id().to_hex(), EMPTY_TREE_ID);
    let (root, trees) = FlatTree::new().build_trees();
    assert_eq!(root.to_hex(), EMPTY_TREE_ID);
    assert_eq!(trees.len(), 1);
}

#[test]
fn sample_tree() {
    let (root, trees) = sample_flat().build_trees();
    let ids: Vec<String> = trees.iter().map(|(id, _)| id.to_hex()).collect();
    assert_eq!(ids, [BIN_TREE_ID, ROOT_TREE_ID]);
    assert_eq!(root.to_hex(), ROOT_TREE_ID);

    let bin = String::from_utf8(trees[0].1.canonical_bytes().unwrap()).unwrap();
    assert_eq!(
        bin,
        concat!(
            r#"{"run.sh":{"blob":"299001868fb8c02fd431c336c6d058f5558c5dff5b5af5e6fe04b870a6a9cbba","#,
            r#""kind":"exec","size":18,"type":"file"}}"#
        )
    );
    let root_json = String::from_utf8(trees[1].1.canonical_bytes().unwrap()).unwrap();
    assert_eq!(
        root_json,
        concat!(
            r#"{"README.md":{"blob":"5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03","#,
            r#""kind":"file","size":6,"type":"file"},"#,
            r#""bin":{"tree":"f17cec96123856a46998e1bbc7403b768d4350876cc005bdfffb2844233f435d","type":"dir"},"#,
            r#""latest":{"blob":"b335630551682c19a781afebcf4d07bf978fb1f8ac04c6bf87428ed5106870f5","#,
            r#""kind":"symlink","size":9,"type":"file"}}"#
        )
    );
}

fn root_changeset() -> Changeset {
    Changeset {
        parents: vec![],
        changes: BTreeMap::from([(p("README.md"), FileEntry::of(README, FileKind::File).into())]),
        actor: Actor::new(ActorKind::Human, "et"),
        message: "init".into(),
        time: 1_760_659_200,
    }
}

#[test]
fn sample_changesets() {
    let root = root_changeset();
    root.validate().unwrap();
    assert_eq!(
        String::from_utf8(root.canonical_bytes().unwrap()).unwrap(),
        concat!(
            r#"{"actor":{"kind":"human","name":"et"},"#,
            r#""changes":{"README.md":{"blob":"5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03","#,
            r#""kind":"file","op":"put","size":6}},"#,
            r#""message":"init","parents":[],"time":1760659200}"#
        )
    );
    assert_eq!(root.id().unwrap().to_hex(), ROOT_CHANGESET_ID);

    // A child by an agent: delete, exec put, symlink put, non-ASCII and an
    // escaped newline in the message.
    let child = Changeset {
        parents: vec![ROOT_CHANGESET_ID.parse::<ChangesetId>().unwrap()],
        changes: BTreeMap::from([
            (p("README.md"), FileChange::Delete),
            (
                p("bin/run.sh"),
                FileEntry::of(RUN_SH, FileKind::Exec).into(),
            ),
            (
                p("latest"),
                FileEntry::of(LINK_TARGET, FileKind::Symlink).into(),
            ),
        ]),
        actor: Actor {
            kind: ActorKind::Agent,
            name: "fldr-bot".into(),
            agent: Some("claude-code".into()),
            session: Some("s-1".into()),
        },
        message: "add run.sh é\n".into(),
        time: 1_760_659_260,
    };
    child.validate().unwrap();
    let bytes = child.canonical_bytes().unwrap();
    assert_eq!(
        String::from_utf8(bytes.clone()).unwrap(),
        concat!(
            r#"{"actor":{"agent":"claude-code","kind":"agent","name":"fldr-bot","session":"s-1"},"#,
            r#""changes":{"README.md":{"op":"delete"},"#,
            r#""bin/run.sh":{"blob":"299001868fb8c02fd431c336c6d058f5558c5dff5b5af5e6fe04b870a6a9cbba","#,
            r#""kind":"exec","op":"put","size":18},"#,
            r#""latest":{"blob":"b335630551682c19a781afebcf4d07bf978fb1f8ac04c6bf87428ed5106870f5","#,
            r#""kind":"symlink","op":"put","size":9}},"#,
            "\"message\":\"add run.sh é\\n\",",
            r#""parents":["404093e05c501b765a4d7ad4bb1b0460a2cd682692a1beda31564e7254d9c27a"],"time":1760659260}"#
        )
    );
    assert_eq!(child.id().unwrap().to_hex(), CHILD_CHANGESET_ID);
    assert_eq!(Changeset::from_canonical_bytes(&bytes).unwrap(), child);

    // Replaying both changesets yields the sample tree.
    let after_root = FlatTree::new().apply(&root.changes).unwrap();
    let after_child = after_root.apply(&child.changes).unwrap();
    let expected = FlatTree::from_files(
        sample_flat()
            .iter()
            .filter(|(path, _)| path.as_str() != "README.md")
            .map(|(path, e)| (path.clone(), *e)),
    )
    .unwrap();
    assert_eq!(after_child, expected);
}

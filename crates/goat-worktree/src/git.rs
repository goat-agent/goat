use std::path::Path;

use crate::error::WorktreeError;

#[derive(Clone)]
pub(crate) struct BaseRef {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) oid: String,
}

pub(crate) enum ExistingBase {
    Branch(String),
    Ref(BaseRef),
}

pub(crate) fn resolve_base_ref(root: &Path) -> Result<BaseRef, WorktreeError> {
    if goat_git::commit_exists(root, "origin/HEAD")? {
        return Ok(BaseRef {
            name: "origin/HEAD".to_owned(),
            kind: "origin_head".to_owned(),
            oid: goat_git::commit_oid(root, "origin/HEAD")?,
        });
    }
    Ok(BaseRef {
        name: "HEAD".to_owned(),
        kind: "head".to_owned(),
        oid: goat_git::commit_oid(root, "HEAD")?,
    })
}

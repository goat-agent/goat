use std::cell::RefCell;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::{Error, Id};
use crate::store::Store;
use crate::tool::{Journal, TRACKED_BYTES};

#[derive(Debug)]
pub(super) struct Ledger {
    store: Rc<Store>,
    session: Id,
    seen: RefCell<HashSet<PathBuf>>,
}

impl Ledger {
    pub(super) fn new(store: Rc<Store>, session: Id) -> Self {
        Self {
            store,
            session,
            seen: RefCell::new(HashSet::new()),
        }
    }

    pub(super) fn begin(&self) {
        self.seen.borrow_mut().clear();
    }

    pub(super) fn restore(&self, path: &Path, content: Option<&[u8]>) -> Result<(), Error> {
        let io = |source| Error::Restore {
            path: path.to_owned(),
            source,
        };
        let current = match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() && meta.len() <= TRACKED_BYTES => {
                Some(Some(std::fs::read(path).map_err(io)?))
            }
            Ok(_) => None,
            Err(err) if err.kind() == ErrorKind::NotFound => Some(None),
            Err(err) => return Err(io(err)),
        };
        if let Some(current) = &current {
            if current.as_deref() == content {
                return Ok(());
            }
            self.store
                .record_change(self.session, path, current.as_deref())?;
        }
        let written = match content {
            Some(bytes) => path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(path, bytes)),
            None => match std::fs::remove_file(path) {
                Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
                removed => removed,
            },
        };
        written.map_err(io)
    }
}

impl Journal for Ledger {
    fn record(&self, path: &Path, before: Option<&[u8]>) -> Result<(), String> {
        if !self.seen.borrow_mut().insert(path.to_owned()) {
            return Ok(());
        }
        self.store
            .record_change(self.session, path, before)
            .map_err(|err| {
                self.seen.borrow_mut().remove(path);
                format!("could not save the file before changing it: {err}")
            })
    }
}

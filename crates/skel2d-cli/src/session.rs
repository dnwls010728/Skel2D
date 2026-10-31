//! One document on disk plus an in-memory undo stack, shared by `serve` and
//! `mcp`. External edits to the file are picked up by `reload_if_changed`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use skel2d_core::Document;

const MAX_UNDO: usize = 100;

pub struct Session {
    pub file: PathBuf,
    pub doc: Document,
    modified: Option<SystemTime>,
    undo: Vec<Document>,
}

impl Session {
    pub fn open(file: &Path) -> Result<Self> {
        let doc = skel2d_core::load(file)?;
        Ok(Session { file: file.to_path_buf(), doc, modified: mtime(file), undo: Vec::new() })
    }

    /// Reloads the document if the file changed on disk (clears undo history).
    pub fn reload_if_changed(&mut self) -> Result<()> {
        let modified = mtime(&self.file);
        if modified != self.modified {
            self.doc = skel2d_core::load(&self.file)?;
            self.modified = modified;
            self.undo.clear();
        }
        Ok(())
    }

    /// Saves `next` atomically and makes it current, remembering the previous document.
    pub fn commit(&mut self, next: Document) -> Result<()> {
        skel2d_core::save(&self.file, &next)?;
        let prev = std::mem::replace(&mut self.doc, next);
        self.undo.push(prev);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.modified = mtime(&self.file);
        Ok(())
    }

    /// Reverts the last commit; returns the remaining undo depth, or `None` if there is nothing to undo.
    pub fn undo(&mut self) -> Result<Option<usize>> {
        let Some(prev) = self.undo.pop() else { return Ok(None) };
        skel2d_core::save(&self.file, &prev)?;
        self.doc = prev;
        self.modified = mtime(&self.file);
        Ok(Some(self.undo.len()))
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

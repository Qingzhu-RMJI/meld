use crate::config::Policy;
use crate::manifest::{FileEntry, Manifest, Tombstone};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A file that should be copied into an account tree that lacks it.
#[derive(Debug, Clone)]
pub struct CopyAction {
    pub source: FileEntry,
    pub dest_root: PathBuf,
    pub dest_path: PathBuf,
}

/// An older copy of a conversation that the newest copy should replace.
#[derive(Debug, Clone)]
pub struct UpdateAction {
    pub source: FileEntry,
    pub target: FileEntry,
}

/// A conversation that was deleted in another account and should be
/// deleted here too, the way Claude does it: write the `deleted_<chat>`
/// marker, then remove `local_<chat>.json`.
#[derive(Debug, Clone)]
pub struct DeleteAction {
    pub target: FileEntry,
    pub tombstone: Tombstone,
    pub tombstone_dest: PathBuf,
}

/// Same relative path exists in two trees with different content and no
/// copy can be picked as the newest. Never resolved silently — surfaced to
/// the user.
#[derive(Debug, Clone)]
pub struct Conflict {
    pub relative_path: PathBuf,
    pub variants: Vec<FileEntry>,
}

#[derive(Debug, Default)]
pub struct DiffReport {
    pub copies: Vec<CopyAction>,
    pub updates: Vec<UpdateAction>,
    pub deletes: Vec<DeleteAction>,
    pub conflicts: Vec<Conflict>,
    /// relative_path -> roots that already have it (for status output).
    pub coverage: BTreeMap<PathBuf, Vec<PathBuf>>,
}

impl DiffReport {
    /// Number of file changes a sync would make.
    pub fn pending(&self) -> usize {
        self.copies.len() + self.updates.len() + self.deletes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending() == 0
    }
}

/// Mirror diff: every account should end up with the same set of
/// conversations, in the same version.
///
/// Policy, in order:
/// 1. a conversation deleted in any account (a `deleted_<chat>` marker
///    exists) is deleted in every account that still has it, and never
///    copied anywhere (`propagate_deletes`)
/// 2. path missing from a root → copy (newest-mtime variant is canonical)
/// 3. same path, different content → the newest copy replaces the older
///    ones (`propagate_updates`); with that off, or when no copy is strictly
///    newest, the divergence is reported as a conflict and nothing is
///    overwritten.
pub fn compute(manifest: &Manifest, policy: Policy) -> DiffReport {
    let mut by_rel: BTreeMap<PathBuf, Vec<&FileEntry>> = BTreeMap::new();
    for e in &manifest.entries {
        by_rel.entry(e.relative_path.clone()).or_default().push(e);
    }

    // Newest marker per deleted conversation.
    let mut deleted: BTreeMap<&PathBuf, &Tombstone> = BTreeMap::new();
    for t in &manifest.tombstones {
        let slot = deleted.entry(&t.session_path).or_insert(t);
        if t.mtime > slot.mtime {
            *slot = t;
        }
    }

    let mut report = DiffReport::default();

    for (rel, entries) in &by_rel {
        let have_roots: Vec<PathBuf> =
            entries.iter().map(|e| e.account_root.clone()).collect();
        report.coverage.insert(rel.clone(), have_roots.clone());

        if policy.propagate_deletes {
            if let (Some(t), Some(marker)) =
                (deleted.get(rel), crate::scan::session_tombstone_path(rel))
            {
                for e in entries {
                    report.deletes.push(DeleteAction {
                        target: (*e).clone(),
                        tombstone: (*t).clone(),
                        tombstone_dest: e.account_root.join(&marker),
                    });
                }
                continue;
            }
        }

        // Canonical source: newest mtime wins among variants.
        let canonical = entries
            .iter()
            .max_by_key(|e| (e.mtime, e.size))
            .expect("non-empty group");

        // content_hash ignores volatile bookkeeping fields, so two copies of
        // a conversation that only differ in "last opened" timestamps are
        // treated as identical.
        let stale: Vec<&FileEntry> = entries
            .iter()
            .copied()
            .filter(|e| e.content_hash != canonical.content_hash)
            .collect();
        if !stale.is_empty() {
            // Only overwrite when the canonical copy is strictly newer than
            // every copy it would replace; a tie means we can't tell which
            // version is the latest.
            let strictly_newest = stale.iter().all(|e| e.mtime < canonical.mtime);
            if policy.propagate_updates && strictly_newest {
                for e in stale {
                    report.updates.push(UpdateAction {
                        source: (*canonical).clone(),
                        target: e.clone(),
                    });
                }
            } else {
                report.conflicts.push(Conflict {
                    relative_path: rel.clone(),
                    variants: entries.iter().map(|e| (*e).clone()).collect(),
                });
            }
        }

        for root in &manifest.account_roots {
            if !have_roots.contains(root) {
                report.copies.push(CopyAction {
                    source: (*canonical).clone(),
                    dest_root: root.clone(),
                    dest_path: root.join(rel),
                });
            }
        }
    }

    report
}

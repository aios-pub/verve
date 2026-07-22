//! Notes persistence — stores user markdown notes as JSON in `~/.verve/notes.json`.
//!
//! Notes form a tree: each note has an optional `parent_id`; root notes have `None`.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::state::persistence::data_dir;

/// A single note (markdown document).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Parent note id; `None` means this is a top-level (root) note.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Whether this note is pinned to the top of its parent group.
    #[serde(default)]
    pub pinned: bool,
}

/// Notes store: all notes + UI state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotesStore {
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub active_note: Option<String>,
    /// "split" | "edit" | "preview"
    #[serde(default = "default_view_mode")]
    pub view_mode: String,
    /// IDs of the 5 most recently used slash commands, most recent first.
    #[serde(default)]
    pub recent_commands: Vec<String>,
    /// Note ids whose child lists are currently collapsed in the sidebar.
    #[serde(default)]
    pub collapsed: Vec<String>,
}

/// Push a command ID to the recent-commands list, deduping and truncating to 5.
pub fn push_recent_command(recent: &mut Vec<String>, id: &str) {
    recent.retain(|x| x != id);
    recent.insert(0, id.to_string());
    recent.truncate(5);
}

fn default_view_mode() -> String {
    "live".to_string()
}

fn notes_path(dir: &Path) -> std::path::PathBuf {
    dir.join("notes.json")
}

/// Load notes from disk, or return an empty store when the file is missing/corrupt.
pub fn load() -> NotesStore {
    let Ok(dir) = data_dir() else {
        return NotesStore::default();
    };
    let path = notes_path(&dir);
    match fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<NotesStore>(&contents) {
            Ok(store) => store,
            Err(e) => {
                log::warn!("notes.json corrupt, starting fresh: {e:?}");
                NotesStore::default()
            }
        },
        Err(_) => NotesStore::default(),
    }
}

/// Persist notes to disk.
pub fn save(store: &NotesStore) -> Result<()> {
    let dir = data_dir()?;
    let path = notes_path(&dir);
    let json = serde_json::to_string_pretty(store)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).with_context(|| format!("write notes {:?}", tmp))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename notes {:?}", path))?;
    Ok(())
}

// ---- constructors ----------------------------------------------------------

fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn new_note(title: String, parent_id: Option<String>) -> Note {
    let now = now_str();
    Note {
        id: uuid::Uuid::new_v4().to_string(),
        title: if title.is_empty() {
            rust_i18n::t!("notes.untitled").to_string()
        } else {
            title
        },
        content: String::new(),
        created_at: now.clone(),
        updated_at: now,
        tags: Vec::new(),
        parent_id,
        pinned: false,
    }
}

/// Create a new root-level note (no parent). The note is inserted at the front
/// of the top-level list.
pub fn create_note(store: &mut NotesStore, title: String) -> &Note {
    let note = new_note(title, None);
    store.notes.insert(0, note);
    store.active_note = Some(store.notes[0].id.clone());
    &store.notes[0]
}

/// Create a new child note under `parent_id`. The child is inserted right after
/// its parent in the flat list (so depth-first traversal naturally yields it
/// after the parent, matching Notion's ordering). Returns the new note id.
pub fn create_child_note(store: &mut NotesStore, parent_id: &str, title: String) -> String {
    let note = new_note(title, Some(parent_id.to_string()));
    let id = note.id.clone();
    // Insert after the parent to keep a sensible order.
    let pos = store
        .notes
        .iter()
        .position(|n| n.id == parent_id)
        .map(|p| p + 1)
        .unwrap_or(store.notes.len());
    store.notes.insert(pos, note);
    store.active_note = Some(id.clone());
    id
}

// ---- mutations -------------------------------------------------------------

/// Update a note's content and timestamp.
pub fn update_note_content(store: &mut NotesStore, id: &str, content: String) {
    if let Some(note) = store.notes.iter_mut().find(|n| n.id == id) {
        note.content = content;
        note.updated_at = now_str();
    }
}

/// Update a note's title.
pub fn update_note_title(store: &mut NotesStore, id: &str, title: String) {
    if let Some(note) = store.notes.iter_mut().find(|n| n.id == id) {
        note.title = title;
        note.updated_at = now_str();
    }
}

/// Delete a note AND all of its descendants (children, grandchildren, …).
/// If the active note was one of the deleted notes, switches to the first
/// remaining note.
pub fn delete_note_cascade(store: &mut NotesStore, id: &str) {
    let to_delete = collect_descendants(store, id);
    store.notes.retain(|n| !to_delete.contains(&n.id));
    // Auto-switch active if it was deleted.
    if let Some(active) = &store.active_note {
        if to_delete.contains(active) {
            store.active_note = store.notes.first().map(|n| n.id.clone());
        }
    }
}

/// Collect the id of the given note plus all its descendant ids, recursively.
fn collect_descendants(store: &NotesStore, root_id: &str) -> Vec<String> {
    let mut out = vec![root_id.to_string()];
    let mut stack = vec![root_id.to_string()];
    while let Some(pid) = stack.pop() {
        for n in &store.notes {
            if n.parent_id.as_deref() == Some(&pid) {
                out.push(n.id.clone());
                stack.push(n.id.clone());
            }
        }
    }
    out
}

/// Count descendants of a note (not counting the note itself).
pub fn count_descendants(store: &NotesStore, id: &str) -> usize {
    collect_descendants(store, id).len() - 1
}

/// Duplicate a note (not its descendants) with a "(Copy)" suffix in the title.
/// The copy is inserted right after the original with the same parent_id.
/// Returns the new note's id.
pub fn duplicate_note(store: &mut NotesStore, id: &str) -> Option<String> {
    let original = store.notes.iter().find(|n| n.id == id)?.clone();
    let suffix = rust_i18n::t!("notes.duplicate_copy_suffix").to_string();
    let mut copy = new_note(
        format!("{}{}", original.title, suffix),
        original.parent_id.clone(),
    );
    copy.content = original.content.clone();
    copy.tags = original.tags.clone();
    copy.pinned = false;
    let new_id = copy.id.clone();
    // Insert right after the original.
    let pos = store
        .notes
        .iter()
        .position(|n| n.id == id)
        .map(|p| p + 1)
        .unwrap_or(store.notes.len());
    store.notes.insert(pos, copy);
    Some(new_id)
}

/// Toggle the pinned state of a note.
pub fn toggle_pin(store: &mut NotesStore, id: &str) {
    if let Some(note) = store.notes.iter_mut().find(|n| n.id == id) {
        note.pinned = !note.pinned;
        note.updated_at = now_str();
    }
}

/// Move a note to the root level (parent_id = None) and place it at the front
/// of the root list. Does NOT move its descendants.
pub fn move_note_to_root(store: &mut NotesStore, id: &str) {
    // Remove the note from its current position.
    let Some(pos) = store.notes.iter().position(|n| n.id == id) else {
        return;
    };
    let mut note = store.notes.remove(pos);
    note.parent_id = None;
    note.updated_at = now_str();
    // Insert at the front (after any pinned notes).
    store.notes.insert(0, note);
}

// ---- queries ---------------------------------------------------------------

/// Return direct children of the given parent (`None` = root notes), with pinned
/// notes sorted first, then by their position in the flat list.
pub fn children_of<'a>(store: &'a NotesStore, parent_id: Option<&str>) -> Vec<&'a Note> {
    let mut kids: Vec<&'a Note> = store
        .notes
        .iter()
        .filter(|n| n.parent_id.as_deref() == parent_id)
        .collect();
    // Sort pinned notes to the front, preserving relative order within each group.
    kids.sort_by_key(|n| !n.pinned);
    kids
}

/// Whether a note has at least one child.
pub fn has_children(store: &NotesStore, id: &str) -> bool {
    store.notes.iter().any(|n| n.parent_id.as_deref() == Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut store = NotesStore::default();
        create_note(&mut store, "Test".to_string());
        assert_eq!(store.notes.len(), 1);
        assert_eq!(store.notes[0].title, "Test");
        assert!(store.notes[0].parent_id.is_none());

        let json = serde_json::to_string(&store).unwrap();
        let loaded: NotesStore = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.notes.len(), 1);
    }

    #[test]
    fn create_child_inserts_after_parent() {
        let mut store = NotesStore::default();
        create_note(&mut store, "Root".to_string());
        let root_id = store.notes[0].id.clone();
        create_child_note(&mut store, &root_id, "Child".to_string());

        assert_eq!(store.notes.len(), 2);
        assert_eq!(store.notes[0].title, "Root");
        assert_eq!(store.notes[1].title, "Child");
        assert_eq!(store.notes[1].parent_id.as_deref(), Some(root_id.as_str()));

        let kids = children_of(&store, Some(&root_id));
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].title, "Child");
    }

    #[test]
    fn delete_cascade_removes_descendants() {
        let mut store = NotesStore::default();
        create_note(&mut store, "Root".to_string());
        let root_id = store.notes[0].id.clone();
        create_child_note(&mut store, &root_id, "A".to_string());
        let a_id = store.notes[1].id.clone();
        create_child_note(&mut store, &a_id, "B".to_string());

        assert_eq!(store.notes.len(), 3);
        assert_eq!(count_descendants(&store, &root_id), 2);

        delete_note_cascade(&mut store, &root_id);
        assert_eq!(store.notes.len(), 0);
    }

    #[test]
    fn delete_note_updates_active() {
        let mut store = NotesStore::default();
        create_note(&mut store, "First".to_string());
        create_note(&mut store, "Second".to_string());
        let id = store.notes[1].id.clone();
        delete_note_cascade(&mut store, &id);
        assert_eq!(store.notes.len(), 1);
        assert_eq!(store.active_note.as_deref(), Some(store.notes[0].id.as_str()));
    }
}

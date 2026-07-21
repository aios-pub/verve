//! Notes persistence — stores user markdown notes as JSON in `~/.verve/notes.json`.

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
}

fn default_view_mode() -> String {
    "split".to_string()
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

/// Create a new note with the given title and empty content.
pub fn create_note(store: &mut NotesStore, title: String) -> &Note {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let note = Note {
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
    };
    store.notes.insert(0, note);
    store.active_note = Some(store.notes[0].id.clone());
    &store.notes[0]
}

/// Update a note's content and timestamp.
pub fn update_note_content(store: &mut NotesStore, id: &str, content: String) {
    if let Some(note) = store.notes.iter_mut().find(|n| n.id == id) {
        note.content = content;
        note.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    }
}

/// Update a note's title.
pub fn update_note_title(store: &mut NotesStore, id: &str, title: String) {
    if let Some(note) = store.notes.iter_mut().find(|n| n.id == id) {
        note.title = title;
        note.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    }
}

/// Delete a note by id.
pub fn delete_note(store: &mut NotesStore, id: &str) {
    store.notes.retain(|n| n.id != id);
    if store.active_note.as_deref() == Some(id) {
        store.active_note = store.notes.first().map(|n| n.id.clone());
    }
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

        let json = serde_json::to_string(&store).unwrap();
        let loaded: NotesStore = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.notes.len(), 1);
    }

    #[test]
    fn delete_note_updates_active() {
        let mut store = NotesStore::default();
        create_note(&mut store, "First".to_string());
        create_note(&mut store, "Second".to_string());
        let id = store.notes[1].id.clone();
        delete_note(&mut store, &id);
        assert_eq!(store.notes.len(), 1);
        assert_eq!(store.active_note.as_deref(), Some(store.notes[0].id.as_str()));
    }
}

//! Notes persistence — each note is a Markdown file under `~/.verve/notes/`,
//! with YAML frontmatter holding its metadata (id/title/timestamps/tags/parent/pinned).
//!
//! Notes form a tree: each note has an optional `parent_id`; root notes have `None`.
//! UI-only state (active note, view mode, …) lives in `notes.meta.json`. The
//! legacy single-file `notes.json` is migrated automatically on first load.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

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
    /// In-memory id → path cache (not persisted; rebuilt by `load`). Lets
    /// `save_note` find the old file in O(1) after a title rename.
    #[serde(skip)]
    pub file_paths: NoteFilePaths,
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

/// In-memory cache mapping note id → on-disk `.md` path. Rebuilt by `load()`
/// and maintained incrementally by `save_note` / `delete_notes_files`, so note
/// writes can locate the (possibly renamed) old file in O(1) instead of
/// scanning the whole directory.
pub type NoteFilePaths = HashMap<String, PathBuf>;

// ---- paths ----------------------------------------------------------------

fn notes_dir_path(dir: &Path) -> PathBuf {
    dir.join("notes")
}
fn meta_path(dir: &Path) -> PathBuf {
    dir.join("notes.meta.json")
}
fn old_json_path(dir: &Path) -> PathBuf {
    dir.join("notes.json")
}
fn bak_path(dir: &Path) -> PathBuf {
    dir.join("notes.json.bak")
}

// ---- frontmatter (de)serialization ----------------------------------------

/// Metadata written as YAML frontmatter at the top of each note file. The
/// field spelling (`parent`, not `parent_id`) matches the on-disk convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoteFrontmatter {
    id: String,
    title: String,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pinned: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

impl From<&Note> for NoteFrontmatter {
    fn from(n: &Note) -> Self {
        NoteFrontmatter {
            id: n.id.clone(),
            title: n.title.clone(),
            created_at: n.created_at.clone(),
            updated_at: n.updated_at.clone(),
            tags: n.tags.clone(),
            parent: n.parent_id.clone(),
            pinned: n.pinned,
        }
    }
}

impl NoteFrontmatter {
    fn into_note(self, content: String) -> Note {
        Note {
            id: self.id,
            title: self.title,
            content,
            created_at: self.created_at,
            updated_at: self.updated_at,
            tags: self.tags,
            parent_id: self.parent,
            pinned: self.pinned,
        }
    }
}

/// Split a Markdown document into `(frontmatter_yaml, body)`. The frontmatter
/// is delimited by a leading `---` and a closing `---`/`...` on its own line.
/// Returns `None` when no valid frontmatter block is present. Newline-splitting
/// stays on ASCII `\n` boundaries, so offsets are always valid char boundaries.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    // Tolerate a UTF-8 BOM written by some external editors.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let after_open = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    // Walk lines (keeping their trailing newline) so byte offsets land on `\n`.
    let mut pos = 0usize;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed == "---" || trimmed == "..." {
            let yaml = &after_open[..pos];
            let body = &after_open[pos + line.len()..];
            return Some((yaml, body));
        }
        pos += line.len();
    }
    None
}

/// Serialize a note into a Markdown document with YAML frontmatter.
fn note_to_markdown(note: &Note) -> Result<String> {
    let fm = NoteFrontmatter::from(note);
    let yaml = serde_yaml::to_string(&fm)?;
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(yaml.trim_end());
    out.push_str("\n---\n\n");
    out.push_str(&note.content);
    Ok(out)
}

/// Parse a note from a Markdown document with YAML frontmatter. Returns
/// `None` (rather than panicking) when the file is malformed.
pub(crate) fn parse_note_markdown(text: &str) -> Option<Note> {
    let (yaml, body) = split_frontmatter(text)?;
    let fm: NoteFrontmatter = serde_yaml::from_str(yaml).ok()?;
    // Drop the single blank line that separates the frontmatter fence from
    // the body (see `note_to_markdown`).
    let body = body.strip_prefix('\n').unwrap_or(body);
    Some(fm.into_note(body.to_string()))
}

// ---- filename helpers -----------------------------------------------------

/// Turn a note title into a safe single-segment filename (no extension),
/// replacing path separators / control chars and clamping the length by char
/// count so multi-byte titles stay on UTF-8 boundaries.
fn sanitize_filename(title: &str) -> String {
    const MAX_LEN: usize = 80;
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_matches(['_', ' ', '.']).to_string();
    let fallback = rust_i18n::t!("notes.untitled").to_string();
    let name = if trimmed.is_empty() { fallback } else { trimmed };
    name.chars().take(MAX_LEN).collect()
}

/// Compute a non-conflicting `.md` path for `note` inside `dir`.
fn unique_note_path(dir: &Path, note: &Note, existing: &HashSet<String>) -> PathBuf {
    let base = sanitize_filename(&note.title);
    let primary = format!("{base}.md");
    if !existing.contains(&primary) {
        return dir.join(primary);
    }
    // Trailing 8 chars disambiguate same-title notes reliably: sparkid's
    // leading chars are a timestamp prefix that collides for notes minted in
    // the same tick (also fine for legacy uuid ids, whose tail is random hex).
    let short_id: String = crate::state::models::id_suffix(&note.id, 8);
    let with_id = format!("{base}-{short_id}.md");
    if !existing.contains(&with_id) {
        return dir.join(with_id);
    }
    dir.join(format!("{short_id}.md"))
}

fn collect_md_filenames(dir: &Path) -> HashSet<String> {
    let mut set = HashSet::new();
    if let Ok(it) = fs::read_dir(dir) {
        for entry in it.flatten() {
            if entry.path().extension().is_some_and(|e| e == "md") {
                if let Some(name) = entry.file_name().to_str() {
                    set.insert(name.to_string());
                }
            }
        }
    }
    set
}

/// Remove the on-disk file whose frontmatter `id` matches, if any. Used to
/// delete a note, or to clear the old filename before writing a renamed one.
fn delete_note_file_by_id(dir: &Path, id: &str) {
    let Ok(it) = fs::read_dir(dir) else {
        return;
    };
    for entry in it.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "md") {
            if let Ok(text) = fs::read_to_string(&path) {
                if parse_note_markdown(&text).is_some_and(|n| n.id == id) {
                    let _ = fs::remove_file(&path);
                    return;
                }
            }
        }
    }
}

fn atomic_write_text(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, content).with_context(|| format!("write {:?}", tmp))?;
    fs::rename(&tmp, path).with_context(|| format!("rename {:?}", path))?;
    Ok(())
}

// ---- per-note & meta persistence ------------------------------------------

/// Persist a single note to its own `.md` file (atomic write). A title change
/// renames the file automatically: the old file is removed (via `paths` in
/// O(1)) before the new one is written, and `paths` is updated.
pub fn save_note(note: &Note, paths: &mut NoteFilePaths) -> Result<()> {
    let dir = data_dir()?;
    let notes_dir = notes_dir_path(&dir);
    fs::create_dir_all(&notes_dir).with_context(|| format!("create {:?}", notes_dir))?;
    save_note_to_dir(&notes_dir, note, paths)
}

fn save_note_to_dir(notes_dir: &Path, note: &Note, paths: &mut NoteFilePaths) -> Result<()> {
    // Remove the old file in O(1) via the cached path — a title rename changes
    // the filename, so it can't be derived. Fall back to a directory scan only
    // when the id isn't cached (e.g. a file created externally since `load`).
    if let Some(old) = paths.get(&note.id) {
        if old.exists() {
            let _ = fs::remove_file(old);
        }
    } else {
        delete_note_file_by_id(notes_dir, &note.id);
    }
    let existing = collect_md_filenames(notes_dir);
    let path = unique_note_path(notes_dir, note, &existing);
    let md = note_to_markdown(note)?;
    atomic_write_text(&path, &md)?;
    paths.insert(note.id.clone(), path);
    Ok(())
}

/// Delete the on-disk file backing a note (no-op if it is already gone).
pub fn delete_note_file(id: &str, paths: &mut NoteFilePaths) {
    if let Some(p) = paths.remove(id) {
        let _ = fs::remove_file(&p);
        return;
    }
    let Ok(dir) = data_dir() else { return };
    delete_note_file_by_id(&notes_dir_path(&dir), id);
}

/// Delete the on-disk files backing each note id in O(ids) using the path
/// cache. (Ids absent from the cache are skipped — they reappear on the next
/// `load`. The cache is kept complete by `load` + the fs watcher at runtime.)
pub fn delete_notes_files(ids: &[String], paths: &mut NoteFilePaths) {
    for id in ids {
        if let Some(p) = paths.remove(id) {
            let _ = fs::remove_file(&p);
        }
    }
}

/// UI-only state kept alongside the notes directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct NotesMeta {
    #[serde(default)]
    active_note: Option<String>,
    #[serde(default = "default_view_mode")]
    view_mode: String,
    #[serde(default)]
    recent_commands: Vec<String>,
    #[serde(default)]
    collapsed: Vec<String>,
    /// Stable ordering of notes in the sidebar (list of note ids). Notes not
    /// present here fall back to `created_at` descending.
    #[serde(default)]
    note_order: Vec<String>,
}

/// Persist UI state + sidebar ordering to `notes.meta.json`.
pub fn save_meta(store: &NotesStore) -> Result<()> {
    let dir = data_dir()?;
    let meta = NotesMeta {
        active_note: store.active_note.clone(),
        view_mode: store.view_mode.clone(),
        recent_commands: store.recent_commands.clone(),
        collapsed: store.collapsed.clone(),
        note_order: store.notes.iter().map(|n| n.id.clone()).collect(),
    };
    let json = serde_json::to_string_pretty(&meta)?;
    atomic_write_text(&meta_path(&dir), &json)
}

fn load_meta(dir: &Path) -> Option<NotesMeta> {
    let path = meta_path(dir);
    let contents = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Sort notes in place to honor the persisted `note_order`; ids absent from
/// the order are appended at the end, newest-first by `created_at`.
fn apply_note_order(notes: &mut Vec<Note>, order: &[String]) {
    let pos: HashMap<&String, usize> = order
        .iter()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect();
    notes.sort_by(|a, b| {
        let pa = pos.get(&a.id).copied().unwrap_or(usize::MAX);
        let pb = pos.get(&b.id).copied().unwrap_or(usize::MAX);
        pa.cmp(&pb)
            .then_with(|| b.created_at.cmp(&a.created_at))
    });
}

// ---- load / migrate -------------------------------------------------------

/// Load notes from disk (one `.md` per note), or return an empty store on a
/// first run. Migrates the legacy `notes.json` automatically.
pub fn load() -> NotesStore {
    let Ok(dir) = data_dir() else {
        return NotesStore::default();
    };
    let _ = migrate_from_json_if_needed(&dir);

    let notes_dir = notes_dir_path(&dir);
    let _ = fs::create_dir_all(&notes_dir);

    let mut notes: Vec<Note> = Vec::new();
    let mut file_paths: NoteFilePaths = HashMap::new();
    if let Ok(it) = fs::read_dir(&notes_dir) {
        for entry in it.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md") {
                match fs::read_to_string(&path) {
                    Ok(text) => match parse_note_markdown(&text) {
                        Some(n) => {
                            file_paths.insert(n.id.clone(), path.clone());
                            notes.push(n);
                        }
                        None => log::warn!("skip malformed note {:?}", path),
                    },
                    Err(e) => log::warn!("read note {:?} failed: {e:?}", path),
                }
            }
        }
    }

    let meta = load_meta(&dir).unwrap_or_default();
    apply_note_order(&mut notes, &meta.note_order);

    NotesStore {
        notes,
        active_note: meta.active_note,
        view_mode: meta.view_mode,
        recent_commands: meta.recent_commands,
        collapsed: meta.collapsed,
        file_paths,
    }
}

/// One-time migration: when `notes.json` exists but `notes/` does not, write
/// one `.md` per note, persist UI state to `notes.meta.json`, and rename the
/// old file to `notes.json.bak`. Idempotent.
fn migrate_from_json_if_needed(dir: &Path) -> Result<()> {
    let notes_dir = notes_dir_path(dir);
    let old_json = old_json_path(dir);
    let bak = bak_path(dir);
    if notes_dir.exists() || bak.exists() || !old_json.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(&old_json)?;
    let old: NotesStore = match serde_json::from_str(&contents) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("notes.json corrupt, skip migration: {e:?}");
            return Ok(());
        }
    };
    fs::create_dir_all(&notes_dir)?;
    let mut paths: NoteFilePaths = HashMap::new();
    for note in &old.notes {
        if let Err(e) = save_note_to_dir(&notes_dir, note, &mut paths) {
            log::warn!("migrate note {} failed: {e:?}", note.id);
        }
    }
    let meta = NotesMeta {
        active_note: old.active_note.clone(),
        view_mode: old.view_mode.clone(),
        recent_commands: old.recent_commands.clone(),
        collapsed: old.collapsed.clone(),
        note_order: old.notes.iter().map(|n| n.id.clone()).collect(),
    };
    let json = serde_json::to_string_pretty(&meta)?;
    atomic_write_text(&meta_path(dir), &json)?;
    fs::rename(&old_json, &bak)
        .with_context(|| format!("rename {:?} -> {:?}", old_json, bak))?;
    log::info!("migrated {} notes from notes.json to notes/", old.notes.len());
    Ok(())
}

/// Persist all notes + UI state. Prefer the per-note `save_note` / `save_meta`
/// in hot paths; this full write is kept for compatibility with older callers.
pub fn save(store: &NotesStore) -> Result<()> {
    let dir = data_dir()?;
    let notes_dir = notes_dir_path(&dir);
    fs::create_dir_all(&notes_dir)?;
    // Clone the cache so the (immutable) store isn't mutated; this legacy
    // full-write path isn't hot — see `save_note` for normal use.
    let mut paths = store.file_paths.clone();
    for note in &store.notes {
        if let Err(e) = save_note_to_dir(&notes_dir, note, &mut paths) {
            log::warn!("save note {} failed: {e:?}", note.id);
        }
    }
    save_meta(store)
}

// ---- constructors ----------------------------------------------------------

fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn new_note(title: String, parent_id: Option<String>) -> Note {
    let now = now_str();
    Note {
        // sparkid: 21-char Base58, time-sortable (timestamp prefix + monotonic
        // counter + random tail). Shorter than a UUID, lexicographically ordered,
        // and the time-prefix also makes the filename-dedup short_id meaningful.
        id: sparkid::SparkId::new().to_string(),
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

/// Public accessor for the id of a note plus all its descendants, so callers
/// can collect on-disk file ids before an in-memory cascade delete.
pub fn note_ids_in_subtree(store: &NotesStore, id: &str) -> Vec<String> {
    collect_descendants(store, id)
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
    store
        .notes
        .iter()
        .any(|n| n.parent_id.as_deref() == Some(id))
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
        assert_eq!(
            store.active_note.as_deref(),
            Some(store.notes[0].id.as_str())
        );
    }

    // ---- file-storage helpers for tests ------------------------------------

    /// Point `data_dir()` at a unique temp dir for the duration of the guard.
    struct TestEnv {
        _dir_guard: crate::state::persistence::ThreadDataDirGuard,
        _cleanup: DirCleanup,
    }

    struct DirCleanup(PathBuf);
    impl Drop for DirCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn setup_test_env() -> (TestEnv, PathBuf) {
        let dir = std::env::temp_dir().join(format!("verve-notes-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let env = TestEnv {
            _dir_guard: crate::state::persistence::set_thread_data_dir(dir.clone()),
            _cleanup: DirCleanup(dir.clone()),
        };
        (env, dir)
    }

    fn sample_note(title: &str) -> Note {
        Note {
            id: sparkid::SparkId::new().to_string(),
            title: title.to_string(),
            content: format!("# {title}\n\n正文 mixed with 中文"),
            created_at: "2026-08-12 10:00:00".to_string(),
            updated_at: "2026-08-12 11:30:00".to_string(),
            tags: vec!["架构".to_string(), "笔记".to_string()],
            parent_id: None,
            pinned: false,
        }
    }

    #[test]
    fn markdown_roundtrip_basic() {
        let note = sample_note("设计文档");
        let md = note_to_markdown(&note).unwrap();
        assert!(md.starts_with("---\n"), "md = {md}");
        let parsed = parse_note_markdown(&md).expect("parse");
        assert_eq!(parsed.id, note.id);
        assert_eq!(parsed.title, note.title);
        assert_eq!(parsed.content, note.content);
        assert_eq!(parsed.tags, note.tags);
        assert!(parsed.parent_id.is_none());
        assert!(!parsed.pinned);
    }

    #[test]
    fn markdown_roundtrip_child_and_pinned() {
        let mut note = sample_note("周报");
        note.parent_id = Some("parent-uuid".to_string());
        note.pinned = true;
        let md = note_to_markdown(&note).unwrap();
        let parsed = parse_note_markdown(&md).expect("parse");
        assert_eq!(parsed.parent_id.as_deref(), Some("parent-uuid"));
        assert!(parsed.pinned);
    }

    #[test]
    fn markdown_roundtrip_empty_content() {
        let mut note = sample_note("空笔记");
        note.content = String::new();
        let md = note_to_markdown(&note).unwrap();
        let parsed = parse_note_markdown(&md).expect("parse");
        assert_eq!(parsed.content, "");
    }

    #[test]
    fn parse_rejects_missing_frontmatter() {
        assert!(parse_note_markdown("just some text").is_none());
        assert!(parse_note_markdown("").is_none());
    }

    #[test]
    fn sanitize_filename_handles_special_chars() {
        assert_eq!(sanitize_filename("设计/文档:1"), "设计_文档_1");
        assert_eq!(sanitize_filename(" _ hi *? "), "hi");
        // empty → localized untitled fallback (non-empty)
        assert!(!sanitize_filename("").is_empty());
        // over-length multibyte title truncated on a char boundary
        let long: String = "中".repeat(120);
        assert_eq!(sanitize_filename(&long).chars().count(), 80);
    }

    #[test]
    fn save_and_load_single_note() {
        let (_env, _dir) = setup_test_env();
        let note = sample_note("独立笔记");
        let id = note.id.clone();
        let mut paths = NoteFilePaths::new();
        save_note(&note, &mut paths).unwrap();
        assert!(paths.contains_key(&id), "path cache updated after save");

        let loaded = load();
        assert_eq!(loaded.notes.len(), 1);
        assert_eq!(loaded.notes[0].id, id);
        assert_eq!(loaded.notes[0].content, note.content);
        assert!(loaded.file_paths.contains_key(&id), "load rebuilds path cache");
    }

    #[test]
    fn rename_via_title_change() {
        let (_env, _dir) = setup_test_env();
        let mut note = sample_note("旧标题");
        let mut paths = NoteFilePaths::new();
        save_note(&note, &mut paths).unwrap();
        note.title = "新标题".to_string();
        save_note(&note, &mut paths).unwrap();

        let loaded = load();
        assert_eq!(loaded.notes.len(), 1, "old file should be removed");
        assert_eq!(loaded.notes[0].title, "新标题");
    }

    /// A title rename removes the old file via the cached path (O(1)), not a
    /// directory scan — after two saves with different titles there must be
    /// exactly one `.md` on disk, and the stale path no longer exists.
    #[test]
    fn save_note_uses_cached_path_for_rename() {
        let (_env, dir) = setup_test_env();
        let mut note = sample_note("first");
        let mut paths = NoteFilePaths::new();
        save_note(&note, &mut paths).unwrap();
        let old_path = paths.get(&note.id).cloned().expect("cached");
        note.title = "second".to_string();
        save_note(&note, &mut paths).unwrap();
        assert!(!old_path.exists(), "old file removed via cache, not scan");
        let md_count = fs::read_dir(dir.join("notes"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .count();
        assert_eq!(md_count, 1);
    }

    #[test]
    fn delete_note_file_removes_disk() {
        let (_env, _dir) = setup_test_env();
        let note = sample_note("待删除");
        let id = note.id.clone();
        let mut paths = NoteFilePaths::new();
        save_note(&note, &mut paths).unwrap();
        delete_note_file(&id, &mut paths);
        let loaded = load();
        assert!(loaded.notes.is_empty());
        assert!(!paths.contains_key(&id), "path cache cleared after delete");
    }

    #[test]
    fn meta_persists_active_and_order() {
        let (_env, dir) = setup_test_env();
        let mut store = NotesStore::default();
        let a = create_note(&mut store, "A".to_string()).id.clone();
        let b = create_note(&mut store, "B".to_string()).id.clone();
        store.active_note = Some(b.clone());
        save_meta(&store).unwrap();

        let meta = load_meta(&dir).expect("meta");
        assert_eq!(meta.active_note.as_deref(), Some(b.as_str()));
        assert_eq!(meta.note_order, vec![b, a]); // create inserts at front
    }

    #[test]
    fn migrate_converts_json_to_markdown_files() {
        let (_env, dir) = setup_test_env();
        // Seed a legacy notes.json with a parent + child.
        let mut store = NotesStore::default();
        let root = create_note(&mut store, "根笔记".to_string());
        let root_id = root.id.clone();
        create_child_note(&mut store, &root_id, "子笔记".to_string());
        let json = serde_json::to_string_pretty(&store).unwrap();
        fs::write(dir.join("notes.json"), json).unwrap();

        migrate_from_json_if_needed(&dir).expect("migrate");

        let md_count = fs::read_dir(dir.join("notes"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .count();
        assert_eq!(md_count, 2);
        assert!(dir.join("notes.json.bak").exists());
        assert!(!dir.join("notes.json").exists());

        // load() reads them back, preserving the tree.
        let loaded = load();
        assert_eq!(loaded.notes.len(), 2);
        let child = loaded
            .notes
            .iter()
            .find(|n| n.title == "子笔记")
            .expect("child present");
        assert_eq!(child.parent_id.as_deref(), Some(root_id.as_str()));
    }

    #[test]
    fn migrate_is_idempotent() {
        let (_env, dir) = setup_test_env();
        fs::write(dir.join("notes.json"), "{}").unwrap();
        migrate_from_json_if_needed(&dir).unwrap();
        // Second call must be a no-op (bak now exists).
        migrate_from_json_if_needed(&dir).unwrap();
    }
}

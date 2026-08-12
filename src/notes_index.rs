//! Full-text search index for notes, backed by [tantivy].
//!
//! The disk markdown files (`~/.verve/notes/*.md`) are the single source of
//! truth; this index is a rebuildable derivative stored under
//! `~/.verve/notes.index/` (git-ignored). Each text field is indexed twice —
//! once with [jieba] word segmentation and once with [n-gram] substring
//! tokenization — so a query can hit either word-level matches (jieba) or any
//! contiguous substring (n-gram). That maximizes recall for CJK + Latin notes.
//!
//! All write operations go through a shared `Mutex<IndexWriter>`; reads use a
//! reloading `IndexReader` with no lock. Callers (the notes panel) drive
//! incremental updates on every note CRUD and a full `rebuild_from_disk` on
//! startup / external file changes. The UI surfaces [`IndexStatus`] so the user
//! can tell while a rebuild is in progress (search then falls back to memory).
//!
//! [tantivy]: https://docs.rs/tantivy
//! [jieba]: https://docs.rs/tantivy-jieba
//! [n-gram]: tantivy::tokenizer::NgramTokenizer

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, STORED, STRING,
};
use tantivy::tokenizer::NgramTokenizer;
use tantivy::{doc, DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Score, Term};

use crate::notes::{parse_note_markdown, Note};

/// One search hit: the matching note's id + relevance score (for ordering).
pub struct SearchHit {
    pub note_id: String,
    #[allow(dead_code)]
    pub score: f32,
}

/// Lifecycle of the on-disk index, surfaced to the UI so the user can tell when
/// search is still building (vs. ready / degraded to the in-memory fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStatus {
    /// Full rebuild in progress. The UI shows a status bar; search falls back
    /// to the in-memory matcher so results stay complete.
    Building,
    /// Index is open and queryable.
    Ready,
    /// Index could not be opened/built. Search falls back to memory silently.
    Failed,
}

pub struct NotesIndex {
    index: Index,
    writer: Arc<Mutex<IndexWriter>>,
    reader: IndexReader,
    id_field: Field,
    title_jieba: Field,
    title_ngram: Field,
    content_jieba: Field,
    content_ngram: Field,
}

impl NotesIndex {
    /// Tokenizer name for jieba (word-level) analysis.
    const JIEBA: &'static str = "verve_jieba";
    /// Tokenizer name for n-gram (substring) analysis.
    const NGRAM: &'static str = "verve_ngram";

    /// Build the fixed schema and return it together with the field handles.
    fn build_schema() -> (Schema, Field, Field, Field, Field, Field) {
        let mut sb = Schema::builder();
        // note_id: exact-match (raw tokenization) for delete-by-term; stored for
        // retrieval of the matched id.
        let id_field = sb.add_text_field("note_id", STRING | STORED);
        let title_jieba = sb.add_text_field("title_jieba", Self::text_opts(Self::JIEBA, true));
        let title_ngram = sb.add_text_field("title_ngram", Self::text_opts(Self::NGRAM, false));
        let content_jieba = sb.add_text_field("content_jieba", Self::text_opts(Self::JIEBA, true));
        let content_ngram = sb.add_text_field("content_ngram", Self::text_opts(Self::NGRAM, false));
        let schema = sb.build();
        (schema, id_field, title_jieba, title_ngram, content_jieba, content_ngram)
    }

    /// Text field options bound to `tokenizer`. The `*_jieba` fields are STORED
    /// (so the original text is retrievable); the `*_ngram` fields are index-only.
    fn text_opts(tokenizer: &str, stored: bool) -> TextOptions {
        let indexing = TextFieldIndexing::default()
            .set_tokenizer(tokenizer)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions);
        let opts = TextOptions::default().set_indexing_options(indexing);
        if stored {
            opts.set_stored()
        } else {
            opts
        }
    }

    /// Open an existing index at `index_dir`, or create a fresh one. If the
    /// on-disk schema is stale/incompatible (e.g. after an upgrade), drop the
    /// directory and recreate so search is never permanently blocked.
    pub fn open_or_create(index_dir: &Path) -> Result<Self> {
        let (schema, id_field, title_jieba, title_ngram, content_jieba, content_ngram) =
            Self::build_schema();

        let index = match Index::open_in_dir(index_dir) {
            Ok(ix) => ix,
            Err(_) => {
                let _ = fs::remove_dir_all(index_dir);
                fs::create_dir_all(index_dir).with_context(|| "create notes index dir")?;
                Index::create_in_dir(index_dir, schema.clone())
                    .with_context(|| "create notes index")?
            }
        };

        // Register both analyzers under the names referenced by the schema.
        index
            .tokenizers()
            .register(Self::JIEBA, tantivy_jieba::JiebaTokenizer::new());
        // n-gram (1,2): min_gram=1 keeps single-CJK-char queries reachable
        // (a 2,3 n-gram could not match a 1-char search); max_gram=2 covers
        // 2-char substrings. Longer substrings are left to the jieba field, so
        // the index stays small without losing common CJK substring recall.
        index
            .tokenizers()
            .register(Self::NGRAM, NgramTokenizer::new(1, 2, false)?);

        // ~48 MB write arena is plenty for a personal note corpus.
        let writer = index.writer(50_000_000).with_context(|| "open index writer")?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .with_context(|| "open index reader")?;

        Ok(Self {
            index,
            writer: Arc::new(Mutex::new(writer)),
            reader,
            id_field,
            title_jieba,
            title_ngram,
            content_jieba,
            content_ngram,
        })
    }

    /// Build the tantivy document for a note (all fields populated).
    fn doc_for(&self, note: &Note) -> tantivy::TantivyDocument {
        doc!(
            self.id_field => note.id.as_str(),
            self.title_jieba => note.title.as_str(),
            self.title_ngram => note.title.as_str(),
            self.content_jieba => note.content.as_str(),
            self.content_ngram => note.content.as_str(),
        )
    }

    /// Insert or replace a single note (delete-by-id then add, then commit).
    /// Reloads the reader so a search right after observes the change.
    pub fn upsert(&self, note: &Note) -> Result<()> {
        {
            let mut w = self.writer.lock().map_err(|e| anyhow::anyhow!("writer lock: {e}"))?;
            w.delete_term(Term::from_field_text(self.id_field, &note.id));
            let _ = w.add_document(self.doc_for(note));
            w.commit()?;
        }
        let _ = self.reader.reload();
        Ok(())
    }

    /// Remove a note by id (does not require the md file to still exist).
    /// Reloads the reader so a search right after observes the deletion.
    pub fn delete(&self, note_id: &str) -> Result<()> {
        {
            let mut w = self.writer.lock().map_err(|e| anyhow::anyhow!("writer lock: {e}"))?;
            w.delete_term(Term::from_field_text(self.id_field, note_id));
            w.commit()?;
        }
        let _ = self.reader.reload();
        Ok(())
    }

    /// Wipe and rebuild the whole index from on-disk markdown files. The disk
    /// is the source of truth — this never reads the in-memory store, so the
    /// index stays consistent even if the store is mid-edit. Malformed files
    /// are skipped rather than aborting the rebuild.
    pub fn rebuild_from_disk(&self, notes_dir: &Path) -> Result<()> {
        let mut w = self.writer.lock().map_err(|e| anyhow::anyhow!("writer lock: {e}"))?;
        w.delete_all_documents()?;
        if let Ok(entries) = fs::read_dir(notes_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_md = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e == "md");
                if !is_md {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&path) else {
                    continue;
                };
                if let Some(note) = parse_note_markdown(&text) {
                    // A single bad document must not abort the whole rebuild.
                    let _ = w.add_document(self.doc_for(&note));
                }
            }
        }
        w.commit()?;
        drop(w);
        // OnCommit reloads automatically, but reload explicitly so a search
        // immediately following this call observes the fresh segment.
        self.reader.reload()?;
        Ok(())
    }

    /// Full-text search across title + content, both analyzers, returning the
    /// top `limit` hits ordered by relevance. Title fields are boosted so title
    /// matches outrank body matches.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let q = query.trim();
        if q.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut parser = QueryParser::for_index(
            &self.index,
            vec![
                self.title_jieba,
                self.title_ngram,
                self.content_jieba,
                self.content_ngram,
            ],
        );
        parser.set_field_boost(self.title_jieba, 2.0);
        parser.set_field_boost(self.title_ngram, 2.0);
        let parsed = parser.parse_query(q).context("parse search query")?;
        let searcher = self.reader.searcher();
        let top: Vec<(Score, DocAddress)> =
            searcher.search(&parsed, &TopDocs::with_limit(limit).order_by_score())?;
        let mut hits = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let doc: tantivy::TantivyDocument = searcher.doc(addr)?;
            if let Some(id) = doc.get_first(self.id_field).and_then(|v| v.as_str()) {
                hits.push(SearchHit {
                    note_id: id.to_string(),
                    score,
                });
            }
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A unique temp dir for one test's index + notes, cleaned up on drop.
    struct Scratch {
        root: PathBuf,
    }
    impl Scratch {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "verve-notes-index-{label}-{}-{}",
                std::process::id(),
                crate::state::models::new_id()
            ));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }
        fn index_dir(&self) -> PathBuf {
            self.root.join("idx")
        }
        fn notes_dir(&self) -> PathBuf {
            let d = self.root.join("notes");
            fs::create_dir_all(&d).unwrap();
            d
        }
        /// Write a note as a frontmatter+body markdown file (mirrors notes.rs).
        fn write_md(&self, note: &Note) {
            let fname = format!("{}.md", note.id);
            let md = format!(
                "---\nid: {id}\ntitle: {title}\ncreated_at: \"{ca}\"\nupdated_at: \"{ua}\"\n---\n\n{body}\n",
                id = note.id,
                title = note.title,
                ca = note.created_at,
                ua = note.updated_at,
                body = note.content,
            );
            fs::write(self.notes_dir().join(fname), md).unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn note(id: &str, title: &str, content: &str) -> Note {
        Note {
            id: id.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            created_at: "2026-01-01".to_string(),
            updated_at: "2026-01-01".to_string(),
            tags: Vec::new(),
            parent_id: None,
            pinned: false,
        }
    }

    #[test]
    fn upsert_then_search_finds_by_content_word() {
        let s = Scratch::new("upsert");
        let idx = NotesIndex::open_or_create(&s.index_dir()).unwrap();
        idx.upsert(&note("1", "笔记标题", "Rust 编程语言入门指南")).unwrap();
        // jieba should match the word "编程" in the body.
        let hits = idx.search("编程", 10).unwrap();
        assert!(hits.iter().any(|h| h.note_id == "1"), "expected hit for 编程");
    }

    #[test]
    fn ngram_matches_single_char_and_arbitrary_substring() {
        let s = Scratch::new("ngram");
        let idx = NotesIndex::open_or_create(&s.index_dir()).unwrap();
        idx.upsert(&note("1", "我的笔记本", "普通正文")).unwrap();
        // Single CJK char — only the n-gram field can recall this.
        let hits = idx.search("记", 10).unwrap();
        assert!(hits.iter().any(|h| h.note_id == "1"), "ngram should match single char 记");
    }

    #[test]
    fn delete_removes_note_from_index() {
        let s = Scratch::new("delete");
        let idx = NotesIndex::open_or_create(&s.index_dir()).unwrap();
        idx.upsert(&note("1", "待删除", "独特关键词xyz")).unwrap();
        assert!(!idx.search("独特关键词xyz", 10).unwrap().is_empty());
        idx.delete("1").unwrap();
        assert!(idx.search("独特关键词xyz", 10).unwrap().is_empty());
    }

    #[test]
    fn upsert_replaces_previous_version() {
        let s = Scratch::new("replace");
        let idx = NotesIndex::open_or_create(&s.index_dir()).unwrap();
        idx.upsert(&note("1", "原标题", "旧内容oldcontent")).unwrap();
        idx.upsert(&note("1", "新标题", "新内容newcontent")).unwrap();
        // Old term gone, new term present — and only one doc for id "1".
        assert!(idx.search("oldcontent", 10).unwrap().is_empty());
        let hits = idx.search("newcontent", 10).unwrap();
        assert_eq!(hits.iter().filter(|h| h.note_id == "1").count(), 1);
    }

    #[test]
    fn rebuild_from_disk_reads_markdown_files() {
        let s = Scratch::new("rebuild");
        let idx = NotesIndex::open_or_create(&s.index_dir()).unwrap();
        // Write two notes to disk (the index has not seen them yet).
        s.write_md(&note("a", "编程笔记", "学习编程语言"));
        s.write_md(&note("b", "日记", "今天天气不错"));
        idx.rebuild_from_disk(&s.notes_dir()).unwrap();
        let hits = idx.search("编程", 10).unwrap();
        assert!(hits.iter().any(|h| h.note_id == "a"));
        // A fresh rebuild should be idempotent (no duplicate docs).
        idx.rebuild_from_disk(&s.notes_dir()).unwrap();
        let hits2 = idx.search("编程", 10).unwrap();
        assert_eq!(hits2.iter().filter(|h| h.note_id == "a").count(), 1);
    }

    #[test]
    fn open_or_create_is_idempotent_on_existing_dir() {
        let s = Scratch::new("reopen");
        let idx = NotesIndex::open_or_create(&s.index_dir()).unwrap();
        idx.upsert(&note("1", "持久", "可复现内容")).unwrap();
        drop(idx);
        // Reopening the same dir must keep the document (not wipe it).
        let idx2 = NotesIndex::open_or_create(&s.index_dir()).unwrap();
        assert!(idx2.search("可复现内容", 10).unwrap().iter().any(|h| h.note_id == "1"));
    }
}

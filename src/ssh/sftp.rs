//! SFTP subsystem wrapper over `russh-sftp`.
//!
//! Exposes a simplified API (list dir, upload, download, mkdir, rename,
//! remove) used by the UI. A [`SftpSessionWrapper`] is opened from an
//! authenticated [`SshConnection`](super::client::SshConnection) via
//! [`SshConnection::open_sftp`](crate::ssh::client::SshConnection::open_sftp).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, TimeZone};
use russh::client::{Handle, Handler};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// A file or directory entry on the remote server.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// File name (basename only, no path).
    pub name: String,
    /// Absolute remote path (`parent + "/" + name`).
    pub path: String,
    /// File size in bytes (0 for directories).
    pub size: u64,
    pub is_dir: bool,
    pub is_symlink: bool,
    /// Unix permissions (e.g. `0o755`).
    pub permissions: u32,
    /// Last modification time, if reported by the server.
    pub modified: Option<DateTime<Local>>,
}

/// Transfer progress events sent from the actor to the UI during up/download.
#[derive(Debug, Clone)]
pub enum TransferProgress {
    /// A transfer has started.
    Started {
        /// Stable id for this transfer (filename for now).
        id: String,
        filename: String,
        total: u64,
        /// True = upload, false = download.
        uploading: bool,
    },
    /// Bytes transferred so far.
    Progress {
        id: String,
        transferred: u64,
        total: u64,
    },
    /// Transfer completed successfully. `note` carries non-fatal warnings
    /// (e.g. entries skipped during a directory upload) for the UI to show.
    Completed {
        id: String,
        note: Option<String>,
    },
    /// Transfer failed with an error.
    Error { id: String, message: String },
}

/// Wrapper around `russh_sftp::client::SftpSession` with Verve-flavored APIs.
pub struct SftpSessionWrapper {
    sftp: SftpSession,
}

impl SftpSessionWrapper {
    /// Open an SFTP subsystem on the given (already authenticated) russh handle.
    pub(crate) async fn open<H: Handler<Error = russh::Error>>(
        handle: &mut Handle<H>,
    ) -> Result<Self, String> {
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| format!("打开 session 失败: {e}"))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| format!("请求 sftp 子系统失败: {e}"))?;
        let stream = channel.into_stream();
        let sftp = SftpSession::new(stream)
            .await
            .map_err(|e| format!("SFTP 初始化失败: {e}"))?;
        Ok(Self { sftp })
    }

    /// Return the canonical (absolute) form of a path.
    pub async fn canonicalize(&self, path: &str) -> Result<String, String> {
        self.sftp
            .canonicalize(path)
            .await
            .map_err(|e| format!("realpath 失败: {e}"))
    }

    /// List entries in a directory, sorted: directories first, then files,
    /// alphabetical within each group. "." and ".." are filtered out.
    pub async fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, String> {
        let read_dir = self
            .sftp
            .read_dir(path)
            .await
            .map_err(|e| format!("读取目录失败: {e}"))?;

        let parent = path.trim_end_matches('/');
        let mut entries: Vec<FileEntry> = Vec::new();
        for dir_entry in read_dir {
            let name = dir_entry.file_name().to_string();
            if name == "." || name == ".." {
                continue;
            }
            let attrs = dir_entry.metadata();
            let full_path = format!("{}/{}", parent, name);

            let file_type = attrs.file_type();
            let is_dir = file_type.is_dir();
            let is_symlink = file_type.is_symlink();

            // For symlinks, is_dir above is based on the link itself (LNK bit),
            // not the target. That's fine: we treat symlinks as their own kind
            // in the UI; the user can follow by double-click (try_stat target).
            let size = attrs.size.unwrap_or(0);
            let permissions = attrs.permissions.unwrap_or(0);
            let modified = attrs
                .mtime
                .and_then(|t| Local.timestamp_opt(t as i64, 0).single());

            entries.push(FileEntry {
                name,
                path: full_path,
                size,
                is_dir,
                is_symlink,
                permissions,
                modified,
            });
        }

        // Sort: dirs first, then alphabetical (case-insensitive).
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        Ok(entries)
    }

    /// Get metadata for a single path.
    pub async fn stat(&self, path: &str) -> Result<FileEntry, String> {
        let attrs = self
            .sftp
            .metadata(path)
            .await
            .map_err(|e| format!("stat 失败: {e}"))?;
        let name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .to_string();
        let file_type = attrs.file_type();
        Ok(FileEntry {
            name,
            path: path.to_string(),
            size: attrs.size.unwrap_or(0),
            is_dir: file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
            permissions: attrs.permissions.unwrap_or(0),
            modified: attrs
                .mtime
                .and_then(|t| Local.timestamp_opt(t as i64, 0).single()),
        })
    }

    /// Create a directory.
    pub async fn mkdir(&self, path: &str) -> Result<(), String> {
        self.sftp
            .create_dir(path)
            .await
            .map_err(|e| format!("创建目录失败: {e}"))
    }

    /// Remove a regular file.
    pub async fn remove_file(&self, path: &str) -> Result<(), String> {
        self.sftp
            .remove_file(path)
            .await
            .map_err(|e| format!("删除文件失败: {e}"))
    }

    /// Remove an empty directory.
    pub async fn remove_dir(&self, path: &str) -> Result<(), String> {
        self.sftp
            .remove_dir(path)
            .await
            .map_err(|e| format!("删除目录失败: {e}"))
    }

    /// Recursively remove a directory and all of its contents (files,
    /// subdirectories, symlinks). Mirrors `rm -rf`.
    ///
    /// Strategy: for each child entry, first try `remove_file` (works for
    /// regular files and symlinks, including symlinks to directories). If
    /// that fails with an error that looks like "this is a directory", fall
    /// back to recursing. This handles servers whose readdir attrs do not
    /// reliably report the file type (missing permissions field, etc.).
    pub async fn remove_dir_all(&self, path: &str) -> Result<(), String> {
        // Normalize: strip trailing slashes, but "/" must stay as "/".
        let path = if path == "/" {
            "/"
        } else {
            path.trim_end_matches('/')
        };
        if path.is_empty() {
            return Err("拒绝删除根目录".into());
        }
        let entries = self.list_dir(path).await?;
        for entry in entries {
            let child = &entry.path;
            if entry.is_dir {
                // Reported as a directory: recurse.
                Box::pin(self.remove_dir_all(child)).await?;
            } else {
                // Try as a file (also covers symlinks). On failure, fall back
                // to treating it as a directory (covers mislabeled entries).
                match self.remove_file(child).await {
                    Ok(()) => {}
                    Err(file_err) => match Box::pin(self.remove_dir_all(child)).await {
                        Ok(()) => {}
                        Err(dir_err) => {
                            return Err(format!(
                                "删除 {} 失败（文件: {}；目录: {}）",
                                child, file_err, dir_err
                            ));
                        }
                    },
                }
            }
        }
        self.remove_dir(path)
            .await
            .map_err(|e| format!("删除目录 {} 失败（可能非空）: {}", path, e))
    }

    /// Rename (move) a remote path.
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        self.sftp
            .rename(from, to)
            .await
            .map_err(|e| format!("重命名失败: {e}"))
    }

    /// Download a remote file to a local path, streaming in 64 KiB chunks and
    /// reporting progress through `progress_tx`.
    pub async fn download_file(
        &self,
        remote_path: &str,
        local_path: &Path,
        progress_tx: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<(), String> {
        let id = remote_path.to_string();
        let filename = Path::new(remote_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(remote_path)
            .to_string();

        let mut remote = self
            .sftp
            .open(remote_path)
            .await
            .map_err(|e| format!("打开远程文件失败: {e}"))?;

        let meta = remote
            .metadata()
            .await
            .map_err(|e| format!("读取远程文件元信息失败: {e}"))?;
        let total = meta.size.unwrap_or(0);

        let _ = progress_tx.send(TransferProgress::Started {
            id: id.clone(),
            filename: filename.clone(),
            total,
            uploading: false,
        });

        let mut local = tokio::fs::File::create(local_path)
            .await
            .map_err(|e| format!("创建本地文件失败: {e}"))?;

        let mut buf = vec![0u8; 64 * 1024];
        let mut transferred: u64 = 0;
        loop {
            let n = remote
                .read(&mut buf)
                .await
                .map_err(|e| format!("读取远程文件失败: {e}"))?;
            if n == 0 {
                break;
            }
            local
                .write_all(&buf[..n])
                .await
                .map_err(|e| format!("写入本地文件失败: {e}"))?;
            transferred += n as u64;
            let _ = progress_tx.send(TransferProgress::Progress {
                id: id.clone(),
                transferred,
                total,
            });
        }
        local
            .flush()
            .await
            .map_err(|e| format!("刷新本地文件失败: {e}"))?;
        drop(local);

        let _ = progress_tx.send(TransferProgress::Completed {
            id,
            note: None,
        });
        Ok(())
    }

    /// Upload a local file to a remote path, streaming in 64 KiB chunks.
    pub async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        progress_tx: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<(), String> {
        let id = remote_path.to_string();
        let filename = Path::new(remote_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(remote_path)
            .to_string();

        let total = tokio::fs::metadata(local_path)
            .await
            .map_err(|e| format!("读取本地文件元信息失败: {e}"))?
            .len();

        let _ = progress_tx.send(TransferProgress::Started {
            id: id.clone(),
            filename,
            total,
            uploading: true,
        });

        self.stream_upload(local_path, remote_path, |transferred, total| {
            let _ = progress_tx.send(TransferProgress::Progress {
                id: id.clone(),
                transferred,
                total,
            });
        })
        .await?;

        let _ = progress_tx.send(TransferProgress::Completed {
            id,
            note: None,
        });
        Ok(())
    }

    /// Recursively upload a local directory to `remote_dir`, which is created
    /// if missing (existing files inside are overwritten). The whole tree is
    /// reported as a single transfer: `total` is the sum of all file sizes
    /// and each chunk updates the aggregate `transferred` count.
    pub async fn upload_dir(
        &self,
        local_dir: &Path,
        remote_dir: &str,
        progress_tx: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<(), String> {
        let id = remote_dir.to_string();
        let dirname = local_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("folder")
            .to_string();

        // Started goes out before the local walk: if the walk fails (e.g. an
        // unreadable path) the error still attaches to a visible transfer
        // row instead of being silently dropped by the UI. The real total
        // arrives with the first Progress event.
        let _ = progress_tx.send(TransferProgress::Started {
            id: id.clone(),
            filename: format!("{dirname}/"),
            total: 0,
            uploading: true,
        });

        let (dirs, files, skipped) = collect_local_tree(local_dir)?;
        let total: u64 = files.iter().map(|f| f.size).sum();

        // Create the destination folder itself plus every subdirectory,
        // parents before children (that's how collect_local_tree sorts them).
        self.ensure_remote_dir(remote_dir).await?;
        for rel in &dirs {
            if rel.is_empty() {
                continue; // the root, handled above
            }
            let target = Self::join_path(remote_dir, rel);
            self.ensure_remote_dir(&target).await?;
        }

        let mut done: u64 = 0;
        for f in &files {
            let target = Self::join_path(remote_dir, &f.rel);
            let tx = progress_tx.clone();
            let id = id.clone();
            self.stream_upload(&f.local, &target, move |transferred, _| {
                let _ = tx.send(TransferProgress::Progress {
                    id: id.clone(),
                    transferred: done + transferred,
                    total,
                });
            })
            .await?;
            done += f.size;
        }

        let note = if skipped.is_empty() {
            None
        } else {
            let preview: Vec<String> = skipped.iter().take(3).cloned().collect();
            Some(format!(
                "跳过 {} 项（无权限/链接/特殊文件）：{}{}",
                skipped.len(),
                preview.join("、"),
                if skipped.len() > 3 { " …" } else { "" }
            ))
        };
        let _ = progress_tx.send(TransferProgress::Completed { id, note });
        Ok(())
    }

    /// Create `path` on the remote, tolerating an already-existing directory
    /// (plain mkdir fails then, which would abort re-uploads into an
    /// existing folder). Any other failure is returned.
    async fn ensure_remote_dir(&self, path: &str) -> Result<(), String> {
        if let Err(e) = self.mkdir(path).await {
            match self.stat(path).await {
                Ok(entry) if entry.is_dir => {}
                _ => return Err(format!("创建目录 {path} 失败: {e}")),
            }
        }
        Ok(())
    }

    /// Core of `upload_file`/`upload_dir`: stream `local_path` to a newly
    /// created/truncated `remote_path` in 64 KiB chunks, invoking
    /// `on_progress(transferred, total)` after every chunk. Returns the file
    /// size in bytes.
    async fn stream_upload(
        &self,
        local_path: &Path,
        remote_path: &str,
        mut on_progress: impl FnMut(u64, u64),
    ) -> Result<u64, String> {
        let mut local = tokio::fs::File::open(local_path)
            .await
            .map_err(|e| format!("打开本地文件失败: {e}"))?;
        let total = local
            .metadata()
            .await
            .map_err(|e| format!("读取本地文件元信息失败: {e}"))?
            .len();

        let mut remote = self
            .sftp
            .open_with_flags(
                remote_path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|e| format!("打开远程文件失败: {e}"))?;

        let mut buf = vec![0u8; 64 * 1024];
        let mut transferred: u64 = 0;
        loop {
            let n = local
                .read(&mut buf)
                .await
                .map_err(|e| format!("读取本地文件失败: {e}"))?;
            if n == 0 {
                break;
            }
            remote
                .write_all(&buf[..n])
                .await
                .map_err(|e| format!("写入远程文件失败: {e}"))?;
            transferred += n as u64;
            on_progress(transferred, total);
        }
        remote
            .flush()
            .await
            .map_err(|e| format!("刷新远程文件失败: {e}"))?;
        drop(remote);
        Ok(total)
    }

    /// Join a base directory with a child component using "/" as separator.
    /// Handles trailing slashes on the base path.
    pub fn join_path(base: &str, child: &str) -> String {
        let base = base.trim_end_matches('/');
        format!("{}/{}", base, child)
    }
}

/// A regular file discovered inside a local directory tree.
struct LocalFileEntry {
    /// Absolute local path.
    local: PathBuf,
    /// Path relative to the walk root, always '/'-separated so it can be
    /// appended to a remote path directly.
    rel: String,
    size: u64,
}

/// Walk `root` and collect its subdirectories (relative paths, root itself
/// included as the empty string) and all regular files. The third element
/// lists everything that was NOT uploaded (relative paths): unreadable
/// entries, directory symlinks, broken links and other node types (fifos,
/// sockets, devices) so the UI can report them instead of silently dropping
/// content.
///
/// Directory symlinks are never followed (walkdir default), so cycles are
/// impossible. Symlinks to files are followed and uploaded as regular files.
///
/// The dir/file lists are sorted so that parents come before children, which
/// keeps remote `mkdir` calls in `upload_dir` valid in a single pass.
fn collect_local_tree(
    root: &Path,
) -> Result<(Vec<String>, Vec<LocalFileEntry>, Vec<String>), String> {
    let rel_of = |p: &Path| -> String {
        p.strip_prefix(root)
            .unwrap_or(p)
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut dirs: Vec<String> = vec![String::new()];
    let mut files: Vec<LocalFileEntry> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        // One unreadable entry must not abort the whole upload; record it as
        // skipped and keep going.
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                skipped.push(match err.path() {
                    Some(p) => rel_of(p),
                    None => err.to_string(),
                });
                continue;
            }
        };
        if entry.depth() == 0 {
            continue; // the root itself
        }
        let rel = rel_of(entry.path());
        let file_type = entry.file_type();
        if file_type.is_dir() {
            dirs.push(rel);
        } else if file_type.is_file() {
            files.push(LocalFileEntry {
                local: entry.path().to_path_buf(),
                rel,
                size: entry.metadata().map(|m| m.len()).unwrap_or(0),
            });
        } else {
            // Symlinks and special nodes. metadata() follows symlinks: file
            // symlinks upload as regular files; directory symlinks, broken
            // links and fifos/sockets/devices are skipped and reported.
            match std::fs::metadata(entry.path()) {
                Ok(meta) if meta.is_file() => files.push(LocalFileEntry {
                    local: entry.path().to_path_buf(),
                    rel,
                    size: meta.len(),
                }),
                _ => skipped.push(rel),
            }
        }
    }
    dirs.sort();
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok((dirs, files, skipped))
}

/// Return a human-readable size string: "1.2 KB", "3.4 MB", etc.
pub fn humanize_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", bytes, UNITS[i])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

/// Parse a remote path into (parent, breadcrumb segments).
/// `/home/user/foo` → `("/", ["home", "user", "foo"])`; the first segment
/// represents the root so clicking it returns to `/`.
pub fn split_path(path: &str) -> (String, Vec<(String, String)>) {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return ("/".to_string(), vec![("/".to_string(), "/".to_string())]);
    }
    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    let mut segs: Vec<(String, String)> = Vec::with_capacity(parts.len() + 1);
    segs.push(("/".to_string(), "/".to_string()));
    let mut acc = String::new();
    for p in &parts {
        acc.push('/');
        acc.push_str(p);
        segs.push((acc.clone(), p.to_string()));
    }
    (path.to_string(), segs)
}

/// Given a parent directory and a bare filename (possibly with a single
/// subdirectory prefix), build the full remote path.
pub fn build_remote_path(parent: &str, filename: &str) -> PathBuf {
    let mut p = PathBuf::from(parent.trim_end_matches('/'));
    p.push(filename);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize() {
        assert_eq!(humanize_size(0), "0 B");
        assert_eq!(humanize_size(512), "512 B");
        assert_eq!(humanize_size(1024), "1.0 KB");
        assert_eq!(humanize_size(1536), "1.5 KB");
        assert_eq!(humanize_size(1048576), "1.0 MB");
        assert_eq!(humanize_size(1073741824), "1.0 GB");
    }

    #[test]
    fn split_simple() {
        let (parent, segs) = split_path("/home/user/foo");
        assert_eq!(parent, "/home/user/foo");
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[0], ("/".to_string(), "/".to_string()));
        assert_eq!(segs[1].0, "/home");
        assert_eq!(segs[1].1, "home");
        assert_eq!(segs[3].0, "/home/user/foo");
    }

    #[test]
    fn split_root() {
        let (parent, segs) = split_path("/");
        assert_eq!(parent, "/");
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn join_paths() {
        assert_eq!(
            SftpSessionWrapper::join_path("/home/user", "foo"),
            "/home/user/foo"
        );
        assert_eq!(
            SftpSessionWrapper::join_path("/home/user/", "foo"),
            "/home/user/foo"
        );
    }

    /// Minimal temp dir that does not pull in the `tempfile` crate.
    fn temp_root(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "verve_sftp_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).expect("create temp dir");
        p
    }

    #[test]
    fn collect_tree_lists_dirs_files_and_sizes() {
        let root = temp_root("walk");
        std::fs::create_dir_all(root.join("sub/deep")).expect("mkdir");
        std::fs::create_dir_all(root.join("empty_dir")).expect("mkdir");
        std::fs::write(root.join("a.txt"), b"abc").expect("write");
        std::fs::write(root.join("sub/b.txt"), b"12345").expect("write");
        // Empty file, plus a Chinese filename to exercise multibyte paths.
        std::fs::write(root.join("sub/deep/c.txt"), b"").expect("write");
        std::fs::create_dir_all(root.join("中文目录")).expect("mkdir");
        std::fs::write(root.join("中文目录/文件.txt"), b"hi").expect("write");

        let (dirs, files, skipped) = collect_local_tree(&root).expect("walk");
        assert_eq!(
            dirs,
            vec![
                String::new(),
                "empty_dir".to_string(),
                "sub".to_string(),
                "sub/deep".to_string(),
                "中文目录".to_string(),
            ]
        );
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(
            rels,
            vec!["a.txt", "sub/b.txt", "sub/deep/c.txt", "中文目录/文件.txt"]
        );
        assert_eq!(files.iter().map(|f| f.size).sum::<u64>(), 3 + 5 + 0 + 2);
        // Local paths stay absolute and point at real files.
        assert!(files.iter().all(|f| f.local.is_absolute()));
        assert!(files.iter().all(|f| f.local.is_file()));
        assert!(skipped.is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn collect_tree_on_empty_dir_yields_root_only() {
        let root = temp_root("walk_empty");
        let (dirs, files, skipped) = collect_local_tree(&root).expect("walk");
        assert_eq!(dirs, vec![String::new()]);
        assert!(files.is_empty());
        assert!(skipped.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn collect_tree_reports_dir_symlinks_as_skipped() {
        use std::os::unix::fs::symlink;

        let root = temp_root("walk_symlink");
        std::fs::create_dir_all(root.join("real")).expect("mkdir");
        std::fs::write(root.join("real/f.txt"), b"x").expect("write");
        symlink(root.join("real"), root.join("dirlink")).expect("symlink");
        // A file symlink is followed and uploaded as a regular file.
        symlink(root.join("real/f.txt"), root.join("filelink")).expect("symlink");

        let (dirs, files, skipped) = collect_local_tree(&root).expect("walk");
        assert!(dirs.contains(&"real".to_string()));
        assert!(!dirs.contains(&"dirlink".to_string()));
        assert_eq!(skipped, vec!["dirlink".to_string()]);
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert!(rels.contains(&"real/f.txt"));
        assert!(rels.contains(&"filelink"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn collect_tree_tolerates_unreadable_entries() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("walk_unreadable");
        std::fs::write(root.join("ok.txt"), b"x").expect("write");
        let locked = root.join("locked");
        std::fs::create_dir_all(&locked).expect("mkdir");
        std::fs::write(locked.join("s.txt"), b"y").expect("write");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
            .expect("chmod 000");

        let (files, skipped, ok) = match collect_local_tree(&root) {
            Ok((_, files, skipped)) => (files, skipped, true),
            Err(_) => (Vec::new(), Vec::new(), false),
        };
        // Restore access before cleanup, whatever the assertions below do.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
            .expect("restore mode");
        assert!(ok, "walk should not abort on an unreadable entry");
        // Root ignores permission bits — only assert when the skip happened.
        if !skipped.is_empty() {
            assert_eq!(skipped, vec!["locked".to_string()]);
        }
        assert!(files.iter().any(|f| f.rel == "ok.txt"));

        std::fs::remove_dir_all(&root).ok();
    }
}

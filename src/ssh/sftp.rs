//! SFTP subsystem wrapper over `russh-sftp`.
//!
//! Exposes a simplified API (list dir, upload, download, mkdir, rename,
//! remove) used by the UI. A [`SftpSessionWrapper`] is opened from an
//! authenticated [`SshConnection`](super::client::SshConnection) via
//! [`SshConnection::open_sftp`](crate::ssh::client::SshConnection::open_sftp).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, TimeZone};
use russh::client::{Handler, Handle};
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
    Progress { id: String, transferred: u64, total: u64 },
    /// Transfer completed successfully.
    Completed { id: String },
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
                    Err(file_err) => {
                        match Box::pin(self.remove_dir_all(child)).await {
                            Ok(()) => {}
                            Err(dir_err) => {
                                return Err(format!(
                                    "删除 {} 失败（文件: {}；目录: {}）",
                                    child, file_err, dir_err
                                ));
                            }
                        }
                    }
                }
            }
        }
        self.remove_dir(path).await.map_err(|e| {
            format!("删除目录 {} 失败（可能非空）: {}", path, e)
        })
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

        let _ = progress_tx.send(TransferProgress::Completed { id });
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

        let mut local = tokio::fs::File::open(local_path)
            .await
            .map_err(|e| format!("打开本地文件失败: {e}"))?;
        let local_meta = local
            .metadata()
            .await
            .map_err(|e| format!("读取本地文件元信息失败: {e}"))?;
        let total = local_meta.len();

        let _ = progress_tx.send(TransferProgress::Started {
            id: id.clone(),
            filename,
            total,
            uploading: true,
        });

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
            let _ = progress_tx.send(TransferProgress::Progress {
                id: id.clone(),
                transferred,
                total,
            });
        }
        remote
            .flush()
            .await
            .map_err(|e| format!("刷新远程文件失败: {e}"))?;
        drop(remote);

        let _ = progress_tx.send(TransferProgress::Completed { id });
        Ok(())
    }

    /// Join a base directory with a child component using "/" as separator.
    /// Handles trailing slashes on the base path.
    pub fn join_path(base: &str, child: &str) -> String {
        let base = base.trim_end_matches('/');
        format!("{}/{}", base, child)
    }
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
        assert_eq!(SftpSessionWrapper::join_path("/home/user", "foo"), "/home/user/foo");
        assert_eq!(SftpSessionWrapper::join_path("/home/user/", "foo"), "/home/user/foo");
    }
}

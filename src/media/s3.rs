//! S3 / MinIO object operations via rust-s3 (rustls TLS backend).
//!
//! Credentials are never written to logs or returned in errors: every surfaced
//! message is passed through [`desensitize`] to strip accidental key leakage.

use std::time::Duration;

use anyhow::{Context, Result};
use s3::{AddressingStyle, Auth, Client, Credentials};

use crate::media::config::S3Config;

/// Scheme + host used both for the client base URL and the embedded object URL.
fn endpoint_url(cfg: &S3Config) -> String {
    format!("{}://{}", cfg.scheme(), cfg.endpoint.trim())
}

/// Build an async S3 client. Uses path-style addressing by default for
/// MinIO / self-hosted compatibility; region defaults to `us-east-1` when empty
/// (MinIO accepts it for signing).
pub fn build_client(cfg: &S3Config) -> Result<Client> {
    let creds = Credentials::new(&cfg.access_key, &cfg.secret_key)
        .context("build S3 credentials")?;
    let region = if cfg.region.trim().is_empty() {
        "us-east-1".to_string()
    } else {
        cfg.region.trim().to_string()
    };
    let style = if cfg.path_style {
        AddressingStyle::Path
    } else {
        AddressingStyle::VirtualHosted
    };
    Client::builder(endpoint_url(cfg))
        .context("parse S3 endpoint")?
        .region(region)
        .auth(Auth::Static(creds))
        .addressing_style(style)
        .timeout(Duration::from_secs(30))
        .build()
        .context("build S3 client")
}

/// Public path-style object URL: `{scheme}://{endpoint}/{bucket}/{key}`.
pub fn public_object_url(cfg: &S3Config, key: &str) -> String {
    format!("{}/{}/{}", endpoint_url(cfg), cfg.bucket.trim(), key)
}

/// Connectivity check via HEAD bucket. The error (if any) is desensitized.
pub async fn test_connection(cfg: &S3Config) -> Result<()> {
    let client = build_client(cfg)?;
    client
        .buckets()
        .head(cfg.bucket.trim())
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{}", desensitize(&e.to_string(), cfg)))?;
    Ok(())
}

/// Upload `bytes` to `{bucket}/{key}` with a single PUT. Returns the public URL.
pub async fn put_object(
    cfg: &S3Config,
    key: &str,
    bytes: Vec<u8>,
    content_type: &str,
) -> Result<String> {
    let client = build_client(cfg)?;
    let put = client
        .objects()
        .put(cfg.bucket.trim(), key)
        .content_type(content_type)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    put.body_bytes(bytes)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{}", desensitize(&e.to_string(), cfg)))?;
    Ok(public_object_url(cfg, key))
}

/// Delete an object (used by note-deletion media cleanup in a later phase).
pub async fn delete_object(cfg: &S3Config, key: &str) -> Result<()> {
    let client = build_client(cfg)?;
    client
        .objects()
        .delete(cfg.bucket.trim(), key)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{}", desensitize(&e.to_string(), cfg)))?;
    Ok(())
}

/// Multipart-upload large bytes (video) to `{bucket}/{key}`.
///
/// `part_size` is the chunk size in bytes (S3 requires ≥ 5 MiB for all but the
/// last part). `on_progress(done_parts, total_parts)` is invoked after each part
/// completes. Files at or below `part_size` fall back to a single PUT. On any
/// part failure the upload is aborted and the error (desensitized) returned.
pub async fn put_object_multipart(
    cfg: &S3Config,
    key: &str,
    bytes: Vec<u8>,
    part_size: usize,
    content_type: &str,
    mut on_progress: impl FnMut(u32, u32),
) -> Result<String> {
    let client = build_client(cfg)?;
    let bucket = cfg.bucket.trim();
    let total_len = bytes.len();

    // Small file (or zero/invalid part size): single PUT.
    if part_size == 0 || total_len <= part_size {
        let url = put_object(cfg, key, bytes, content_type).await?;
        on_progress(1, 1);
        return Ok(url);
    }

    let created = client
        .objects()
        .create_multipart_upload(bucket, key)
        .content_type(content_type)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{}", desensitize(&e.to_string(), cfg)))?;
    let upload_id = created.upload_id.clone();

    // ceil(total_len / part_size); total_len > part_size here so this is >= 2.
    let total_parts = ((total_len + part_size - 1) / part_size) as u32;

    let mut etags: Vec<(u32, String)> = Vec::new();
    let mut start = 0usize;
    let mut part_num = 1u32;
    let mut failed: Option<anyhow::Error> = None;
    while start < total_len {
        let end = (start + part_size).min(total_len);
        let chunk = bytes[start..end].to_vec();
        match client
            .objects()
            .upload_part(bucket, key, &upload_id, part_num)
            .body_bytes(chunk)
            .send()
            .await
        {
            Ok(out) => match out.etag.clone() {
                Some(etag) => {
                    etags.push((part_num, etag));
                    on_progress(part_num, total_parts);
                }
                None => {
                    failed = Some(anyhow::anyhow!("missing etag for part {part_num}"));
                    break;
                }
            },
            Err(e) => {
                failed = Some(anyhow::anyhow!(
                    "{}",
                    desensitize(&e.to_string(), cfg)
                ));
                break;
            }
        }
        start = end;
        part_num += 1;
    }

    if let Some(err) = failed {
        let _ = client
            .objects()
            .abort_multipart_upload(bucket, key, &upload_id)
            .send()
            .await;
        return Err(err);
    }

    let mut req = client
        .objects()
        .complete_multipart_upload(bucket, key, &upload_id);
    for (n, etag) in etags {
        req = req.part(n, etag).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    req.send()
        .await
        .map_err(|e| anyhow::anyhow!("{}", desensitize(&e.to_string(), cfg)))?;
    Ok(public_object_url(cfg, key))
}

/// Strip accidental access/secret key leakage from a message before display.
fn desensitize(msg: &str, cfg: &S3Config) -> String {
    let mut out = msg.to_string();
    if !cfg.access_key.is_empty() {
        out = out.replace(&cfg.access_key, "<ak>");
    }
    if !cfg.secret_key.is_empty() {
        out = out.replace(&cfg.secret_key, "<sk>");
    }
    out
}

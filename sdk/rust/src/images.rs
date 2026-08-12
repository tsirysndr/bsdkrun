//! Host-level image operations.

use serde_json::Value;

use crate::error::Result;
use crate::process::run_checked;
use crate::types::ImageInfo;

/// List downloaded images (pulled OCI images + fetched BSD images).
pub fn list() -> Result<Vec<ImageInfo>> {
    let res = run_checked(["images", "--json"], "bsdkrun images")?;
    let raw = if res.stdout.trim().is_empty() {
        "[]".to_string()
    } else {
        res.stdout
    };
    let rows: Value = serde_json::from_str(&raw)?;
    Ok(rows
        .as_array()
        .map(|rows| rows.iter().map(ImageInfo::from_row).collect())
        .unwrap_or_default())
}

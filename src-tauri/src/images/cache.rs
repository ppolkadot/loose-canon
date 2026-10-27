//! The on-disk image cache.
//!
//! Files live under a root directory the caller chooses (the application cache directory, found
//! through Tauri's path API), at `images/<listing id>/<position>.<ext>`. The database stores only
//! that **relative** path. Names are built from integers and a fixed extension list, never from
//! anything a website supplied, and [`resolve`] re-validates a stored path before it is turned
//! back into a file path, so a damaged or tampered row cannot point outside the cache.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

use super::decode::StoredFormat;

/// Writes `bytes` atomically (temp file, then rename) and returns the relative path to store.
pub fn write_image(
    root: &Path,
    listing_id: i64,
    position: u32,
    format: StoredFormat,
    bytes: &[u8],
) -> io::Result<String> {
    let relative = format!("images/{listing_id}/{position}.{}", format.extension());
    let path = resolve(root, &relative)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsafe cache path"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", format.extension()));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, &path)?;
    Ok(relative)
}

/// Turns a stored relative path back into a path under `root`, or `None` if it is not a plain
/// relative path made of normal components (no `..`, no root, no drive prefix, no separators
/// other than `/`, no empty parts).
#[must_use]
pub fn resolve(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.contains(['\\', ':', '\0']) || relative.starts_with('/') {
        return None;
    }
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        let mut components = Path::new(part).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) if name == part => path.push(part),
            _ => return None,
        }
    }
    Some(path)
}

#[cfg(test)]
mod tests;

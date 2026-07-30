use crate::errors::friendly_io_error;
use anyhow::{anyhow, Result};
use std::path::Path;

/// Hapus permanen (bukan ke Recycle Bin). Dipanggil setelah user konfirmasi 'y'.
pub fn delete_permanent(path: &Path, is_dir: bool) -> Result<()> {
    let result = if is_dir {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|e| anyhow!(friendly_io_error(&e)))
}

/// Buka file pakai aplikasi default OS (di Windows = sama kayak double-click).
pub fn open_with_default_app(path: &Path) -> Result<()> {
    open::that(path).map_err(|e| anyhow!(friendly_io_error(&e)))
}

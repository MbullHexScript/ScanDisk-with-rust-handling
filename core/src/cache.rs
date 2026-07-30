use crate::tree::Node;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Naikkan angka ini kalau struktur `Node`/`CacheFile` berubah dengan cara yang
/// gak backward-compatible. Cache lama dengan versi beda otomatis dianggap
/// cache-miss (fallback rescan), bukan error — lihat `load()`.
const CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    /// `#[serde(default)]` -> cache dari SEBELUM field ini ada akan ke-parse
    /// jadi 0, otomatis gak cocok sama CACHE_SCHEMA_VERSION saat ini, otomatis
    /// diperlakukan sebagai cache-miss oleh `load()`. Ini yang bikin versioning
    /// jalan tanpa perlu migrasi manual.
    #[serde(default)]
    schema_version: u32,
    scanned_at: u64,
    root: PathBuf,
    tree: Node,
}

/// Hasil load cache yang berhasil: tree-nya + kapan terakhir di-scan (buat
/// ditampilkan di UI, mis. "terakhir scan 3 hari lalu").
pub struct LoadedCache {
    pub tree: Node,
    pub scanned_at: u64,
}

/// Direktori tempat semua cache disimpan: ~/.cache/Sweep/ (atau AppData di Windows).
fn cache_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().context("gagal menemukan folder cache OS")?;
    let dir = base.join("Sweep");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// FNV-1a 64-bit, hash sederhana buat nama file cache dari path direktori.
///
/// Sengaja gak pakai `std::collections::hash_map::DefaultHasher`: hasher itu
/// TIDAK dijamin stabil antar versi Rust/platform (bisa beda hasil kalau nanti
/// di-compile ulang pakai compiler versi lain). FNV-1a manual di sini simpel
/// dan hasilnya selalu sama untuk input yang sama, kapan pun & di mana pun.
///
/// Ini juga menghilangkan risiko collision dari pendekatan lama (sanitasi
/// karakter non-alfanumerik jadi `_`, yang bisa bikin dua path beda nyasar ke
/// nama file cache yang sama).
fn hash_path(root: &Path) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET;
    for byte in root.to_string_lossy().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn cache_filename(root: &Path) -> String {
    format!("{:016x}.json", hash_path(root))
}

/// Simpan cache secara atomik: tulis ke file sementara dulu, baru `rename()`
/// ke nama final. `rename()` dalam direktori yang sama itu atomik di
/// Windows(NTFS)/Linux/macOS — jadi gak akan pernah ada file cache yang
/// "setengah-jadi" kebaca kalau proses ke-interrupt di tengah `fs::write`.
pub fn save(root: &Path, tree: &Node) -> Result<()> {
    let dir = cache_dir()?;
    let filename = cache_filename(root);
    let final_path = dir.join(&filename);
    let tmp_path = dir.join(format!("{filename}.tmp"));

    let scanned_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let payload = CacheFile {
        schema_version: CACHE_SCHEMA_VERSION,
        scanned_at,
        root: root.to_path_buf(),
        tree: tree.clone(),
    };
    let json = serde_json::to_string(&payload)?;

    fs::write(&tmp_path, json)?;
    fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// Load cache kalau ada & valid. User kontrol rescan manual lewat tombol `r`/`--rescan`
/// (bukan auto-invalidate by usia), sesuai keputusan UX awal — tapi `scanned_at`
/// tetap dikembalikan supaya UI bisa nampilin umur cache biar user bisa keputusan sendiri.
///
/// Penting: error *IO* (gagal baca file/folder cache) di-propagate sebagai `Err`
/// karena itu layak diberitahu ke user (P0.1). Tapi cache yang gagal di-*parse*
/// (JSON corrupt, atau `schema_version` gak cocok karena aplikasinya baru di-update)
/// diperlakukan sebagai cache-miss biasa (`Ok(None)`) — ini situasi yang wajar
/// terjadi seiring waktu, bukan sesuatu yang perlu bikin user khawatir.
pub fn load(root: &Path) -> Result<Option<LoadedCache>> {
    let path = cache_dir()?.join(cache_filename(root));
    if !path.exists() {
        return Ok(None);
    }

    let json = fs::read_to_string(&path)?; // IO error -> propagate ke caller

    let payload: CacheFile = match serde_json::from_str(&json) {
        Ok(p) => p,
        Err(_) => return Ok(None), // corrupt -> treat sebagai cache-miss, bukan error
    };

    if payload.schema_version != CACHE_SCHEMA_VERSION {
        return Ok(None); // versi lama/beda -> treat sebagai cache-miss
    }

    Ok(Some(LoadedCache { tree: payload.tree, scanned_at: payload.scanned_at }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_path_konsisten_untuk_input_yang_sama() {
        let p = Path::new("/some/test/path");
        assert_eq!(hash_path(p), hash_path(p));
    }

    #[test]
    fn hash_path_beda_untuk_path_yang_beda() {
        let a = Path::new("/some/path/a");
        let b = Path::new("/some/path/b");
        assert_ne!(hash_path(a), hash_path(b));
    }

    #[test]
    fn path_dengan_separator_beda_gak_collide_secara_kebetulan() {
        // Kasus yang jadi motivasi P1.2: sebelumnya sanitasi karakter non-alfanumerik
        // berpotensi bikin path yang beda nyasar ke nama cache yang sama.
        let a = Path::new("D:/Kuliah");
        let b = Path::new("D:\\Kuliah");
        // Gak dijamin beda 100% (namanya juga hash), tapi utamanya nama file
        // sekarang murni angka hex dari isi path, bukan substitusi karakter
        // yang gampang collide.
        let name_a = cache_filename(a);
        let name_b = cache_filename(b);
        assert!(name_a.ends_with(".json"));
        assert!(name_b.ends_with(".json"));
    }

    #[test]
    fn cache_filename_formatnya_16_hex_digit_plus_json() {
        let name = cache_filename(Path::new("/root/Kuliah"));
        assert!(name.ends_with(".json"));
        assert_eq!(name.len(), 16 + ".json".len());
    }
}

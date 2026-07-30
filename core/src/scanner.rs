use crate::errors::friendly_io_error;
use crate::tree::Node;
use rayon::prelude::*;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Scan satu direktori secara rekursif & paralel, hasilkan tree lengkap dengan size.
/// Symlink sengaja di-skip (biar gak infinite loop / hitung ganda).
pub fn scan_dir(path: &Path) -> Node {
    let entries: Vec<_> = match fs::read_dir(path) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(e) => {
            // PENTING: sebelumnya ini diam-diam balikin folder kosong (0 byte),
            // jadi keliatan sama persis kayak folder yang beneran kosong.
            // Sekarang error-nya direkam di node, biar UI bisa nunjukin
            // ⚠ dan pesannya, bukan menyesatkan user.
            return Node::new_dir_with_error(path.to_path_buf(), friendly_io_error(&e));
        }
    };

    let children: Vec<Node> = entries
        .par_iter()
        .filter_map(|entry| {
            let entry_path = entry.path();
            let meta = entry.metadata().ok()?;

            if meta.is_symlink() {
                return None;
            }

            if meta.is_dir() {
                Some(scan_dir(&entry_path))
            } else {
                Some(Node::new_file(entry_path, meta.len()))
            }
        })
        .collect();

    Node::new_dir(path.to_path_buf(), children)
}

/// Rescan satu subdirektori aja (dipanggil pas user tekan 'r' di dalam suatu folder),
/// hasilnya dipakai buat replace node lama di tree utama.
pub fn rescan_single(path: &Path) -> Node {
    let mut node = scan_dir(path);
    node.sort_recursive();
    node
}

/// Event yang dikirim dari thread scan ke render loop, buat nampilin progress.
pub enum ScanProgress {
    /// 1 item top-level (langsung di bawah root) selesai di-scan.
    ItemDone { name: String, done: usize, total: usize },
    /// Scan selesai total (baik tuntas maupun dibatalkan di tengah jalan).
    /// Kalau dibatalkan, `cancelled` = true dan `node` berisi hasil parsial
    /// (jangan disimpan ke cache).
    Finished { node: Node, cancelled: bool },
}

/// Scan direktori sambil lapor progress lewat `tx`, dan bisa dihentikan lebih awal
/// lewat `cancel`. Dipanggil dari thread terpisah supaya render loop gak nge-freeze
/// selama scan direktori besar berjalan.
///
/// Progress cuma dilapor di level top-level (item langsung di bawah `path`), bukan
/// tiap file di kedalaman berapa pun — cukup buat kasih rasa "kerja" tanpa flood
/// channel dengan ribuan event per detik.
///
/// Catatan soal cancel: rayon `par_iter()` gak bisa langsung menghentikan semua
/// thread di tengah jalan. `cancel` di sini sifatnya best-effort — mencegah item
/// top-level baru mulai di-scan begitu di-set true, tapi item yang sudah kadung
/// jalan akan tetap selesai wajar dulu.
pub fn scan_dir_with_progress(path: &Path, tx: Sender<ScanProgress>, cancel: Arc<AtomicBool>) {
    let entries: Vec<_> = match fs::read_dir(path) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(e) => {
            let node = Node::new_dir_with_error(path.to_path_buf(), friendly_io_error(&e));
            let _ = tx.send(ScanProgress::Finished { node, cancelled: false });
            return;
        }
    };

    let total = entries.len();
    let cancelled_flag = AtomicBool::new(false);

    let children: Vec<Node> = entries
        .par_iter()
        .enumerate()
        .filter_map(|(i, entry)| {
            if cancel.load(Ordering::Relaxed) {
                cancelled_flag.store(true, Ordering::Relaxed);
                return None;
            }

            let entry_path = entry.path();
            let meta = entry.metadata().ok()?;

            let node = if meta.is_symlink() {
                return None;
            } else if meta.is_dir() {
                scan_dir(&entry_path)
            } else {
                Node::new_file(entry_path, meta.len())
            };

            // Channel disconnect (receiver sudah drop, mis. UI ditutup paksa)
            // aman diabaikan — tinggal terusin scan sampai selesai/cancel.
            let _ = tx.send(ScanProgress::ItemDone {
                name: node.name.clone(),
                done: i + 1,
                total,
            });

            Some(node)
        })
        .collect();

    let mut root = Node::new_dir(path.to_path_buf(), children);
    root.sort_recursive();

    let _ = tx.send(ScanProgress::Finished {
        node: root,
        cancelled: cancelled_flag.load(Ordering::Relaxed),
    });
}

use crate::errors::friendly_io_error;
use crate::tree::Node;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Waktu modifikasi direktori yang disimpan ke cache (nanodetik sejak epoch).
///
/// Direktori yang baru dimodifikasi < 2 detik lalu sengaja disimpan sebagai `None`
/// (teknik "racy timestamp" seperti di git): kalau ada file ditambah di direktori itu
/// dalam tick timestamp yang sama setelah scan, mtime-nya bisa tetap sama dan perubahan
/// gak kedeteksi. Dengan `None`, direktori itu pasti dibaca ulang di validasi berikutnya.
fn mtime_for_cache(meta: &fs::Metadata) -> Option<u64> {
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).unwrap_or_default();
    if age.as_secs() < 2 {
        return None;
    }
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_nanos() as u64)
}

/// mtime mentah (tanpa aturan racy) buat dibandingkan dengan yang tersimpan di cache.
fn mtime_raw(meta: &fs::Metadata) -> Option<u64> {
    Some(meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos() as u64)
}

/// Scan satu direktori secara rekursif & paralel, hasilkan tree lengkap dengan size.
/// Symlink sengaja di-skip (biar gak infinite loop / hitung ganda).
pub fn scan_dir(path: &Path) -> Node {
    // mtime dibaca SEBELUM read_dir: kalau ada perubahan di tengah scan, mtime yang
    // tersimpan lebih lama dari kenyataan, jadi validasi berikutnya pasti menangkapnya.
    let mtime = fs::metadata(path).ok().and_then(|m| mtime_for_cache(&m));
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

    let mut node = Node::new_dir(path.to_path_buf(), children);
    node.mtime = mtime;
    node
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
    /// Hasil `reconcile_with_progress`. Kalau `cancelled`, `node` adalah tree cache
    /// yang baru tersinkron sebagian (masih valid dipakai, tapi jangan disimpan ke cache).
    Reconciled { node: Node, stats: ReconcileStats, cancelled: bool },
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
    let mtime = fs::metadata(path).ok().and_then(|m| mtime_for_cache(&m));
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
    root.mtime = mtime;
    root.sort_recursive();

    let _ = tx.send(ScanProgress::Finished {
        node: root,
        cancelled: cancelled_flag.load(Ordering::Relaxed),
    });
}

// ───────────────────────── Sinkronisasi cache dengan disk ─────────────────────────

/// Ringkasan perubahan yang ditemukan saat cache disinkronkan dengan kondisi disk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    /// File/folder yang ada di cache tapi sudah tidak ada di disk.
    pub removed: usize,
    /// File/folder baru di disk yang belum ada di cache.
    pub added: usize,
    /// File yang ukurannya berubah (atau tipenya berganti file <-> folder).
    pub updated: usize,
}

impl ReconcileStats {
    pub fn is_unchanged(&self) -> bool {
        self.removed == 0 && self.added == 0 && self.updated == 0
    }
}

pub struct ReconcileOutcome {
    /// `None` kalau direktori root-nya sendiri sudah tidak ada.
    pub node: Option<Node>,
    pub stats: ReconcileStats,
    pub cancelled: bool,
}

struct Ctx<'a> {
    cancel: &'a AtomicBool,
    cancelled: AtomicBool,
    removed: AtomicUsize,
    added: AtomicUsize,
    updated: AtomicUsize,
}

impl<'a> Ctx<'a> {
    fn new(cancel: &'a AtomicBool) -> Self {
        Self {
            cancel,
            cancelled: AtomicBool::new(false),
            removed: AtomicUsize::new(0),
            added: AtomicUsize::new(0),
            updated: AtomicUsize::new(0),
        }
    }

    fn bump(counter: &AtomicUsize) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn stats(&self) -> ReconcileStats {
        ReconcileStats {
            removed: self.removed.load(Ordering::Relaxed),
            added: self.added.load(Ordering::Relaxed),
            updated: self.updated.load(Ordering::Relaxed),
        }
    }
}

/// Laporan progress di level top-level (dipakai oleh `reconcile_with_progress`).
struct Progress<'a> {
    tx: &'a Sender<ScanProgress>,
    done: AtomicUsize,
    total: AtomicUsize,
}

impl Progress<'_> {
    fn report(&self, name: &str) {
        let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.tx.send(ScanProgress::ItemDone {
            name: name.to_string(),
            done,
            total: self.total.load(Ordering::Relaxed),
        });
    }
}

/// Cocokkan `old` (hasil cache) dengan kondisi disk SEKARANG, tanpa scan ulang penuh:
///
/// - item yang sudah tidak ada di disk dibuang (inilah yang bikin folder lama
///   "hantu" tetap tampil kalau cache dipercaya mentah-mentah),
/// - file yang ukurannya berubah di-update,
/// - direktori yang mtime-nya berubah dibaca ulang isinya, jadi file/folder baru
///   ikut ketemu; direktori yang mtime-nya sama cuma di-stat anak-anaknya
///   (tanpa `read_dir`), jadi jauh lebih murah dari scan penuh.
pub fn reconcile(old: Node, cancel: &AtomicBool) -> ReconcileOutcome {
    let ctx = Ctx::new(cancel);
    let node = reconcile_root(old, &ctx, None);
    finish(node, &ctx)
}

/// Sama seperti `reconcile`, tapi dipanggil dari thread terpisah: lapor progress
/// top-level lewat `tx` dan kirim `ScanProgress::Reconciled` di akhir.
pub fn reconcile_with_progress(old: Node, tx: Sender<ScanProgress>, cancel: Arc<AtomicBool>) {
    let ctx = Ctx::new(&cancel);
    let root_path = old.path.clone();
    let progress = Progress { tx: &tx, done: AtomicUsize::new(0), total: AtomicUsize::new(old.children.len()) };
    let node = reconcile_root(old, &ctx, Some(&progress));
    let outcome = finish(node, &ctx);

    let node = outcome.node.unwrap_or_else(|| {
        Node::new_dir_with_error(root_path, "Direktori tidak ditemukan".to_string())
    });
    let _ = tx.send(ScanProgress::Reconciled {
        node,
        stats: outcome.stats,
        cancelled: outcome.cancelled,
    });
}

fn finish(node: Option<Node>, ctx: &Ctx) -> ReconcileOutcome {
    let node = node.map(|mut n| {
        n.sort_recursive();
        n
    });
    ReconcileOutcome {
        node,
        stats: ctx.stats(),
        cancelled: ctx.cancelled.load(Ordering::Relaxed),
    }
}

fn reconcile_root(old: Node, ctx: &Ctx, progress: Option<&Progress>) -> Option<Node> {
    match fs::symlink_metadata(&old.path) {
        Ok(meta) if meta.is_dir() => Some(reconcile_dir(old, &meta, ctx, progress)),
        Ok(_) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        // Gak bisa di-stat karena alasan lain (mis. permission): jangan buang data lama.
        Err(_) => Some(old),
    }
}

fn reconcile_node(old: Node, ctx: &Ctx) -> Option<Node> {
    if ctx.cancel.load(Ordering::Relaxed) {
        ctx.cancelled.store(true, Ordering::Relaxed);
        return Some(old); // dibatalkan: biarkan apa adanya
    }

    let meta = match fs::symlink_metadata(&old.path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ctx::bump(&ctx.removed);
            return None;
        }
        // Gak bisa diverifikasi (mis. permission): pertahankan data lama.
        Err(_) => return Some(old),
    };

    // Symlink sengaja di-skip oleh scanner, jadi kalau item ini sekarang jadi symlink
    // perlakukan sama seperti hilang.
    if meta.file_type().is_symlink() {
        Ctx::bump(&ctx.removed);
        return None;
    }

    match (meta.is_dir(), old.is_dir) {
        (true, true) => Some(reconcile_dir(old, &meta, ctx, None)),
        (true, false) => {
            // dulu file, sekarang folder
            Ctx::bump(&ctx.updated);
            Some(scan_dir(&old.path))
        }
        (false, true) => {
            // dulu folder, sekarang file
            Ctx::bump(&ctx.updated);
            Some(Node::new_file(old.path, meta.len()))
        }
        (false, false) => {
            let mut old = old;
            if old.size != meta.len() {
                Ctx::bump(&ctx.updated);
                old.size = meta.len();
            }
            Some(old)
        }
    }
}

fn reconcile_dir(old: Node, meta: &fs::Metadata, ctx: &Ctx, progress: Option<&Progress>) -> Node {
    let path = old.path.clone();
    let mtime_now = mtime_raw(meta);
    let stored_mtime = mtime_for_cache(meta);

    // Struktur dianggap gak berubah HANYA kalau mtime sama persis dengan yang tersimpan
    // dan direktori ini dulu berhasil dibaca (kalau dulu error, coba lagi sekarang).
    let structure_unchanged =
        old.scan_error.is_none() && old.mtime.is_some() && old.mtime == mtime_now;

    let report = |n: &Option<Node>, fallback: &str| {
        if let Some(p) = progress {
            p.report(n.as_ref().map(|n| n.name.as_str()).unwrap_or(fallback));
        }
    };

    let children: Vec<Node> = if structure_unchanged {
        old.children
            .into_par_iter()
            .filter_map(|child| {
                let name = child.name.clone();
                let r = reconcile_node(child, ctx);
                report(&r, &name);
                r
            })
            .collect()
    } else {
        let entries: Vec<fs::DirEntry> = match fs::read_dir(&path) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(e) => return Node::new_dir_with_error(path, friendly_io_error(&e)),
        };

        let mut old_by_path: HashMap<PathBuf, Node> =
            old.children.into_iter().map(|c| (c.path.clone(), c)).collect();

        let work: Vec<(fs::DirEntry, Option<Node>)> = entries
            .into_iter()
            .map(|e| {
                let old_child = old_by_path.remove(&e.path());
                (e, old_child)
            })
            .collect();

        // Sisa di old_by_path = ada di cache, tapi gak ada lagi di direktori ini.
        for _ in old_by_path.drain() {
            Ctx::bump(&ctx.removed);
        }

        if let Some(p) = progress {
            // jumlah item top-level bisa berubah kalau isi root berubah
            p.total.store(work.len(), Ordering::Relaxed);
        }

        work.into_par_iter()
            .filter_map(|(entry, old_child)| {
                let entry_path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let r = match old_child {
                    Some(c) => reconcile_node(c, ctx),
                    None => {
                        let m = entry.metadata().ok()?;
                        if m.file_type().is_symlink() {
                            None
                        } else {
                            Ctx::bump(&ctx.added);
                            Some(if m.is_dir() {
                                scan_dir(&entry_path)
                            } else {
                                Node::new_file(entry_path, m.len())
                            })
                        }
                    }
                };
                report(&r, &name);
                r
            })
            .collect()
    };

    let mut node = Node::new_dir(path, children);
    node.mtime = stored_mtime;
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Direktori sementara yang otomatis dibersihkan.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir()
                .join(format!("sweep_test_{}_{}_{}", name, std::process::id(), n));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn write(&self, rel: &str, bytes: usize) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::File::create(p).unwrap().write_all(&vec![b'x'; bytes]).unwrap();
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// Kumpulkan (path relatif, size, is_dir) seluruh tree, terurut, buat dibandingkan.
    fn flatten(n: &Node, base: &Path, out: &mut Vec<(String, u64, bool)>) {
        let rel = n.path.strip_prefix(base).unwrap().to_string_lossy().to_string();
        out.push((rel, n.size, n.is_dir));
        for c in &n.children {
            flatten(c, base, out);
        }
    }
    fn snapshot(n: &Node, base: &Path) -> Vec<(String, u64, bool)> {
        let mut v = vec![];
        flatten(n, base, &mut v);
        v.sort();
        v
    }

    fn assert_matches_fresh_scan(reconciled: &Node, root: &Path) {
        let fresh = scan_dir(root);
        assert_eq!(snapshot(reconciled, root), snapshot(&fresh, root));
    }

    #[test]
    fn folder_yang_sudah_dihapus_hilang_dari_hasil_reconcile() {
        // Skenario bug asli: cache masih nyimpen folder yang sudah dihapus di luar Sweep.
        let t = TempDir::new("hapus");
        t.write("semester1/a.bin", 1000);
        t.write("semester2/b.bin", 2000);
        t.write("lama/c.bin", 5000);
        let cached = scan_dir(t.path());
        assert_eq!(cached.size, 8000);

        fs::remove_dir_all(t.path().join("lama")).unwrap();

        let out = reconcile(cached, &no_cancel());
        let node = out.node.unwrap();
        assert_eq!(node.size, 3000);
        assert!(node.children.iter().all(|c| c.name != "lama"));
        assert_eq!(out.stats, ReconcileStats { removed: 1, added: 0, updated: 0 });
        assert_matches_fresh_scan(&node, t.path());
    }

    #[test]
    fn item_baru_terdeteksi_walau_scan_baru_saja_dilakukan() {
        // mtime direktori yang masih "segar" disimpan None (racy), jadi pasti dibaca ulang.
        let t = TempDir::new("baru_racy");
        t.write("a.bin", 100);
        let cached = scan_dir(t.path());

        t.write("folder_baru/x.bin", 700);
        t.write("b.bin", 50);

        let out = reconcile(cached, &no_cancel());
        let node = out.node.unwrap();
        assert_eq!(node.size, 850);
        assert_eq!(out.stats.added, 2); // folder_baru + b.bin
        assert_matches_fresh_scan(&node, t.path());
    }

    #[test]
    fn perubahan_terdeteksi_lewat_mtime_untuk_cache_yang_sudah_lama() {
        // Jalur mtime sungguhan: scan > 2 detik lalu, baru ada perubahan.
        let t = TempDir::new("mtime");
        t.write("sub/a.bin", 100);
        t.write("sub/dalam/b.bin", 200);
        std::thread::sleep(Duration::from_millis(2300));
        let cached = scan_dir(t.path());
        let sub = cached.children.iter().find(|c| c.name == "sub").unwrap();
        assert!(sub.mtime.is_some(), "mtime direktori lama harus tersimpan");

        t.write("sub/dalam/c.bin", 300); // tambah (mengubah mtime 'dalam')
        fs::remove_file(t.path().join("sub/a.bin")).unwrap(); // hapus (mengubah mtime 'sub')

        let out = reconcile(cached, &no_cancel());
        let node = out.node.unwrap();
        assert_eq!(node.size, 500);
        assert_eq!(out.stats.removed, 1);
        assert_eq!(out.stats.added, 1);
        assert_matches_fresh_scan(&node, t.path());
    }

    #[test]
    fn perubahan_ukuran_file_terdeteksi() {
        let t = TempDir::new("ukuran");
        t.write("a.bin", 100);
        std::thread::sleep(Duration::from_millis(2300));
        let cached = scan_dir(t.path());

        t.write("a.bin", 4000); // timpa isi file -> mtime direktori TIDAK berubah

        let out = reconcile(cached, &no_cancel());
        let node = out.node.unwrap();
        assert_eq!(node.size, 4000);
        assert_eq!(out.stats, ReconcileStats { removed: 0, added: 0, updated: 1 });
    }

    #[test]
    fn tidak_ada_perubahan_berarti_stats_nol_dan_hasil_identik() {
        let t = TempDir::new("sama");
        t.write("a.bin", 10);
        t.write("d/b.bin", 20);
        std::thread::sleep(Duration::from_millis(2300));
        let cached = scan_dir(t.path());

        let out = reconcile(cached, &no_cancel());
        assert!(out.stats.is_unchanged());
        assert!(!out.cancelled);
        assert_matches_fresh_scan(&out.node.unwrap(), t.path());
    }

    #[test]
    fn root_yang_sudah_hilang_balikin_none() {
        let t = TempDir::new("root_hilang");
        t.write("a.bin", 10);
        let cached = scan_dir(t.path());
        fs::remove_dir_all(t.path()).unwrap();

        let out = reconcile(cached, &no_cancel());
        assert!(out.node.is_none());
    }

    #[test]
    fn file_berubah_jadi_folder_dan_sebaliknya() {
        let t = TempDir::new("tipe");
        t.write("x", 100); // file
        t.write("y/z.bin", 200); // folder
        let cached = scan_dir(t.path());

        fs::remove_file(t.path().join("x")).unwrap();
        t.write("x/isi.bin", 300); // x jadi folder
        fs::remove_dir_all(t.path().join("y")).unwrap();
        t.write("y", 50); // y jadi file

        let out = reconcile(cached, &no_cancel());
        let node = out.node.unwrap();
        assert_eq!(node.size, 350);
        assert_matches_fresh_scan(&node, t.path());
    }

    #[test]
    fn reconcile_dibatalkan_tetap_mengembalikan_tree_utuh() {
        let t = TempDir::new("batal");
        t.write("a.bin", 100);
        t.write("d/b.bin", 200);
        let cached = scan_dir(t.path());

        let cancel = AtomicBool::new(true); // sudah dibatalkan dari awal
        let out = reconcile(cached, &cancel);
        assert!(out.cancelled);
        assert_eq!(out.node.unwrap().size, 300); // data lama gak rusak
    }

    #[test]
    fn reconcile_with_progress_mengirim_event_reconciled() {
        let t = TempDir::new("progress");
        t.write("a.bin", 100);
        t.write("d/b.bin", 200);
        let cached = scan_dir(t.path());
        fs::remove_dir_all(t.path().join("d")).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        reconcile_with_progress(cached, tx, Arc::new(AtomicBool::new(false)));

        let mut got = None;
        while let Ok(msg) = rx.try_recv() {
            if let ScanProgress::Reconciled { node, stats, cancelled } = msg {
                got = Some((node, stats, cancelled));
            }
        }
        let (node, stats, cancelled) = got.expect("harus ada event Reconciled");
        assert!(!cancelled);
        assert_eq!(stats.removed, 1);
        assert_eq!(node.size, 100);
    }
}

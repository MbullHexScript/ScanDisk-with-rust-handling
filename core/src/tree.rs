use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Satu node di dalam tree: bisa berupa file atau direktori.
/// `size` untuk direktori adalah total ukuran semua isinya (rekursif).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
    /// Kosong kalau ini file, atau kalau direktori tapi belum di-scan.
    pub children: Vec<Node>,
    /// None = berhasil di-scan normal.
    /// Some(pesan) = direktori ini gagal dibaca (mis. permission denied),
    /// size & children di node ini dianggap tidak lengkap.
    /// `#[serde(default)]` supaya cache lama (belum punya field ini) tetap
    /// bisa di-load tanpa error deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_error: Option<String>,
    /// Waktu modifikasi direktori (nanodetik sejak epoch) saat di-scan. Dipakai
    /// `scanner::reconcile` buat tahu apakah isi direktori berubah (file/folder
    /// ditambah/dihapus) tanpa harus baca ulang semua direktori. `None` untuk file,
    /// cache lama, atau direktori yang baru banget dimodifikasi (lihat `mtime_for_cache`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime: Option<u64>,
}

impl Node {
    pub fn new_file(path: PathBuf, size: u64) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        Self { name, path, size, is_dir: false, children: vec![], scan_error: None, mtime: None }
    }

    pub fn new_dir(path: PathBuf, children: Vec<Node>) -> Self {
        let size = children.iter().map(|c| c.size).sum();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        Self { name, path, size, is_dir: true, children, scan_error: None, mtime: None }
    }

    /// Direktori yang gagal dibaca sama sekali (mis. permission denied).
    /// size & children kosong karena memang gak bisa dibaca isinya.
    pub fn new_dir_with_error(path: PathBuf, error_msg: String) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        Self {
            name,
            path,
            size: 0,
            is_dir: true,
            children: vec![],
            scan_error: Some(error_msg),
            mtime: None,
        }
    }

    /// Sort children dari size terbesar ke terkecil (rekursif, sekali di awal cukup).
    pub fn sort_recursive(&mut self) {
        self.children.sort_by(|a, b| b.size.cmp(&a.size));
        for c in self.children.iter_mut() {
            if c.is_dir {
                c.sort_recursive();
            }
        }
    }

    /// Cari & hapus node dengan path tertentu di manapun dalam tree ini.
    /// Gak langsung ngitung ulang size induk-induknya; panggil `recompute_sizes()`
    /// di root setelah ini supaya seluruh size ke atas ikut ke-update dengan benar.
    pub fn remove_by_path(&mut self, target: &Path) -> bool {
        if let Some(pos) = self.children.iter().position(|c| c.path == target) {
            self.children.remove(pos);
            return true;
        }
        for c in self.children.iter_mut() {
            if c.is_dir && target.starts_with(&c.path) {
                if c.remove_by_path(target) {
                    return true;
                }
            }
        }
        false
    }

    /// Ganti node yang path-nya `target` dengan `replacement` (dipakai setelah rescan
    /// satu subdirektori). Sama seperti `remove_by_path`, panggil `recompute_sizes()`
    /// di root setelahnya.
    pub fn replace_by_path(&mut self, target: &Path, replacement: Node) -> bool {
        if self.path == target {
            *self = replacement;
            return true;
        }
        let mut replacement = Some(replacement);
        for c in self.children.iter_mut() {
            if target.starts_with(&c.path) {
                if c.replace_by_path(target, replacement.take().unwrap()) {
                    return true;
                }
            }
        }
        false
    }

    pub fn recompute_sizes(&mut self) -> u64 {
        if !self.is_dir {
            return self.size;
        }
        self.size = self.children.iter_mut().map(|c| c.recompute_sizes()).sum();
        self.size
    }

    /// Ambil node (mutable) sesuai path, buat keperluan navigasi tanpa rescan.
    pub fn find_mut(&mut self, target: &Path) -> Option<&mut Node> {
        if self.path == target {
            return Some(self);
        }
        for c in self.children.iter_mut() {
            if target.starts_with(&c.path) {
                return c.find_mut(target);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bangun tree test:
    /// /root
    /// ├── a.txt         (100)
    /// ├── sub1/
    /// │   ├── b.txt      (200)
    /// │   └── sub2/
    /// │       └── c.txt  (300)
    /// └── ab/             <- sengaja nama mirip "sub1"/"a" buat test prefix
    ///     └── d.txt       (50)
    fn build_test_tree() -> Node {
        let c = Node::new_file(PathBuf::from("/root/sub1/sub2/c.txt"), 300);
        let sub2 = Node::new_dir(PathBuf::from("/root/sub1/sub2"), vec![c]);
        let b = Node::new_file(PathBuf::from("/root/sub1/b.txt"), 200);
        let sub1 = Node::new_dir(PathBuf::from("/root/sub1"), vec![b, sub2]);
        let a = Node::new_file(PathBuf::from("/root/a.txt"), 100);
        let d = Node::new_file(PathBuf::from("/root/ab/d.txt"), 50);
        let ab = Node::new_dir(PathBuf::from("/root/ab"), vec![d]);
        Node::new_dir(PathBuf::from("/root"), vec![a, sub1, ab])
    }

    #[test]
    fn size_root_adalah_jumlah_semua_file() {
        let root = build_test_tree();
        assert_eq!(root.size, 100 + 200 + 300 + 50);
    }

    #[test]
    fn remove_file_dalam_subdirektori_dalam_lalu_recompute_size_ke_atas() {
        let mut root = build_test_tree();

        // Hapus c.txt yang ada di /root/sub1/sub2/ (2 level dari root)
        let removed = root.remove_by_path(Path::new("/root/sub1/sub2/c.txt"));
        assert!(removed, "c.txt harusnya ketemu & kehapus");

        root.recompute_sizes();

        // sub2 sekarang kosong
        let sub2 = root.find_mut(Path::new("/root/sub1/sub2")).unwrap();
        assert_eq!(sub2.size, 0);
        assert_eq!(sub2.children.len(), 0);

        // sub1 size berkurang 300 (dari b.txt 200 + sub2 0)
        let sub1 = root.find_mut(Path::new("/root/sub1")).unwrap();
        assert_eq!(sub1.size, 200);

        // root size ikut berkurang sampai ke atas
        assert_eq!(root.size, 100 + 200 + 0 + 50);
    }

    #[test]
    fn remove_folder_menghapus_semua_isinya_sekaligus() {
        let mut root = build_test_tree();

        let removed = root.remove_by_path(Path::new("/root/sub1"));
        assert!(removed);
        root.recompute_sizes();

        assert!(root.find_mut(Path::new("/root/sub1")).is_none());
        assert_eq!(root.size, 100 + 50); // cuma a.txt + ab/d.txt tersisa
    }

    #[test]
    fn remove_path_yang_gak_ada_balikin_false_dan_gak_ubah_apa_apa() {
        let mut root = build_test_tree();
        let original_size = root.size;

        let removed = root.remove_by_path(Path::new("/root/tidak_ada/xxx.txt"));
        assert!(!removed);

        root.recompute_sizes();
        assert_eq!(root.size, original_size);
    }

    #[test]
    fn replace_subdirektori_dengan_hasil_rescan_baru() {
        let mut root = build_test_tree();

        // Simulasi: sub1 di-rescan ulang, sekarang cuma punya 1 file baru (500 byte)
        let new_file = Node::new_file(PathBuf::from("/root/sub1/new.txt"), 500);
        let new_sub1 = Node::new_dir(PathBuf::from("/root/sub1"), vec![new_file]);

        let replaced = root.replace_by_path(Path::new("/root/sub1"), new_sub1);
        assert!(replaced);
        root.recompute_sizes();

        let sub1 = root.find_mut(Path::new("/root/sub1")).unwrap();
        assert_eq!(sub1.children.len(), 1);
        assert_eq!(sub1.size, 500);

        // root ikut ke-update: a.txt(100) + sub1 baru(500) + ab(50)
        assert_eq!(root.size, 100 + 500 + 50);
    }

    #[test]
    fn replace_root_itu_sendiri() {
        let mut root = build_test_tree();
        let fresh_root = Node::new_dir(PathBuf::from("/root"), vec![]);

        let replaced = root.replace_by_path(Path::new("/root"), fresh_root);
        assert!(replaced);
        assert_eq!(root.children.len(), 0);
        assert_eq!(root.size, 0);
    }

    /// Edge case yang paling penting divalidasi: `/root/a.txt` (file) dan
    /// `/root/ab` (folder) punya nama yang mirip secara string ("a" adalah
    /// prefix dari "ab"), tapi `Path::starts_with` bekerja per-komponen path
    /// (bukan per-karakter string), jadi seharusnya TIDAK saling ketuker.
    #[test]
    fn find_mut_tidak_ketuker_antar_path_yang_mirip_secara_string() {
        let mut root = build_test_tree();

        let found_a = root.find_mut(Path::new("/root/a.txt")).unwrap();
        assert_eq!(found_a.name, "a.txt");
        assert!(!found_a.is_dir);

        let found_ab = root.find_mut(Path::new("/root/ab")).unwrap();
        assert_eq!(found_ab.name, "ab");
        assert!(found_ab.is_dir);
        assert_eq!(found_ab.children.len(), 1);
    }

    #[test]
    fn find_mut_path_yang_gak_ada_balikin_none() {
        let mut root = build_test_tree();
        assert!(root.find_mut(Path::new("/root/gak/ada/sama/sekali")).is_none());
    }

    #[test]
    fn sort_recursive_urutan_besar_ke_kecil_di_semua_level() {
        let mut root = build_test_tree();
        root.sort_recursive();

        // level teratas: sub1(500) > a.txt(100) > ab(50)
        assert_eq!(root.children[0].name, "sub1");
        assert_eq!(root.children[1].name, "a.txt");
        assert_eq!(root.children[2].name, "ab");

        // di dalam sub1: sub2(300) > b.txt(200)
        let sub1 = &root.children[0];
        assert_eq!(sub1.children[0].name, "sub2");
        assert_eq!(sub1.children[1].name, "b.txt");
    }
}

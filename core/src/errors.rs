use std::io;

/// Ubah `io::Error` jadi pesan Bahasa Indonesia yang bisa langsung ditampilkan
/// ke user di TUI, tanpa detail teknis OS yang membingungkan.
pub fn friendly_io_error(e: &io::Error) -> String {
    use io::ErrorKind::*;
    match e.kind() {
        PermissionDenied => "Akses ditolak (permission denied)".to_string(),
        NotFound => "Direktori atau file tidak ditemukan".to_string(),
        AlreadyExists => "Sudah ada file/folder dengan nama yang sama".to_string(),
        _ => {
            // Beberapa error umum lain yang belum stabil ErrorKind-nya di semua
            // versi Rust (mis. StorageFull) dicek lewat pesan mentahnya.
            let raw = e.to_string();
            if raw.contains("No space left") || raw.contains("disk full") {
                "Disk penuh, tidak ada ruang tersisa".to_string()
            } else {
                format!("Terjadi kesalahan: {raw}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_denied_diterjemahkan() {
        let e = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        assert_eq!(friendly_io_error(&e), "Akses ditolak (permission denied)");
    }

    #[test]
    fn not_found_diterjemahkan() {
        let e = io::Error::new(io::ErrorKind::NotFound, "missing");
        assert_eq!(friendly_io_error(&e), "Direktori atau file tidak ditemukan");
    }

    #[test]
    fn error_lain_fallback_ke_pesan_generik() {
        let e = io::Error::new(io::ErrorKind::Other, "aneh banget");
        let msg = friendly_io_error(&e);
        assert!(msg.starts_with("Terjadi kesalahan"));
    }
}

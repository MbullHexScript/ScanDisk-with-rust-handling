# Sweep

TUI buat lihat & bersihin direktori yang paling makan storage, navigasi pakai keyboard,
tanpa perlu scan ulang tiap buka folder.

## Build (Windows / cross-platform)
Butuh Rust (install dari https://rustup.rs/):
```
cargo build --release
```
Binary hasil ada di `target/release/Sweep.exe`

## Pakai
```
Sweep D:\Kuliah
```

## Opsi
| Opsi | Fungsi |
|---|---|
| *(default)* | Pakai cache, tapi **dicocokkan dulu dengan kondisi disk**: folder/file yang sudah dihapus dibuang, yang baru ditambahkan, ukuran yang berubah diperbarui. |
| `-r`, `--rescan` | Abaikan cache, scan ulang dari nol. |
| `-f`, `--fast` | Pakai cache apa adanya tanpa validasi (instan, tapi bisa basi kalau isi disk berubah di luar Sweep). |

## Keybinding
| Key | Aksi |
|---|---|
| ↑↓ / j k | Pindah item |
| Enter / l | Masuk folder / buka file (pakai app default Windows) |
| Backspace / h | Naik ke folder induk |
| d | Hapus **permanen** (dengan konfirmasi y/n) |
| r | Rescan folder yang lagi dibuka |
| q | Keluar |

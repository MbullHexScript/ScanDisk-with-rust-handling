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

## Keybinding
| Key | Aksi |
|---|---|
| ↑↓ / j k | Pindah item |
| Enter / l | Masuk folder / buka file (pakai app default Windows) |
| Backspace / h | Naik ke folder induk |
| d | Hapus **permanen** (dengan konfirmasi y/n) |
| r | Rescan folder yang lagi dibuka |
| q | Keluar |

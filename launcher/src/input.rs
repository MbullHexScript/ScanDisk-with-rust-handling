use crate::{App, BrowsingState, Screen};
use anyhow::Result;
use sweep_core::{cache, ops, scanner, scanner::ScanProgress};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::sync::atomic::Ordering;
use std::time::Duration;

pub fn handle_input(app: &mut App) -> Result<()> {
    // Drain progress dari thread scan setiap tick, TERLEPAS dari ada key event
    // atau tidak — biar progress bar jalan mulus meski user gak mencet apa-apa.
    drain_scan_progress(app);

    if !event::poll(Duration::from_millis(150))? {
        return Ok(());
    }

    if let Event::Key(key) = event::read()? {
        // PENTING: di Windows, crossterm ngirim event Press DAN Release buat
        // tiap tombol yang dipencet. Kalau gak di-filter, 1 kali pencet
        // ke-handle 2 kali (efeknya kayak "double klik" pas navigasi).
        // Di Linux/Mac biasanya cuma ada event Press, jadi filter ini aman
        // dipakai di semua platform.
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        match &mut app.screen {
            Screen::Scanning(s) => {
                // Selama scan: cuma Esc/q yang direspon, buat minta batal.
                // Cancellation ini best-effort (lihat komentar di scanner.rs) —
                // item yang udah kadung jalan tetap selesai wajar dulu.
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        s.cancel.store(true, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
            Screen::Browsing(b) => handle_browsing_key(&mut app.should_quit, b, key.code)?,
        }
    }

    Ok(())
}

/// Cek channel progress dari thread scan. Kalau ada `Finished`, transisi
/// `Screen::Scanning` -> `Screen::Browsing` (atau keluar aplikasi kalau di-cancel).
fn drain_scan_progress(app: &mut App) {
    let mut transition = None;
    let mut quit = false;

    if let Screen::Scanning(s) = &mut app.screen {
        while let Ok(msg) = s.rx.try_recv() {
            match msg {
                ScanProgress::ItemDone { name, done, total } => {
                    s.current = name;
                    s.done = done;
                    s.total = total;
                }
                ScanProgress::Finished { mut node, cancelled } => {
                    if cancelled {
                        quit = true;
                    } else {
                        node.sort_recursive();

                        let mut message = s.startup_message.clone();
                        if let Err(e) = cache::save(&s.root_path, &node) {
                            message = Some(format!("Gagal menyimpan cache: {e}"));
                        }
                        let scanned_at = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .ok();

                        transition = Some(Screen::Browsing(BrowsingState {
                            root: node,
                            root_path: s.root_path.clone(),
                            current_path: s.root_path.clone(),
                            cursor: 0,
                            pending_delete: None,
                            message,
                            cache_scanned_at: scanned_at,
                        }));
                    }
                }
            }
        }
    }

    if quit {
        app.should_quit = true;
    }
    if let Some(screen) = transition {
        app.screen = screen;
    }
}

fn handle_browsing_key(should_quit: &mut bool, app: &mut BrowsingState, code: KeyCode) -> Result<()> {
    // Mode konfirmasi delete: cuma respon y/n/esc
    if let Some(target) = app.pending_delete.clone() {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                do_delete(app, &target)?;
                app.pending_delete = None;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.pending_delete = None;
                app.message = Some("Dibatalkan.".into());
            }
            _ => {}
        }
        return Ok(());
    }

    match code {
        KeyCode::Char('q') => *should_quit = true,

        KeyCode::Down | KeyCode::Char('j') => {
            let len = app.current_children().len();
            if len > 0 {
                app.cursor = (app.cursor + 1).min(len - 1);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.cursor = app.cursor.saturating_sub(1);
        }

        KeyCode::Enter | KeyCode::Char('l') => {
            let children = app.current_children();
            if let Some(selected) = children.get(app.cursor) {
                if selected.is_dir {
                    app.current_path = selected.path.clone();
                    app.cursor = 0;
                } else if let Err(e) = ops::open_with_default_app(&selected.path) {
                    app.message = Some(format!("Gagal buka file: {e}"));
                }
            }
        }

        KeyCode::Backspace | KeyCode::Char('h') => {
            if app.current_path != app.root_path {
                let leaving = app.current_path.clone();
                if let Some(parent) = app.current_path.parent() {
                    app.current_path = parent.to_path_buf();
                }
                let siblings = app.current_children();
                app.cursor = siblings
                    .iter()
                    .position(|n| n.path == leaving)
                    .unwrap_or(0);
            }
        }

        KeyCode::Char('d') => {
            let children = app.current_children();
            if let Some(selected) = children.get(app.cursor) {
                app.pending_delete = Some(selected.path.clone());
            }
        }

        KeyCode::Char('r') => {
            let target = app.current_path.clone();
            let new_node = scanner::rescan_single(&target);
            let scan_error = new_node.scan_error.clone();
            if target == app.root_path {
                app.root = new_node;
            } else {
                app.root.replace_by_path(&target, new_node);
                app.root.recompute_sizes();
            }
            app.cursor = 0;
            if scan_error.is_none() {
                app.cache_scanned_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .ok();
            }
            match (scan_error, cache::save(&app.root_path, &app.root)) {
                (Some(err), _) => {
                    app.message = Some(format!("Rescan gagal: {err}"));
                }
                (None, Err(e)) => {
                    app.message = Some(format!("Rescan selesai, tapi cache gagal disimpan: {e}"));
                }
                (None, Ok(())) => {
                    app.message = Some("Rescan selesai.".into());
                }
            }
        }

        _ => {}
    }

    Ok(())
}

fn do_delete(app: &mut BrowsingState, target: &std::path::Path) -> Result<()> {
    let is_dir = app
        .root
        .find_mut(target)
        .map(|n| n.is_dir)
        .unwrap_or(false);

    match ops::delete_permanent(target, is_dir) {
        Ok(()) => {
            app.root.remove_by_path(target);
            app.root.recompute_sizes();
            let len = app.current_children().len();
            if app.cursor >= len {
                app.cursor = len.saturating_sub(1);
            }
            app.message = match cache::save(&app.root_path, &app.root) {
                Ok(()) => Some("Berhasil dihapus.".into()),
                Err(e) => Some(format!("Berhasil dihapus, tapi cache gagal disimpan: {e}")),
            };
        }
        Err(e) => {
            app.message = Some(format!("Gagal hapus: {e}"));
        }
    }
    Ok(())
}

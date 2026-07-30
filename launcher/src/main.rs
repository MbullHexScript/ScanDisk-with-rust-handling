mod ui;
mod input;

use anyhow::Result;
use clap::Parser;
use sweep_core::{cache, scanner, scanner::ScanProgress, Node};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

#[derive(Parser)]
#[command(
    name = "Sweep",
    version,
    about = "Analisa penggunaan storage dan hapus file atau direktori yang memakan banyak ruang.",
    long_about = None,
    after_help = "Examples:\n  sweep\n  sweep D:/Projectku\n  sweep -r\n  sweep -r D:/Projectku"
)]
struct Args {
    /// Direktori yang akan dipindai
    ///
    /// (default: direktori saat ini)
    path: Option<PathBuf>,

    /// Abaikan cache dan lakukan pemindaian ulang
    #[arg(short, long)]
    rescan: bool,
}

/// Layar aplikasi. `Scanning` dipakai selama scan awal (atau --rescan) berjalan
/// di thread terpisah; `Browsing` adalah TUI navigasi normal seperti sebelumnya.
pub enum Screen {
    Scanning(ScanningState),
    Browsing(BrowsingState),
}

pub struct ScanningState {
    pub root_path: PathBuf,
    pub rx: mpsc::Receiver<ScanProgress>,
    pub cancel: Arc<AtomicBool>,
    pub done: usize,
    pub total: usize,
    pub current: String,
    /// Pesan dari sebelum scan dimulai (mis. cache gagal dibaca), dibawa ke
    /// BrowsingState.message begitu scan selesai supaya user tetap lihat.
    pub startup_message: Option<String>,
}

/// State TUI navigasi normal (persis "App" versi sebelumnya, cuma di-rename).
pub struct BrowsingState {
    pub root: Node,
    pub root_path: PathBuf,
    pub current_path: PathBuf,
    pub cursor: usize,
    pub pending_delete: Option<PathBuf>,
    pub message: Option<String>,
    /// Unix timestamp kapan root ini terakhir kali di-scan. `None` kalau
    /// hasil scan baru saja selesai di sesi ini (belum pernah di-cache).
    pub cache_scanned_at: Option<u64>,
}

impl BrowsingState {
    /// Ambil daftar children di folder yang lagi dibuka (sudah ke-sort dari scan awal).
    pub fn current_children(&mut self) -> Vec<Node> {
        self.root
            .find_mut(&self.current_path)
            .map(|n| n.children.clone())
            .unwrap_or_default()
    }
}

pub struct App {
    pub screen: Screen,
    pub should_quit: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let root_path = args
        .path
        .unwrap_or(std::env::current_dir()?)
        .canonicalize()?;

    let mut startup_message: Option<String> = None;

    let cached: Option<cache::LoadedCache> = if args.rescan {
        None
    } else {
        match cache::load(&root_path) {
            Ok(c) => c,
            Err(e) => {
                startup_message = Some(format!("Cache gagal dibaca, scan ulang: {e}"));
                None
            }
        }
    };

    let screen = match cached {
        Some(loaded) => {
            let mut root = loaded.tree;
            root.sort_recursive();
            Screen::Browsing(BrowsingState {
                root,
                root_path: root_path.clone(),
                current_path: root_path,
                cursor: 0,
                pending_delete: None,
                message: startup_message,
                cache_scanned_at: Some(loaded.scanned_at),
            })
        }
        None => {
            // Scan gak dipanggil langsung di sini (blocking) — dijalankan di
            // thread terpisah + lapor progress lewat channel, supaya render
            // loop tetap jalan (nampilin progress bar) selama scan berlangsung.
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let scan_path = root_path.clone();
            let cancel_for_thread = Arc::clone(&cancel);
            thread::spawn(move || {
                scanner::scan_dir_with_progress(&scan_path, tx, cancel_for_thread);
            });

            Screen::Scanning(ScanningState {
                root_path,
                rx,
                cancel,
                done: 0,
                total: 0,
                current: String::new(),
                startup_message,
            })
        }
    };

    let mut app = App { screen, should_quit: false };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    // Restore terminal (harus tetap jalan walau ada error)
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;
        input::handle_input(app)?;
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

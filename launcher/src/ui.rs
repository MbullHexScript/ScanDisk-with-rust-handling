use crate::{App, BrowsingState, Screen, ScanningState};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph},
    Frame,
};

pub fn render(f: &mut Frame, app: &mut App) {
    match &mut app.screen {
        Screen::Scanning(s) => render_scanning(f, s),
        Screen::Browsing(b) => render_browsing(f, b),
    }
}

fn render_scanning(f: &mut Frame, s: &ScanningState) {
    let area = f.size();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // path yang lagi di-scan
            Constraint::Length(3), // progress bar
            Constraint::Length(3), // footer (cara batal)
            Constraint::Min(0),
        ])
        .split(area);

    let path_text = s.root_path.display().to_string();
    f.render_widget(
        Paragraph::new(path_text)
            .block(Block::default().borders(Borders::ALL).title(" Memindai "))
            .style(Style::default().fg(Color::Cyan)),
        chunks[0],
    );

    // total = 0 berarti belum ada item top-level yang selesai dilaporkan;
    // pakai gauge tak-tentu (0%) sambil tetap tampilkan nama item yang lagi
    // dikerjakan, biar user tetap lihat tanda-tanda progress berjalan.
    let ratio = if s.total > 0 {
        (s.done as f64 / s.total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let label = if s.total > 0 {
        format!("{}/{} item", s.done, s.total)
    } else {
        "menghitung...".to_string()
    };
    f.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(" Progress "))
            .gauge_style(Style::default().fg(Color::Green))
            .ratio(ratio)
            .label(label),
        chunks[1],
    );

    let current_text = if s.current.is_empty() {
        "Membaca daftar isi direktori...".to_string()
    } else {
        format!("Memindai: {}", truncate(&s.current, 60))
    };
    f.render_widget(
        Paragraph::new(current_text).style(Style::default().fg(Color::Gray)),
        chunks[2],
    );

    f.render_widget(
        Paragraph::new("Esc/q — batalkan scan")
            .block(Block::default().borders(Borders::ALL))
            .style(Style::default().fg(Color::DarkGray)),
        chunks[3],
    );
}

fn render_browsing(f: &mut Frame, app: &mut BrowsingState) {
    let area = f.size();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // breadcrumb
            Constraint::Min(3),    // list isi folder
            Constraint::Length(3), // footer keybindings / pesan
        ])
        .split(area);

    render_breadcrumb(f, app, chunks[0]);
    render_list(f, app, chunks[1]);
    render_footer(f, app, chunks[2]);

    if let Some(target) = app.pending_delete.clone() {
        render_confirm_popup(f, area, &target);
    }
}

fn render_breadcrumb(f: &mut Frame, app: &BrowsingState, area: Rect) {
    let path_text = app.current_path.display().to_string();
    let text = match app.cache_scanned_at {
        Some(ts) => format!("{path_text}   ({})", cache_age_label(ts)),
        None => path_text,
    };
    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(" Path "))
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(p, area);
}

/// Ubah timestamp terakhir scan jadi label umur yang gampang dibaca,
/// mis. "baru saja", "5 menit lalu", "3 hari lalu".
fn cache_age_label(scanned_at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(scanned_at);
    let age = now.saturating_sub(scanned_at);

    if age < 60 {
        "baru saja".to_string()
    } else if age < 3600 {
        format!("{} menit lalu", age / 60)
    } else if age < 86400 {
        format!("{} jam lalu", age / 3600)
    } else {
        format!("{} hari lalu", age / 86400)
    }
}

fn render_list(f: &mut Frame, app: &mut BrowsingState, area: Rect) {
    let children = app.current_children();
    let max_size = children.iter().map(|c| c.size).max().unwrap_or(1).max(1);

    let items: Vec<ListItem> = children
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let bar_width = 20usize;
            let filled = ((node.size as f64 / max_size as f64) * bar_width as f64) as usize;
            let bar: String = "█".repeat(filled) + &"░".repeat(bar_width - filled);

            let icon = if node.scan_error.is_some() {
                "⚠"
            } else if node.is_dir {
                "📁"
            } else {
                "📄"
            };

            let line = if let Some(err) = &node.scan_error {
                format!("{icon} {:<28} {}", truncate(&node.name, 28), err)
            } else {
                format!(
                    "{icon} {:<28} {:>10}  {bar}",
                    truncate(&node.name, 28),
                    human_size(node.size)
                )
            };

            let style = if i == app.cursor {
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
            } else if node.scan_error.is_some() {
                Style::default().fg(Color::Red)
            } else if node.is_dir {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(Line::from(Span::styled(line, style)))
        })
        .collect();

    let title = format!(" Isi folder ({} item) ", children.len());
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(list, area);
}

fn render_footer(f: &mut Frame, app: &BrowsingState, area: Rect) {
    let text = if let Some(msg) = &app.message {
        msg.clone()
    } else {
        "↑↓/jk pindah  Enter/l buka  Backspace/h naik  d hapus  r rescan  q keluar".to_string()
    };
    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL))
        .alignment(Alignment::Left)
        .style(Style::default().fg(Color::Gray));
    f.render_widget(p, area);
}

fn render_confirm_popup(f: &mut Frame, area: Rect, target: &std::path::Path) {
    let popup_area = centered_rect(60, 20, area);
    f.render_widget(Clear, popup_area);

    let text = format!(
        "Yakin mau hapus PERMANEN:\n{}\n\n[y] Ya    [n] Batal",
        target.display()
    );
    let p = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Konfirmasi Hapus ")
                .style(Style::default().fg(Color::Red)),
        )
        .alignment(Alignment::Center);
    f.render_widget(p, popup_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit_idx])
    }
}

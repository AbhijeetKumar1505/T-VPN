//! Terminal output helpers that behave in cmd.exe, PowerShell, Windows
//! Terminal and Unix shells alike (Unicode symbols degrade to ASCII on
//! legacy consoles; colors honor NO_COLOR / --no-color).

use console::{Emoji, Term, style};
use std::fmt::Display;

pub static OK: Emoji<'_, '_> = Emoji("✔ ", "+ ");
pub static WARN: Emoji<'_, '_> = Emoji("⚠ ", "! ");
pub static ERR: Emoji<'_, '_> = Emoji("✖ ", "x ");
pub static ON: Emoji<'_, '_> = Emoji("● ", "* ");
pub static OFF: Emoji<'_, '_> = Emoji("○ ", "- ");
pub static WAIT: Emoji<'_, '_> = Emoji("◌ ", "~ ");
pub static DOWN: Emoji<'_, '_> = Emoji("↓", "rx ");
pub static UP: Emoji<'_, '_> = Emoji("↑", "tx ");
pub static DOT: Emoji<'_, '_> = Emoji(" · ", " | ");

pub fn success(msg: impl Display) {
    println!("{}{}", style(OK).green().bold(), msg);
}

pub fn warn(msg: impl Display) {
    eprintln!("{}{}", style(WARN).yellow().bold(), style(msg).yellow());
}

pub fn error(msg: impl Display) {
    eprintln!("{}{}", style(ERR).red().bold(), msg);
}

pub fn error_chain(e: &anyhow::Error) {
    eprintln!("{} {}", style("error:").red().bold(), e);
    for cause in e.chain().skip(1) {
        eprintln!("  {} {}", style("caused by:").dim(), cause);
    }
}

pub fn kv(key: &str, value: impl Display) {
    println!("  {:<11}{}", style(key).dim(), value);
}

pub fn hint(msg: impl Display) {
    println!("  {}", style(msg).dim());
}

pub fn bold(s: impl Display) -> String {
    style(s).bold().to_string()
}

pub fn dim(s: impl Display) -> String {
    style(s).dim().to_string()
}

pub fn green(s: impl Display) -> String {
    style(s).green().to_string()
}

pub fn yellow(s: impl Display) -> String {
    style(s).yellow().to_string()
}

pub fn cyan(s: impl Display) -> String {
    style(s).cyan().to_string()
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

pub fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

pub fn ago(secs: Option<u64>) -> String {
    match secs {
        Some(s) => format!("{} ago", duration(s)),
        None => "never".to_string(),
    }
}

pub fn transfer(rx: u64, tx: u64) -> String {
    format!("{}{}  {}{}", DOWN, bytes(rx), UP, bytes(tx))
}

/// Print an aligned table. Cells may contain ANSI colors; the first column of
/// each row may start with a status marker (passed separately).
pub fn table(headers: &[&str], rows: &[(String, Vec<String>)]) {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for (_, cells) in rows {
        for (i, c) in cells.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(console::measure_text_width(c));
        }
    }
    let line = |cells: Vec<String>| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i + 1 == cells.len() {
                    c.clone()
                } else {
                    console::pad_str(c, widths[i], console::Alignment::Left, None).into_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    println!("  {}", dim(line(headers.iter().map(|h| h.to_string()).collect())));
    for (marker, cells) in rows {
        let marker = if marker.is_empty() { "  ".to_string() } else { marker.clone() };
        println!("{marker}{}", line(cells.clone()));
    }
}

/// A single, continuously rewritten status line (only on interactive terminals).
pub struct StatusLine {
    term: Term,
    enabled: bool,
    drawn: bool,
}

impl StatusLine {
    pub fn new(enabled: bool) -> Self {
        let term = Term::stdout();
        let enabled = enabled && term.is_term();
        StatusLine { term, enabled, drawn: false }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set(&mut self, text: &str) {
        if !self.enabled {
            return;
        }
        let width = self.term.size().1 as usize;
        let text = console::truncate_str(text, width.saturating_sub(1).max(20), "…");
        let _ = self.term.clear_line();
        let _ = self.term.write_str(&text);
        self.drawn = true;
    }

    /// Print a permanent line above the status line.
    pub fn println(&mut self, text: impl Display) {
        self.clear();
        println!("{text}");
    }

    pub fn clear(&mut self) {
        if self.drawn {
            let _ = self.term.clear_line();
            self.drawn = false;
        }
    }
}

impl Drop for StatusLine {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Render text (a WireGuard config) as a terminal QR code.
pub fn qr(text: &str) -> anyhow::Result<String> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)?;
    // Light modules as spaces on an explicit white background keeps the code
    // scannable on both dark and light terminal themes.
    let rendered = code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Dark)
        .light_color(Dense1x2::Light)
        .quiet_zone(true)
        .build();
    if console::colors_enabled() {
        Ok(rendered.lines().map(|l| format!("\x1b[30;47m{l}\x1b[0m")).collect::<Vec<_>>().join("\n"))
    } else {
        Ok(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(5), "5s");
        assert_eq!(duration(65), "1m 05s");
        assert_eq!(duration(3_725), "1h 02m 05s");
    }

    #[test]
    fn renders_qr() {
        let q = qr("[Interface]\nPrivateKey = x\n").unwrap();
        assert!(q.lines().count() > 10);
    }
}

//! Terminal presentation: colors, tables, status lines, and prompts.
//!
//! Data goes to stdout; progress, confirmations, and errors go to stderr so
//! output stays pipeable. Colors only appear on a terminal and honor
//! `NO_COLOR` and `CLICOLOR_FORCE`.

use std::fmt::Write as _;
use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::sync::OnceLock;

static STDOUT_COLOR: OnceLock<bool> = OnceLock::new();
static STDERR_COLOR: OnceLock<bool> = OnceLock::new();

fn color_enabled(is_terminal: bool) -> bool {
    if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return false;
    }
    if std::env::var_os("CLICOLOR_FORCE").is_some_and(|value| value != "0") {
        return true;
    }
    is_terminal && std::env::var("TERM").map_or(true, |term| term != "dumb")
}

fn stdout_color() -> bool {
    *STDOUT_COLOR.get_or_init(|| color_enabled(std::io::stdout().is_terminal()))
}

fn stderr_color() -> bool {
    *STDERR_COLOR.get_or_init(|| color_enabled(std::io::stderr().is_terminal()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Plain,
    Bold,
    Dim,
    Green,
    Yellow,
    Red,
    Cyan,
}

impl Style {
    const fn code(self) -> &'static str {
        match self {
            Self::Plain => "",
            Self::Bold => "1",
            Self::Dim => "2",
            Self::Green => "32",
            Self::Yellow => "33",
            Self::Red => "31",
            Self::Cyan => "36",
        }
    }
}

/// Render server and user text on one terminal line. Add our own styling afterwards.
pub(crate) fn terminal_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_control() {
            safe.extend(ch.escape_default());
        } else {
            safe.push(ch);
        }
    }
    safe
}

fn paint_with(enabled: bool, style: Style, text: &str) -> String {
    if !enabled || style == Style::Plain || text.is_empty() {
        return text.to_owned();
    }
    format!("\x1b[{}m{text}\x1b[0m", style.code())
}

/// Style text destined for stdout.
pub(crate) fn paint(style: Style, text: &str) -> String {
    paint_with(stdout_color(), style, &terminal_text(text))
}

/// Style text destined for stderr.
pub(crate) fn epaint(style: Style, text: &str) -> String {
    paint_with(stderr_color(), style, &terminal_text(text))
}

pub(crate) fn success(message: &str) {
    eprintln!("{} {}", epaint(Style::Green, "✓"), terminal_text(message));
}

pub(crate) fn warn(message: &str) {
    eprintln!("{} {}", epaint(Style::Yellow, "!"), terminal_text(message));
}

pub(crate) fn info(message: &str) {
    eprintln!("{} {}", epaint(Style::Cyan, "›"), terminal_text(message));
}

pub(crate) fn hint(message: &str) {
    eprintln!("  {}", epaint(Style::Dim, message));
}

/// Printed width of `text`, ignoring ANSI escapes.
pub(crate) fn display_width(text: &str) -> usize {
    let mut width = 0;
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character == '\x1b' {
            for escaped in chars.by_ref() {
                if escaped == 'm' {
                    break;
                }
            }
        } else {
            width += 1;
        }
    }
    width
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Right,
}

pub(crate) struct Cell {
    text: String,
    style: Style,
}

impl Cell {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self {
            text: terminal_text(&text.into()),
            style: Style::Plain,
        }
    }

    pub(crate) fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: terminal_text(&text.into()),
            style,
        }
    }
}

impl From<String> for Cell {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for Cell {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

/// A borderless, column-aligned table in the style of `gh` and `kubectl`.
pub(crate) struct Table {
    columns: Vec<(&'static str, Align)>,
    rows: Vec<Vec<Cell>>,
    footer: Option<Vec<Cell>>,
}

impl Table {
    pub(crate) fn new(columns: &[(&'static str, Align)]) -> Self {
        Self {
            columns: columns.to_vec(),
            rows: Vec::new(),
            footer: None,
        }
    }

    pub(crate) fn row(&mut self, cells: Vec<Cell>) {
        self.rows.push(cells);
    }

    pub(crate) fn footer(&mut self, cells: Vec<Cell>) {
        self.footer = Some(cells);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(crate) fn render(&self) -> String {
        let mut widths: Vec<usize> = self
            .columns
            .iter()
            .map(|(title, _)| display_width(title))
            .collect();
        for row in self.rows.iter().chain(self.footer.iter()) {
            for (index, cell) in row.iter().enumerate() {
                if let Some(width) = widths.get_mut(index) {
                    *width = (*width).max(display_width(&cell.text));
                }
            }
        }
        let mut out = String::new();
        let header: Vec<Cell> = self
            .columns
            .iter()
            .map(|(title, _)| Cell::styled(*title, Style::Dim))
            .collect();
        self.render_row(&mut out, &header, &widths, true);
        for row in &self.rows {
            self.render_row(&mut out, row, &widths, false);
        }
        if let Some(footer) = &self.footer {
            let rule: usize = widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1);
            let _ = writeln!(out, "{}", paint(Style::Dim, &"─".repeat(rule)));
            self.render_row(&mut out, footer, &widths, false);
        }
        out
    }

    fn render_row(&self, out: &mut String, row: &[Cell], widths: &[usize], header: bool) {
        let mut line = String::new();
        let last = widths.len().saturating_sub(1);
        for (index, width) in widths.iter().enumerate() {
            let empty = Cell::new("");
            let cell = row.get(index).unwrap_or(&empty);
            let align = if header {
                self.columns[index].1
            } else {
                self.columns
                    .get(index)
                    .map_or(Align::Left, |column| column.1)
            };
            let padding = width.saturating_sub(display_width(&cell.text));
            let text = paint_with(stdout_color(), cell.style, &cell.text);
            match align {
                Align::Left if index == last => line.push_str(&text),
                Align::Left => {
                    line.push_str(&text);
                    line.push_str(&" ".repeat(padding));
                }
                Align::Right => {
                    line.push_str(&" ".repeat(padding));
                    line.push_str(&text);
                }
            }
            if index != last {
                line.push_str("  ");
            }
        }
        let _ = writeln!(out, "{}", line.trim_end());
    }
}

/// Aligned `label  value` lines for detail views.
pub(crate) struct Details {
    rows: Vec<(String, String)>,
}

impl Details {
    pub(crate) fn new() -> Self {
        Self { rows: Vec::new() }
    }

    pub(crate) fn row(&mut self, label: &str, value: impl Into<String>) -> &mut Self {
        self.rows
            .push((label.to_owned(), terminal_text(&value.into())));
        self
    }

    pub(crate) fn render(&self) -> String {
        let width = self
            .rows
            .iter()
            .map(|(label, _)| display_width(label))
            .max()
            .unwrap_or(0);
        let mut out = String::new();
        for (label, value) in &self.rows {
            let padding = " ".repeat(width - display_width(label));
            let _ = writeln!(out, "  {}{padding}  {value}", paint(Style::Dim, label));
        }
        out
    }
}

pub(crate) fn heading(title: &str, subtitle: Option<&str>) -> String {
    match subtitle {
        Some(subtitle) => format!(
            "{} {}\n",
            paint(Style::Bold, title),
            paint(Style::Dim, subtitle)
        ),
        None => format!("{}\n", paint(Style::Bold, title)),
    }
}

// ============ Numbers ============

/// Insert thousands separators into the integer part of a decimal string.
pub(crate) fn group_thousands(value: &str) -> String {
    let (sign, unsigned) = value
        .strip_prefix('-')
        .map_or(("", value), |rest| ("-", rest));
    let (integer, fraction) = unsigned
        .split_once('.')
        .map_or((unsigned, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    if !integer.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.to_owned();
    }
    let mut grouped = String::new();
    for (index, digit) in integer.chars().enumerate() {
        if index != 0 && (integer.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

/// Round a plain decimal string half-up to `places` digits without floats.
pub(crate) fn round_decimal(value: &str, places: usize) -> String {
    let (sign, unsigned) = value
        .strip_prefix('-')
        .map_or(("", value), |rest| ("-", rest));
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return value.to_owned();
    }
    let mut digits: Vec<u8> = integer
        .bytes()
        .chain(fraction.bytes().chain(std::iter::repeat(b'0')).take(places))
        .map(|byte| byte - b'0')
        .collect();
    if fraction
        .as_bytes()
        .get(places)
        .is_some_and(|next| *next >= b'5')
    {
        let mut index = digits.len();
        loop {
            if index == 0 {
                digits.insert(0, 1);
                break;
            }
            index -= 1;
            if digits[index] == 9 {
                digits[index] = 0;
            } else {
                digits[index] += 1;
                break;
            }
        }
    }
    let split = digits.len() - places;
    let integer: String = digits[..split]
        .iter()
        .map(|digit| char::from(b'0' + digit))
        .collect();
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    let fraction: String = digits[split..]
        .iter()
        .map(|digit| char::from(b'0' + digit))
        .collect();
    let is_zero = integer == "0" && fraction.bytes().all(|byte| byte == b'0');
    let sign = if is_zero { "" } else { sign };
    if places == 0 {
        format!("{sign}{integer}")
    } else {
        format!("{sign}{integer}.{fraction}")
    }
}

/// Fiat amount for display: rounded, grouped, with the currency code.
pub(crate) fn fiat(value: &str, currency: &str) -> String {
    let places = if currency == "JPY" { 0 } else { 2 };
    format!(
        "{} {currency}",
        group_thousands(&round_decimal(value, places))
    )
}

/// Crypto amount for display: grouped integer part, all decimals kept.
pub(crate) fn amount(value: &str, unit: &str) -> String {
    format!("{} {unit}", group_thousands(value))
}

// ============ Prompts ============

pub(crate) fn stdin_is_interactive() -> bool {
    std::io::stdin().is_terminal()
}

/// Ask for a line on stderr. Returns `None` at end of input.
pub(crate) fn prompt(question: &str) -> std::io::Result<Option<String>> {
    eprint!("{} {}", epaint(Style::Cyan, "?"), terminal_text(question));
    std::io::stderr().flush()?;
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
}

/// Ask a yes/no question; anything but y/yes is no.
pub(crate) fn confirm(question: &str) -> std::io::Result<bool> {
    Ok(prompt(&format!("{question} [y/N] "))?
        .is_some_and(|answer| matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")))
}

/// Read a secret from the terminal without echoing it.
pub(crate) fn prompt_secret(question: &str) -> std::io::Result<Option<String>> {
    eprint!("{} {}", epaint(Style::Cyan, "?"), terminal_text(question));
    std::io::stderr().flush()?;
    let _guard = echo::disable()?;
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line)?;
    eprintln!();
    if read == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
}

#[cfg(unix)]
mod echo {
    pub(super) struct Restore(libc::termios);

    pub(super) fn disable() -> std::io::Result<Restore> {
        // SAFETY: tcgetattr/tcsetattr only read and write the termios struct
        // for stdin, which stays valid for the duration of each call.
        unsafe {
            let mut original = std::mem::zeroed::<libc::termios>();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut original) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut silent = original;
            silent.c_lflag &= !libc::ECHO;
            silent.c_lflag |= libc::ECHONL;
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &silent) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Restore(original))
        }
    }

    impl Drop for Restore {
        fn drop(&mut self) {
            // SAFETY: restores the termios settings captured in `disable`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
            }
        }
    }
}

#[cfg(windows)]
mod echo {
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };

    pub(super) struct Restore(u32);

    pub(super) fn disable() -> std::io::Result<Restore> {
        // SAFETY: console mode calls on the process's own stdin handle.
        unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0;
            if GetConsoleMode(handle, &mut mode) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if SetConsoleMode(handle, mode & !ENABLE_ECHO_INPUT) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Restore(mode))
        }
    }

    impl Drop for Restore {
        fn drop(&mut self) {
            // SAFETY: restores the console mode captured in `disable`.
            unsafe {
                SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Align, Cell, Table, display_width, fiat, group_thousands, round_decimal};

    #[test]
    fn rounds_decimal_strings_half_up_without_floats() {
        assert_eq!(round_decimal("12345.675", 2), "12345.68");
        assert_eq!(round_decimal("0.994", 2), "0.99");
        assert_eq!(round_decimal("9.995", 2), "10.00");
        assert_eq!(round_decimal("999.5", 0), "1000");
        assert_eq!(round_decimal("-0.001", 2), "0.00");
        assert_eq!(round_decimal("-1.005", 2), "-1.01");
        assert_eq!(round_decimal("7", 2), "7.00");
        assert_eq!(round_decimal("abc", 2), "abc");
    }

    #[test]
    fn groups_thousands_in_the_integer_part_only() {
        assert_eq!(group_thousands("1234567.891234"), "1,234,567.891234");
        assert_eq!(group_thousands("-1000"), "-1,000");
        assert_eq!(group_thousands("999"), "999");
        assert_eq!(fiat("1234.5", "EUR"), "1,234.50 EUR");
        assert_eq!(fiat("1234.5", "JPY"), "1,235 JPY");
    }

    #[test]
    fn tables_align_columns_and_ignore_color_codes_in_width() {
        assert_eq!(display_width("\x1b[1mabc\x1b[0m"), 3);
        let mut table = Table::new(&[("NAME", Align::Left), ("VALUE", Align::Right)]);
        table.row(vec![Cell::new("Cold storage"), Cell::new("1.5")]);
        table.row(vec![Cell::new("Hot"), Cell::new("10,000.25")]);
        let rendered = table.render();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines[0], "NAME              VALUE");
        assert_eq!(lines[1], "Cold storage        1.5");
        assert_eq!(lines[2], "Hot           10,000.25");
    }
}

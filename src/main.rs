//! launch: your everyday helpers and every program on the PATH, in one
//! window.
//!
//! The top half is your own list from ~/.launch, laid out in columns
//! across the window. Below a line sits a search field, and under it the
//! programs whose names hold what you type. One input filters both
//! halves, and Enter runs the one under the cursor.
//!
//! Run from a key binding, with no terminal around it, launch opens its
//! own glass window; pressed again while open, it closes it.

use crust::{seq, style, Crust, Cursor, Input, Pane};
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const RUST_RGB: (u8, u8, u8) = (247, 76, 0);
const TEXT_RGB: (u8, u8, u8) = (225, 225, 230);
const DIM_RGB: (u8, u8, u8) = (140, 140, 150);
const DARK_RGB: (u8, u8, u8) = (70, 70, 80);
const BAR_BG: (u8, u8, u8) = (38, 38, 38);
const PICK_BG: (u8, u8, u8) = (52, 48, 60);

/// The widest a program name gets drawn.
const PROG_W: usize = 28;

/// One line of ~/.launch.
struct Helper {
    name: String,
    hot: String,
    cmd: String,
    group: usize,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Item {
    Helper(usize),
    Prog(usize),
}

/// Where an item you can move to sits on screen, from the last draw.
struct Spot {
    item: Item,
    x: usize,
    y: usize,
}

struct Launch {
    helpers: Vec<Helper>,
    progs: Vec<String>,
    query: String,
    /// Which helpers hold the query.
    lit: Vec<bool>,
    /// The programs that hold the query, best first.
    hits: Vec<usize>,
    spots: Vec<Spot>,
    sel: usize,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-v" | "--version") => {
            println!("launch {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("-h" | "--help") => {
            println!("launch — your helpers and every program, found as you type");
            println!();
            println!("  launch                    the launcher (opens its own window from a key binding)");
            println!("  launch --password PROMPT  ask for a password in a window, print it");
            println!();
            println!("  type        filter the helpers and the programs");
            println!("  arrows Tab  move");
            println!("  Enter       run the one under the cursor, or what you typed");
            println!("  Esc         close");
            println!();
            println!("  ~/.launch holds the helpers, one per line:");
            println!("    Volume Up [Ctrl+F3] = wpctl set-volume @DEFAULT_AUDIO_SINK@ 10%+");
            println!("  A blank line starts a new group.");
            return;
        }
        Some("--password") => {
            let prompt = args.get(1).map(String::as_str).unwrap_or("Password");
            std::process::exit(password(prompt));
        }
        Some("--ask") if args.len() >= 2 => {
            let prompt = args.get(2).map(String::as_str).unwrap_or("Password");
            ask(&args[1], prompt);
            return;
        }
        _ => {}
    }
    // SAFETY: isatty only looks at the descriptor.
    let on_tty = unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 };
    if !on_tty {
        toggle_window();
        return;
    }

    let home = home();
    let text = fs::read_to_string(home.join(".launch")).unwrap_or_default();
    let helpers = parse_helpers(&text);
    let lit = vec![true; helpers.len()];
    let mut l = Launch { helpers, progs: load_programs(&home), query: String::new(), lit, hits: Vec::new(), spots: Vec::new(), sel: 0 };

    let pid_file = runtime_dir().join("launch.pid");
    let _ = fs::write(&pid_file, std::process::id().to_string());
    Crust::init();
    Crust::set_app_identity("launch");
    Cursor::show();
    let cmd = l.run();
    Crust::cleanup();
    let _ = fs::remove_file(&pid_file);
    if let Some(cmd) = cmd {
        run_detached(&cmd, &home);
    }
}

impl Launch {
    /// Keys until a pick or a close. Nothing runs between keys.
    fn run(&mut self) -> Option<String> {
        loop {
            self.draw();
            let Some(key) = Input::getchr_ms(600_000) else { continue };
            match key.as_str() {
                "ESC" | "F12" => return None,
                "ENTER" => {
                    if let Some(spot) = self.spots.get(self.sel) {
                        return Some(match spot.item {
                            Item::Helper(i) => self.helpers[i].cmd.clone(),
                            Item::Prog(i) => self.progs[i].clone(),
                        });
                    }
                    let typed = self.query.trim();
                    if !typed.is_empty() {
                        return Some(typed.to_string());
                    }
                }
                "DOWN" | "TAB" => {
                    if self.sel + 1 < self.spots.len() {
                        self.sel += 1;
                    }
                }
                "UP" | "S-TAB" => self.sel = self.sel.saturating_sub(1),
                "LEFT" => self.sideways(false),
                "RIGHT" => self.sideways(true),
                "BACK" => {
                    self.query.pop();
                    self.refilter();
                }
                "WBACK" | "C-W" => {
                    let t = self.query.trim_end().len();
                    let cut = self.query[..t].rfind(' ').map(|p| p + 1).unwrap_or(0);
                    self.query.truncate(cut);
                    self.refilter();
                }
                "C-U" => {
                    self.query.clear();
                    self.refilter();
                }
                k if k.starts_with("PASTE\0") => {
                    self.query.push_str(k[6..].lines().next().unwrap_or(""));
                    self.refilter();
                }
                k if k.chars().count() == 1 => {
                    self.query.push_str(k);
                    self.refilter();
                }
                _ => {}
            }
        }
    }

    /// Left or right: the nearest item in the next column over.
    fn sideways(&mut self, right: bool) {
        let Some(cur) = self.spots.get(self.sel) else { return };
        let (cx, cy) = (cur.x, cur.y);
        let best = self
            .spots
            .iter()
            .enumerate()
            .filter(|(_, s)| if right { s.x > cx } else { s.x < cx })
            .min_by_key(|(_, s)| (s.x.abs_diff(cx), s.y.abs_diff(cy)));
        if let Some((i, _)) = best {
            self.sel = i;
        }
    }

    fn refilter(&mut self) {
        let q = self.query.trim().to_lowercase();
        self.lit = self.helpers.iter().map(|h| q.is_empty() || h.name.to_lowercase().contains(&q)).collect();
        self.hits = find(&self.progs, &q);
        self.sel = 0;
    }

    fn draw(&mut self) {
        let (cols, rows) = Crust::terminal_size();
        let (w, rows) = (cols as usize, rows as usize);
        let mut out = String::new();
        for y in 1..=rows {
            out.push_str(&format!("{}{}", Cursor::at(1, y as u16), seq::ERASE_EOL));
        }
        self.spots.clear();

        // The bar across the top.
        let tail = format!("   {} helpers · {} programs", self.helpers.len(), self.progs.len());
        let used = 1 + "launch".len() + crust::display_width(&tail);
        out.push_str(&format!(
            "{}{}{}{}",
            Cursor::at(1, 1),
            style::rgb(" ", None, Some(BAR_BG), ""),
            style::rgb("launch", Some(RUST_RGB), Some(BAR_BG), "b"),
            style::rgb(&format!("{tail}{}", " ".repeat(w.saturating_sub(used))), Some(DIM_RGB), Some(BAR_BG), "")
        ));

        // The helpers, a group never split across two columns.
        let bottom = rows.saturating_sub(1);
        let mut y = 3;
        if !self.helpers.is_empty() {
            let nw = self.helpers.iter().map(|h| crust::display_width(&h.name)).max().unwrap_or(0);
            let hw = self.helpers.iter().map(|h| crust::display_width(&h.hot)).max().unwrap_or(0);
            let cell = nw + if hw > 0 { 2 + hw } else { 0 };
            let colw = cell + 4;
            let groups = self.helpers.last().map(|h| h.group + 1).unwrap_or(0);
            let sizes: Vec<usize> = (0..groups).map(|g| self.helpers.iter().filter(|h| h.group == g).count()).collect();
            let columns = balance(&sizes, (w.saturating_sub(2) / colw).max(1));
            let mut height = 0;
            for (c, col) in columns.iter().enumerate() {
                let x = 3 + c * colw;
                let mut yy = y;
                for (k, &g) in col.iter().enumerate() {
                    if k > 0 {
                        yy += 1;
                    }
                    for (i, h) in self.helpers.iter().enumerate().filter(|(_, h)| h.group == g) {
                        if yy < bottom {
                            let picked = self.lit[i] && self.spots.len() == self.sel;
                            out.push_str(&format!("{}{}", Cursor::at(x as u16, yy as u16), helper_cell(h, nw, cell, self.lit[i], picked)));
                            if self.lit[i] {
                                self.spots.push(Spot { item: Item::Helper(i), x, y: yy });
                            }
                        }
                        yy += 1;
                    }
                }
                height = height.max(yy - y);
            }
            y += height + 1;
        }

        // The line, then the search field.
        out.push_str(&format!("{}{}", Cursor::at(3, y as u16), style::rgb(&"─".repeat(w.saturating_sub(4)), Some(DARK_RGB), None, "")));
        let field_y = y + 1;
        out.push_str(&format!(
            "{}{} {}",
            Cursor::at(3, field_y as u16),
            style::rgb("›", Some(RUST_RGB), None, "b"),
            style::rgb(&self.query, Some(TEXT_RGB), None, "")
        ));

        // The programs, down each column, then across.
        let top = field_y + 2;
        let height = bottom.saturating_sub(top);
        if self.query.trim().is_empty() {
            let say = format!("type to search {} programs", self.progs.len());
            out.push_str(&format!("{}{}", Cursor::at(5, top as u16), style::rgb(&say, Some(DIM_RGB), None, "i")));
        } else if self.hits.is_empty() {
            let say = "no program by that name; Enter runs what you typed";
            out.push_str(&format!("{}{}", Cursor::at(5, top as u16), style::rgb(say, Some(DIM_RGB), None, "i")));
        } else if height > 0 {
            let pw = self.hits.iter().map(|&i| crust::display_width(&self.progs[i])).max().unwrap_or(0).min(PROG_W);
            let colw = pw + 4;
            let ncols = (w.saturating_sub(2) / colw).max(1);
            for (k, &i) in self.hits.iter().take(height * ncols).enumerate() {
                let x = 3 + (k / height) * colw;
                let yy = top + k % height;
                let picked = self.spots.len() == self.sel;
                let name = fit(&self.progs[i], pw);
                let text = if picked { style::rgb(&name, Some(TEXT_RGB), Some(PICK_BG), "b") } else { style::rgb(&name, Some(TEXT_RGB), None, "") };
                out.push_str(&format!("{}{}", Cursor::at(x as u16, yy as u16), text));
                self.spots.push(Spot { item: Item::Prog(i), x, y: yy });
            }
        }

        // The bar along the bottom, the version at the far right.
        let foot = if self.query.trim().is_empty() {
            "type to search · arrows move · Enter run · Esc close".to_string()
        } else {
            let n = self.lit.iter().filter(|&&l| l).count();
            format!("{n} helpers and {} programs match", self.hits.len())
        };
        let version = format!("v{} ", env!("CARGO_PKG_VERSION"));
        let foot = format!(" {}", take_cells(&foot, w.saturating_sub(version.len() + 3)));
        let pad = w.saturating_sub(crust::display_width(&foot) + version.len());
        out.push_str(&format!(
            "{}{}{}",
            Cursor::at(1, rows as u16),
            style::rgb(&format!("{foot}{}", " ".repeat(pad)), Some((200, 200, 205)), Some(BAR_BG), ""),
            style::rgb(&version, Some(DIM_RGB), Some(BAR_BG), "")
        ));

        // The cursor rests at the end of what you typed.
        out.push_str(&Cursor::at((5 + crust::display_width(&self.query)) as u16, field_y as u16));
        print!("{out}");
        std::io::stdout().flush().ok();
    }
}

/// One helper: its name, then its key dim beside it.
fn helper_cell(h: &Helper, nw: usize, cell: usize, lit: bool, picked: bool) -> String {
    let body = if h.hot.is_empty() { fit(&h.name, cell) } else { format!("{}  {}", fit(&h.name, nw), fit(&h.hot, cell - nw - 2)) };
    if picked {
        return style::rgb(&body, Some(TEXT_RGB), Some(PICK_BG), "b");
    }
    if !lit {
        return style::rgb(&body, Some(DARK_RGB), None, "");
    }
    let name = style::rgb(&fit(&h.name, nw), Some(TEXT_RGB), None, "");
    if h.hot.is_empty() {
        return format!("{name}{}", " ".repeat(cell - nw));
    }
    format!("{name}  {}", style::rgb(&h.hot, Some(DIM_RGB), None, ""))
}

/// ~/.launch: `Name [Hotkey] = command`, the hotkey optional, a blank
/// line between groups, `#` for a comment.
fn parse_helpers(text: &str) -> Vec<Helper> {
    let mut out: Vec<Helper> = Vec::new();
    let mut group = 0;
    let mut gap = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if t.is_empty() {
            gap = !out.is_empty();
            continue;
        }
        let Some((left, cmd)) = t.split_once(" = ") else { continue };
        if gap {
            group += 1;
            gap = false;
        }
        let left = left.trim();
        let (name, hot) = match left.strip_suffix(']').and_then(|l| l.rsplit_once('[')) {
            Some((n, h)) => (n.trim(), h.trim()),
            None => (left, ""),
        };
        out.push(Helper { name: name.into(), hot: hot.into(), cmd: cmd.trim().into(), group });
    }
    out
}

/// Every program name, sorted. bare keeps them in ~/.bare_exe_cache: a
/// 16-byte header, then the names split by zero bytes. Without that
/// file, one look through the PATH.
fn load_programs(home: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read(home.join(".bare_exe_cache")) {
        Ok(b) if b.len() > 16 => b[16..].split(|&c| c == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect(),
        _ => Vec::new(),
    };
    if names.is_empty() {
        let path = std::env::var("PATH").unwrap_or_default();
        for dir in path.split(':') {
            let Ok(rd) = fs::read_dir(dir) else { continue };
            for e in rd.flatten() {
                use std::os::unix::fs::PermissionsExt;
                if e.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
                    names.push(e.file_name().to_string_lossy().into_owned());
                }
            }
        }
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// The programs that hold `q`: names that start with it first, then the
/// shorter ones, then by name.
fn find(progs: &[String], q: &str) -> Vec<usize> {
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(bool, usize, usize)> = progs
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let lp = p.to_lowercase();
            if lp.starts_with(q) {
                Some((false, p.len(), i))
            } else if lp.contains(q) {
                Some((true, p.len(), i))
            } else {
                None
            }
        })
        .collect();
    scored.sort_unstable();
    scored.into_iter().map(|(_, _, i)| i).collect()
}

/// Groups of these sizes into at most `ncols` columns, in order, as even
/// in height as whole groups allow. One blank line sits between groups.
fn balance(sizes: &[usize], ncols: usize) -> Vec<Vec<usize>> {
    let total = sizes.iter().sum::<usize>() + sizes.len().saturating_sub(1);
    let mut target = total.div_ceil(ncols.max(1)).max(sizes.iter().copied().max().unwrap_or(1));
    loop {
        let mut cols: Vec<Vec<usize>> = vec![Vec::new()];
        let mut h = 0;
        for (g, &s) in sizes.iter().enumerate() {
            let last = cols.last_mut().expect("never empty");
            let need = if last.is_empty() { s } else { h + 1 + s };
            if need > target && !last.is_empty() {
                cols.push(vec![g]);
                h = s;
            } else {
                last.push(g);
                h = need;
            }
        }
        if cols.len() <= ncols.max(1) {
            return cols;
        }
        target += 1;
    }
}

/// Start `cmd` in a session of its own, so it outlives this window.
fn run_detached(cmd: &str, home: &std::path::Path) {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).current_dir(home).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe, as pre_exec requires.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let _ = c.spawn();
}

/// From a key binding: close the open launch window if there is one,
/// or open a new one.
fn toggle_window() {
    let pid_file = runtime_dir().join("launch.pid");
    if let Some(pid) = fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<i32>().ok()) {
        if fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "launch") {
            // SAFETY: a plain signal to a process we just checked is launch.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            let _ = fs::remove_file(&pid_file);
            return;
        }
    }
    let me = std::env::current_exe().unwrap_or_else(|_| "launch".into());
    let err = Command::new("glass").args(["--class", "Launch", "--"]).arg(me).exec();
    eprintln!("launch: cannot start glass: {err}");
}

/// `--password`: ask in a window, print the answer. The answer travels
/// back through a pipe with a name, so it never lands in a file. Exit
/// status 1 when nothing was typed.
fn password(prompt: &str) -> i32 {
    let fifo = runtime_dir().join(format!("launch-{}.pipe", std::process::id()));
    let Ok(c) = CString::new(fifo.as_os_str().as_bytes()) else { return 1 };
    // SAFETY: a plain call with a valid C string.
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
        eprintln!("launch: cannot make {}", fifo.display());
        return 1;
    }
    // Open for reading first, without waiting, so the window can write
    // and go; the pipe holds the answer until it is read.
    let reader = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&fifo);
    let me = std::env::current_exe().unwrap_or_else(|_| "launch".into());
    let _ = Command::new("glass")
        .args(["--class", "Launch", "--"])
        .arg(me)
        .arg("--ask")
        .arg(&fifo)
        .arg(prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status();
    let mut answer = String::new();
    if let Ok(mut r) = reader {
        let _ = r.read_to_string(&mut answer);
    }
    let _ = fs::remove_file(&fifo);
    let answer = answer.trim_end_matches('\n');
    if answer.is_empty() {
        return 1;
    }
    println!("{answer}");
    0
}

/// `--ask PIPE PROMPT`: the window side of `--password`.
fn ask(fifo: &str, prompt: &str) {
    Crust::init();
    Crust::set_app_identity("launch");
    let (cols, rows) = Crust::terminal_size();
    let mut p = Pane::new(1, rows / 2, cols, 1, 255, 236);
    p.scroll = false;
    p.secret = true;
    let answer = p.ask_or_cancel(&format!(" {prompt}: "), "").unwrap_or_default();
    Crust::cleanup();
    if let Ok(mut w) = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(fifo) {
        let _ = writeln!(w, "{answer}");
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| "/tmp".into())
}

/// Exactly `w` cells: cut with a mark when too long, padded when short.
fn fit(s: &str, w: usize) -> String {
    let have = crust::display_width(s);
    if have <= w {
        return format!("{s}{}", " ".repeat(w - have));
    }
    let mut t = take_cells(s, w.saturating_sub(1));
    t.push('…');
    let pad = w.saturating_sub(crust::display_width(&t));
    format!("{t}{}", " ".repeat(pad))
}

/// The first `max` cells of `s`, never cutting a glyph in two.
fn take_cells(s: &str, max: usize) -> String {
    let mut walker = crust::WidthWalker::new();
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let add = walker.push(c);
        if w + add > max {
            break;
        }
        w += add;
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_read_with_keys_and_groups() {
        let h = parse_helpers("# c\nMute [Ctrl+F1] = wpctl a\nUp = b || c\n\n\nTile [Win+x] = pkill -USR2 tile\n");
        assert_eq!(h.len(), 3);
        assert_eq!((h[0].name.as_str(), h[0].hot.as_str(), h[0].cmd.as_str()), ("Mute", "Ctrl+F1", "wpctl a"));
        assert_eq!((h[1].hot.as_str(), h[1].cmd.as_str(), h[1].group), ("", "b || c", 0));
        assert_eq!(h[2].group, 1);
    }

    #[test]
    fn names_that_start_with_it_come_first() {
        let p: Vec<String> = ["xgimp", "gimp-2.10", "gimp", "other"].iter().map(|s| s.to_string()).collect();
        let hits: Vec<&str> = find(&p, "gimp").iter().map(|&i| p[i].as_str()).collect();
        assert_eq!(hits, ["gimp", "gimp-2.10", "xgimp"]);
    }

    #[test]
    fn groups_fill_columns_evenly_and_in_order() {
        let cols = balance(&[5, 2, 4, 3, 6, 2], 3);
        assert!(cols.len() <= 3);
        let flat: Vec<usize> = cols.concat();
        assert_eq!(flat, [0, 1, 2, 3, 4, 5]);
        assert_eq!(balance(&[3, 3], 1), vec![vec![0, 1]]);
    }
}

//! launch: your everyday helpers and every program on the PATH, in one
//! window.
//!
//! The top half is your own list from ~/.launch, laid out in columns
//! across the window. Below a line sits a search field, and under it the
//! programs whose names hold what you type. One input filters both
//! halves, and Enter runs the one under the cursor.
//!
//! A sum typed the rpnx way (`2 3 +`) shows its answer under the field,
//! and Enter puts the answer on the clipboard.
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

type Rgb = (u8, u8, u8);

/// The colours of the window. ~/.launchrc changes any of them with
/// `name = #rrggbb` lines, the names as below; a view writes it.
#[derive(Clone, Copy)]
struct Look {
    accent: Rgb,
    text: Rgb,
    dim: Rgb,
    dark: Rgb,
    bar_bg: Rgb,
    bar_title: Rgb,
    bar_text: Rgb,
    foot_bg: Rgb,
    foot_text: Rgb,
    foot_dim: Rgb,
    pick_fg: Rgb,
    pick_bg: Rgb,
    /// A bar down the left side, joined to the top bar (the LCARS view).
    side: Option<Rgb>,
    /// A strand of Matrix code down the left side, in this colour and
    /// darker towards the top (the matrix view).
    side_glyphs: Option<Rgb>,
}

const PLAIN: Look = Look {
    accent: (247, 76, 0),
    text: (225, 225, 230),
    dim: (140, 140, 150),
    dark: (70, 70, 80),
    bar_bg: (38, 38, 38),
    bar_title: (247, 76, 0),
    bar_text: (140, 140, 150),
    foot_bg: (38, 38, 38),
    foot_text: (200, 200, 205),
    foot_dim: (140, 140, 150),
    pick_fg: (225, 225, 230),
    pick_bg: (52, 48, 60),
    side: None,
    side_glyphs: None,
};

impl Look {
    fn load(home: &std::path::Path) -> Look {
        Look::parse(&fs::read_to_string(home.join(".launchrc")).unwrap_or_default())
    }

    fn parse(text: &str) -> Look {
        let mut k = PLAIN;
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            let Some((name, v)) = line.split_once('=') else { continue };
            let Some(c) = hex(v.trim()) else { continue };
            match name.trim() {
                "accent" => k.accent = c,
                "text" => k.text = c,
                "dim" => k.dim = c,
                "dark" => k.dark = c,
                "bar_bg" => k.bar_bg = c,
                "bar_title" => k.bar_title = c,
                "bar_text" => k.bar_text = c,
                "foot_bg" => k.foot_bg = c,
                "foot_text" => k.foot_text = c,
                "foot_dim" => k.foot_dim = c,
                "pick_fg" => k.pick_fg = c,
                "pick_bg" => k.pick_bg = c,
                "side" => k.side = Some(c),
                "side_glyphs" => k.side_glyphs = Some(c),
                _ => {}
            }
        }
        k
    }
}

/// The letters of the Matrix code: half-width katakana and a few digits.
const KANA: &str = "ﾊﾐﾋｰｳｼﾅﾓﾆｻﾜﾂｵﾘｱﾎﾃﾏｹﾒｴｶｷﾑﾕﾗｾﾈｽﾀﾇﾍ012345789Z";

/// A colour at a share of its brightness.
fn shade((r, g, b): Rgb, k: f32) -> Rgb {
    let f = |v: u8| (v as f32 * k.clamp(0.0, 1.0)) as u8;
    (f(r), f(g), f(b))
}

/// `#rrggbb` as a colour.
fn hex(v: &str) -> Option<Rgb> {
    let n = u32::from_str_radix(v.strip_prefix('#')?.get(..6)?, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// The widest a program name gets drawn.
const PROG_W: usize = 28;

/// A group of helpers: a name over it and a colour for it, from a
/// `## Name #rrggbb` line. A group a blank line starts has neither.
struct Group {
    name: String,
    colour: Option<Rgb>,
}

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
    groups: Vec<Group>,
    progs: Vec<String>,
    query: String,
    /// Which helpers hold the query.
    lit: Vec<bool>,
    /// The programs that hold the query, best first.
    hits: Vec<usize>,
    spots: Vec<Spot>,
    sel: usize,
    /// Ctrl-t: the next program goes the other way (into a glass, or not).
    swap: bool,
    /// What the query comes to, when it is a sum.
    answer: Option<String>,
    look: Look,
    /// frame's and tile's versions, for the top bar.
    desk: String,
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
            println!("  launch --run NAME         run a helper by its name, or a program, as Enter would");
            println!();
            println!("  type        filter the helpers and the programs");
            println!("  arrows Tab  move");
            println!("  Enter       run the one under the cursor, or what you typed");
            println!("  2 3 +       a sum, the rpnx way: the answer shows, Enter copies it");
            println!("  Esc         close");
            println!();
            println!("  ~/.launch holds the helpers, one per line:");
            println!("    Volume Up [Ctrl+F3] = wpctl set-volume @DEFAULT_AUDIO_SINK@ 10%+");
            println!("  A blank line starts a new group; `## Sound #5b8234` starts one with a name and a colour.");
            println!("  ~/.launchrc changes the colours (a view writes it).");
            return;
        }
        Some("--password") => {
            let prompt = args.get(1).map(String::as_str).unwrap_or("Password");
            std::process::exit(password(prompt));
        }
        Some("--run") if args.len() >= 2 => {
            std::process::exit(run_named(&args[1..].join(" ")));
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
    let (helpers, groups) = parse_helpers(&text);
    let lit = vec![true; helpers.len()];
    let look = Look::load(&home);
    let mut l = Launch { helpers, groups, progs: load_programs(&home), query: String::new(), lit, hits: Vec::new(), spots: Vec::new(), sel: 0, swap: false, answer: None, look, desk: desk_versions() };

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
                    // A helper runs as written. A program, or a typed
                    // command, goes into a glass when it needs a terminal.
                    let cmd = match self.spots.get(self.sel).map(|s| s.item) {
                        Some(Item::Helper(i)) => return Some(self.helpers[i].cmd.clone()),
                        Some(Item::Prog(i)) => self.progs[i].clone(),
                        None => match &self.answer {
                            Some(a) => return Some(copy_cmd(a)),
                            None => self.query.trim().to_string(),
                        },
                    };
                    if cmd.is_empty() {
                        continue;
                    }
                    return Some(in_terminal(&cmd, self.swap));
                }
                "C-T" => self.swap = !self.swap,
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
        self.answer = sum(&q).map(tidy);
        self.sel = 0;
    }

    fn draw(&mut self) {
        let (cols, rows) = Crust::terminal_size();
        let (w, rows) = (cols as usize, rows as usize);
        let look = self.look;
        let mut out = String::new();
        for y in 1..=rows {
            out.push_str(&format!("{}{}", Cursor::at(1, y as u16), seq::ERASE_EOL));
        }
        self.spots.clear();

        // A bar down the left side (LCARS), joined to the top bar. Cells
        // of colour, since a terminal may draw block letters with gaps
        // between the rows.
        if let Some(c) = look.side {
            for y in 2..rows {
                out.push_str(&format!("{}{}", Cursor::at(1, y as u16), style::rgb(" ", None, Some(c), "")));
            }
        }

        // Strands of code down the left side, each with its head at the
        // bottom in the accent and a tail fading upward, gaps between them.
        // The same letters each time, so a redraw stays still.
        if let Some(c) = look.side_glyphs {
            let kana: Vec<char> = KANA.chars().collect();
            let (mut len, mut gap, mut k, mut total) = (0, 0, 0, 1);
            for y in (2..rows).rev() {
                if len == 0 && gap == 0 {
                    // The next strand from the bottom: 6 to 17 rows, 1 to 4 apart.
                    len = 6 + (y * 7) % 12;
                    gap = 1 + (y * 5) % 4;
                    (k, total) = (0, len);
                }
                if len > 0 {
                    let ch = kana[(y * 11 + y / 3) % kana.len()].to_string();
                    let lit = if k == 0 { look.accent } else { shade(c, 1.0 - 0.8 * k as f32 / total as f32) };
                    out.push_str(&format!("{}{}", Cursor::at(1, y as u16), style::rgb(&ch, Some(lit), None, "")));
                    len -= 1;
                    k += 1;
                } else {
                    gap -= 1;
                }
            }
        }

        // The bar across the top.
        let tail = format!("   {} helpers · {} programs", self.helpers.len(), self.progs.len());
        let used = 1 + "launch".len() + crust::display_width(&tail);
        // frame's and tile's versions at the far right, when they fit.
        let desk = if self.desk.is_empty() || used + self.desk.len() + 4 > w { String::new() } else { format!("{} ", self.desk) };
        let pad = w.saturating_sub(used + crust::display_width(&desk));
        out.push_str(&format!(
            "{}{}{}{}{}",
            Cursor::at(1, 1),
            style::rgb(" ", None, Some(look.bar_bg), ""),
            style::rgb("launch", Some(look.bar_title), Some(look.bar_bg), "b"),
            style::rgb(&format!("{tail}{}", " ".repeat(pad)), Some(look.bar_text), Some(look.bar_bg), ""),
            style::rgb(&desk, Some(look.bar_text), Some(look.bar_bg), "")
        ));

        // The helpers, a group never split across two columns.
        let bottom = rows.saturating_sub(1);
        let mut y = 3;
        if !self.helpers.is_empty() {
            let nw = self.helpers.iter().map(|h| crust::display_width(&h.name)).max().unwrap_or(0);
            let hw = self.helpers.iter().map(|h| crust::display_width(&h.hot)).max().unwrap_or(0);
            let cell = nw + if hw > 0 { 2 + hw } else { 0 };
            let colw = cell + 4;
            // A named group takes a row more, for its name.
            let sizes: Vec<usize> = self.groups.iter().enumerate()
                .map(|(g, grp)| self.helpers.iter().filter(|h| h.group == g).count() + usize::from(!grp.name.is_empty()))
                .collect();
            let columns = balance(&sizes, (w.saturating_sub(2) / colw).max(1));
            let mut height = 0;
            for (c, col) in columns.iter().enumerate() {
                let x = 3 + c * colw;
                let mut yy = y;
                for (k, &g) in col.iter().enumerate() {
                    if k > 0 {
                        yy += 1;
                    }
                    let grp = &self.groups[g];
                    let colour = grp.colour.unwrap_or(look.dim);
                    if !grp.name.is_empty() {
                        if yy < bottom {
                            let any = self.helpers.iter().enumerate().any(|(i, h)| h.group == g && self.lit[i]);
                            let c = if any { grp.colour.unwrap_or(look.accent) } else { look.dark };
                            let name = fit(&grp.name, cell).trim_end().to_string();
                            let rule = "─".repeat(cell.saturating_sub(crust::display_width(&name) + 1));
                            out.push_str(&format!(
                                "{}{} {}",
                                Cursor::at(x as u16, yy as u16),
                                style::rgb(&name, Some(c), None, "b"),
                                style::rgb(&rule, Some(shade(c, 0.45)), None, "")
                            ));
                        }
                        yy += 1;
                    }
                    for (i, h) in self.helpers.iter().enumerate().filter(|(_, h)| h.group == g) {
                        if yy < bottom {
                            let picked = self.lit[i] && self.spots.len() == self.sel;
                            out.push_str(&format!("{}{}", Cursor::at(x as u16, yy as u16), helper_cell(&look, h, nw, cell, self.lit[i], picked, colour)));
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
        out.push_str(&format!("{}{}", Cursor::at(3, y as u16), style::rgb(&"─".repeat(w.saturating_sub(4)), Some(look.dark), None, "")));
        let field_y = y + 1;
        out.push_str(&format!(
            "{}{} {}",
            Cursor::at(3, field_y as u16),
            style::rgb("›", Some(look.accent), None, "b"),
            style::rgb(&self.query, Some(look.text), None, "")
        ));

        // The programs, down each column, then across.
        let top = field_y + 2;
        let height = bottom.saturating_sub(top);
        if self.query.trim().is_empty() {
            let say = format!("type to search {} programs", self.progs.len());
            out.push_str(&format!("{}{}", Cursor::at(5, top as u16), style::rgb(&say, Some(look.dim), None, "i")));
        } else if let (Some(a), true) = (&self.answer, self.hits.is_empty()) {
            out.push_str(&format!(
                "{}{} {}",
                Cursor::at(5, top as u16),
                style::rgb("=", Some(look.accent), None, "b"),
                style::rgb(a, Some(look.text), None, "b")
            ));
        } else if self.hits.is_empty() {
            let say = "no program by that name; Enter runs what you typed";
            out.push_str(&format!("{}{}", Cursor::at(5, top as u16), style::rgb(say, Some(look.dim), None, "i")));
        } else if height > 0 {
            let pw = self.hits.iter().map(|&i| crust::display_width(&self.progs[i])).max().unwrap_or(0).min(PROG_W);
            let colw = pw + 4;
            let ncols = (w.saturating_sub(2) / colw).max(1);
            for (k, &i) in self.hits.iter().take(height * ncols).enumerate() {
                let x = 3 + (k / height) * colw;
                let yy = top + k % height;
                let picked = self.spots.len() == self.sel;
                let name = fit(&self.progs[i], pw);
                let text = if picked { style::rgb(&name, Some(look.pick_fg), Some(look.pick_bg), "b") } else { style::rgb(&name, Some(look.text), None, "") };
                out.push_str(&format!("{}{}", Cursor::at(x as u16, yy as u16), text));
                self.spots.push(Spot { item: Item::Prog(i), x, y: yy });
            }
        }

        // The bar along the bottom, the version at the far right.
        let foot = if self.swap {
            "Ctrl-t: the next program goes the other way (into a glass, or out of one)".to_string()
        } else if self.query.trim().is_empty() {
            "type to search · arrows move · Enter run · Ctrl-t swap glass · Esc close".to_string()
        } else if self.answer.is_some() && self.spots.is_empty() {
            "a sum, worked out by rpnx · Enter copies the answer · Esc close".to_string()
        } else {
            let n = self.lit.iter().filter(|&&l| l).count();
            format!("{n} helpers and {} programs match", self.hits.len())
        };
        // The side bar ends in its own cell, a gap before the foot.
        let (lead, fw) = match look.side {
            Some(c) => (format!("{} ", style::rgb(" ", None, Some(c), "")), w.saturating_sub(2)),
            None => (String::new(), w),
        };
        let version = format!("v{} ", env!("CARGO_PKG_VERSION"));
        let foot = format!(" {}", take_cells(&foot, fw.saturating_sub(version.len() + 3)));
        let pad = fw.saturating_sub(crust::display_width(&foot) + version.len());
        out.push_str(&format!(
            "{}{lead}{}{}",
            Cursor::at(1, rows as u16),
            style::rgb(&format!("{foot}{}", " ".repeat(pad)), Some(look.foot_text), Some(look.foot_bg), ""),
            style::rgb(&version, Some(look.foot_dim), Some(look.foot_bg), "")
        ));

        // The cursor rests at the end of what you typed.
        out.push_str(&Cursor::at((5 + crust::display_width(&self.query)) as u16, field_y as u16));
        print!("{out}");
        std::io::stdout().flush().ok();
    }
}

/// What `line` comes to when it is a sum the rpnx way: numbers and
/// commands with spaces between, a number first (`2 3 +`, `45 sin`).
/// The rpnx engine does the work, so every command it knows is here.
fn sum(line: &str) -> Option<f64> {
    let first = line.split_whitespace().next()?;
    let digits = first.trim_start_matches(['-', '+']);
    if !digits.starts_with(|c: char| c.is_ascii_digit() || c == '.') || line.split_whitespace().count() < 2 {
        return None;
    }
    let mut s = rpnx_core::new_state();
    for word in line.split_whitespace() {
        if let Ok(n) = word.parse::<f64>() {
            rpnx_core::lift_stack(&mut s);
            s.x = n;
            continue;
        }
        let step = rpnx_core::execute(s, word.to_string());
        if step.error.is_some() {
            return None;
        }
        s = step.state;
    }
    s.x.is_finite().then_some(s.x)
}

/// A number as one would write it: ten decimals at most, no zeros at
/// the end, powers of ten for the very big and the very small.
fn tidy(x: f64) -> String {
    if x != 0.0 && !(1e-6..1e15).contains(&x.abs()) {
        return format!("{x:e}");
    }
    let s = format!("{x:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// The shell line that puts `text` on both selections. xclip stays
/// behind as their owner after this window is gone.
fn copy_cmd(text: &str) -> String {
    let t = sh_quote(text);
    format!("printf %s {t} | xclip -selection clipboard; printf %s {t} | xclip -selection primary")
}

/// One helper: its name, then its key beside it in the group's colour.
fn helper_cell(k: &Look, h: &Helper, nw: usize, cell: usize, lit: bool, picked: bool, hot: Rgb) -> String {
    let body = if h.hot.is_empty() { fit(&h.name, cell) } else { format!("{}  {}", fit(&h.name, nw), fit(&h.hot, cell - nw - 2)) };
    if picked {
        return style::rgb(&body, Some(k.pick_fg), Some(k.pick_bg), "b");
    }
    if !lit {
        return style::rgb(&body, Some(k.dark), None, "");
    }
    let name = style::rgb(&fit(&h.name, nw), Some(k.text), None, "");
    if h.hot.is_empty() {
        return format!("{name}{}", " ".repeat(cell - nw));
    }
    format!("{name}  {}", style::rgb(&h.hot, Some(hot), None, ""))
}

/// ~/.launch: `Name [Hotkey] = command`, the hotkey optional, a blank
/// line between groups, `## Name #rrggbb` to start a named one (the
/// colour optional), `#` for a comment.
fn parse_helpers(text: &str) -> (Vec<Helper>, Vec<Group>) {
    let mut out: Vec<Helper> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    // The next helper starts a group, named when a `##` line came first.
    let mut fresh = true;
    let mut named: Option<Group> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("## ") {
            let (name, colour) = match h.rsplit_once(' ').and_then(|(n, c)| Some((n, hex(c)?))) {
                Some((n, c)) => (n.trim(), Some(c)),
                None => (h.trim(), None),
            };
            named = Some(Group { name: name.into(), colour });
            fresh = true;
            continue;
        }
        if t.starts_with('#') {
            continue;
        }
        if t.is_empty() {
            fresh = true;
            continue;
        }
        let Some((left, cmd)) = t.split_once(" = ") else { continue };
        if fresh {
            groups.push(named.take().unwrap_or(Group { name: String::new(), colour: None }));
            fresh = false;
        }
        let group = groups.len() - 1;
        let left = left.trim();
        let (name, hot) = match left.strip_suffix(']').and_then(|l| l.rsplit_once('[')) {
            Some((n, h)) => (n.trim(), h.trim()),
            None => (left, ""),
        };
        out.push(Helper { name: name.into(), hot: hot.into(), cmd: cmd.trim().into(), group });
    }
    (out, groups)
}

/// "frame 0.1.55 · tile 0.1.87": each as its binary on the PATH says it,
/// the line `--version` prints. Read, never run: an old frame or tile, run
/// with --version, started a second desktop.
fn desk_versions() -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    ["frame", "tile"]
        .iter()
        .filter_map(|name| {
            let bin = path.split(':').map(|d| std::path::Path::new(d).join(name)).find(|p| p.is_file())?;
            version_in(&fs::read(bin).ok()?, name)
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The `name 1.2.3` line in a program's bytes: from each newline, back
/// over digits and dots, then the name. 0.3 ms for frame and tile.
fn version_in(bytes: &[u8], name: &str) -> Option<String> {
    let key = format!("{name} ");
    bytes.iter().enumerate().filter(|&(_, &c)| c == b'\n').find_map(|(end, _)| {
        let start = bytes[..end].iter().rposition(|&c| !(c.is_ascii_digit() || c == b'.'))? + 1;
        let v = std::str::from_utf8(&bytes[start..end]).ok()?;
        let whole = v.split('.').count() == 3 && v.split('.').all(|p| !p.is_empty());
        (whole && bytes[..start].ends_with(key.as_bytes())).then(|| format!("{name} {v}"))
    })
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

/// Does this program want a terminal? Asked only on Enter.
/// - glass itself never goes inside a glass.
/// - A menu entry (.desktop) says so with Terminal=true or false.
/// - A script is taken as a one-shot action, so it runs on its own.
/// - A binary linked to a window system library draws its own window.
/// - Anything else is a terminal program.
/// A program, or a typed command, goes into a glass when it needs a
/// terminal; `swap` (Ctrl+T) turns that choice around.
fn in_terminal(cmd: &str, swap: bool) -> String {
    let first = cmd.split_whitespace().next().unwrap_or("");
    if needs_terminal(first) == swap {
        return cmd.to_string();
    }
    // The window is named after the program: sh prints the title (handed
    // in as $0) before it starts it.
    let name = first.rsplit('/').next().unwrap_or(first);
    format!(
        "glass -- /bin/sh -c {} {}",
        sh_quote(&format!("printf %s \"$0\"; exec {cmd}")),
        sh_quote(&style::title_seq(name))
    )
}

/// `--run NAME`: what Enter would run for a helper of that name, or else
/// for a program of that name. Exit status 1 when there is neither.
fn run_named(name: &str) -> i32 {
    let home = home();
    let text = fs::read_to_string(home.join(".launch")).unwrap_or_default();
    let (helpers, _) = parse_helpers(&text);
    let cmd = match helpers.iter().find(|h| h.name.eq_ignore_ascii_case(name)) {
        Some(h) => h.cmd.clone(),
        None => match name.split_whitespace().next() {
            Some(first) if find_in_path(first).is_some() => in_terminal(name, false),
            _ => return 1,
        },
    };
    run_detached(&cmd, &home);
    0
}

fn needs_terminal(name: &str) -> bool {
    if name.is_empty() || name == "glass" {
        return false;
    }
    let Some(path) = find_in_path(name) else { return false };
    let real = fs::canonicalize(&path).unwrap_or(path.clone());
    let base = real.file_name().map(|b| b.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some(t) = desktop_terminal(&[name, &base]) {
        return t;
    }
    let Ok(bytes) = fs::read(&real) else { return false };
    if bytes.starts_with(b"#!") || !bytes.starts_with(b"\x7fELF") {
        return false;
    }
    // Library names all start with "lib": stop only there, then compare.
    const GUI: [&[u8]; 8] = [b"X11.so", b"xcb.so", b"gtk", b"gdk", b"wayland-client.so", b"Qt", b"SDL", b"EGL.so"];
    let mut rest = &bytes[..];
    while let Some(i) = rest.windows(3).position(|w| w == b"lib") {
        let after = &rest[i + 3..];
        if GUI.iter().any(|g| after.starts_with(g)) {
            return false;
        }
        rest = after;
    }
    true
}

/// Terminal=true or false from the first menu entry whose Exec runs one
/// of these names, or None when no entry does.
fn desktop_terminal(names: &[&str]) -> Option<bool> {
    let dirs = [home().join(".local/share/applications"), PathBuf::from("/usr/share/applications")];
    for dir in dirs {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if e.path().extension().is_none_or(|x| x != "desktop") {
                continue;
            }
            let Ok(text) = fs::read_to_string(e.path()) else { continue };
            let exec = text.lines().find_map(|l| l.strip_prefix("Exec=")).unwrap_or("");
            let prog = exec.split_whitespace().next().unwrap_or("");
            let prog = prog.rsplit('/').next().unwrap_or(prog);
            if names.contains(&prog) {
                return Some(text.lines().any(|l| l.trim() == "Terminal=true"));
            }
        }
    }
    None
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return Some(PathBuf::from(name));
    }
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':').map(|d| PathBuf::from(d).join(name)).find(|p| p.is_file())
}

/// One word for sh, whatever it holds.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Start `cmd` in a session of its own, so it outlives this window.
/// It waits until this window's glass is gone (a second at most): a
/// glass started while another one closes hangs before it opens.
fn run_detached(cmd: &str, home: &std::path::Path) {
    let glass = std::os::unix::process::parent_id();
    let is_glass = fs::read_to_string(format!("/proc/{glass}/comm")).is_ok_and(|c| c.trim() == "glass");
    let script = if is_glass {
        // Gone, or a zombie (Z) that tile has not reaped yet.
        format!(
            "i=0; while read -r _ _ s _ < /proc/{glass}/stat 2>/dev/null && [ \"$s\" != Z ] && [ $i -lt 50 ]; \
             do sleep 0.02; i=$((i+1)); done; {cmd}"
        )
    } else {
        cmd.to_string()
    };
    let mut c = Command::new("sh");
    c.arg("-c").arg(script).current_dir(home).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
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
    fn glass_runs_as_is_unless_swapped() {
        assert_eq!(in_terminal("glass", false), "glass");
        assert!(in_terminal("glass", true).starts_with("glass -- /bin/sh -c "));
    }

    #[test]
    fn launchrc_changes_only_the_colours_it_names() {
        let k = Look::parse("# a view\naccent = #ff9900\nside = #FF9900\ntext = bad\n");
        assert_eq!(k.accent, (255, 153, 0));
        assert_eq!(k.side, Some((255, 153, 0)));
        assert_eq!(k.text, PLAIN.text);
        assert!(PLAIN.side.is_none());
        assert_eq!(Look::parse("side_glyphs = #008f11\n").side_glyphs, Some((0, 143, 17)));
        assert_eq!(shade((200, 100, 0), 0.5), (100, 50, 0));
    }

    #[test]
    fn helpers_read_with_keys_and_groups() {
        let (h, g) = parse_helpers("# c\nMute [Ctrl+F1] = wpctl a\nUp = b || c\n\n\nTile [Win+x] = pkill -USR2 tile\n");
        assert_eq!((h.len(), g.len()), (3, 2));
        assert_eq!((h[0].name.as_str(), h[0].hot.as_str(), h[0].cmd.as_str()), ("Mute", "Ctrl+F1", "wpctl a"));
        assert_eq!((h[1].hot.as_str(), h[1].cmd.as_str(), h[1].group), ("", "b || c", 0));
        assert_eq!(h[2].group, 1);
    }

    #[test]
    fn a_double_hash_names_a_group_and_colours_it() {
        let (h, g) = parse_helpers("## Sound #5b8234\n\nMute = a\n## Wi-Fi and VPN\nRoam = b\nVPN = c\n\nLock = d\n");
        assert_eq!(h.iter().map(|h| h.group).collect::<Vec<_>>(), [0, 1, 1, 2]);
        assert_eq!((g[0].name.as_str(), g[0].colour), ("Sound", Some((0x5b, 0x82, 0x34))));
        assert_eq!((g[1].name.as_str(), g[1].colour), ("Wi-Fi and VPN", None));
        assert_eq!((g[2].name.as_str(), g[2].colour), ("", None));
    }

    #[test]
    fn a_sum_is_worked_out_and_a_name_is_left_alone() {
        let says = |line: &str| sum(line).map(tidy);
        assert_eq!(says("2 3 +").as_deref(), Some("5"));
        assert_eq!(says("2 3 4 * +").as_deref(), Some("14"));
        assert_eq!(says("10 3 /").as_deref(), Some("3.3333333333"));
        assert_eq!(says("0.1 0.2 +").as_deref(), Some("0.3"));
        assert_eq!(says("-2 3 *").as_deref(), Some("-6"));
        assert_eq!(says("2 sqrt 2 *").as_deref(), Some("2.8284271247"));
        assert_eq!(says("1e20 10 *").as_deref(), Some("1e21"));
        // Not a sum: a name, a lone number, a word the engine does not
        // know, and a sum with no answer.
        for line in ["gimp", "2048", "7z a", "2 3 frobnicate", "1 0 /", "sin 2"] {
            assert_eq!(says(line), None, "{line}");
        }
    }

    #[test]
    fn the_answer_goes_to_both_selections() {
        let cmd = copy_cmd("3.5");
        assert_eq!(cmd.matches("printf %s '3.5' | xclip -selection").count(), 2);
        assert!(cmd.contains("clipboard") && cmd.contains("primary"));
    }

    #[test]
    fn names_that_start_with_it_come_first() {
        let p: Vec<String> = ["xgimp", "gimp-2.10", "gimp", "other"].iter().map(|s| s.to_string()).collect();
        let hits: Vec<&str> = find(&p, "gimp").iter().map(|&i| p[i].as_str()).collect();
        assert_eq!(hits, ["gimp", "gimp-2.10", "xgimp"]);
    }

    #[test]
    fn a_quoted_word_survives_sh() {
        let out = Command::new("sh").arg("-c").arg(format!("printf %s {}", sh_quote("it's a \"test\" $HOME"))).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "it's a \"test\" $HOME");
    }

    #[test]
    fn glass_and_unknown_names_run_on_their_own() {
        assert!(!needs_terminal("glass"));
        assert!(!needs_terminal("no-such-program-here"));
    }

    #[test]
    fn groups_fill_columns_evenly_and_in_order() {
        let cols = balance(&[5, 2, 4, 3, 6, 2], 3);
        assert!(cols.len() <= 3);
        let flat: Vec<usize> = cols.concat();
        assert_eq!(flat, [0, 1, 2, 3, 4, 5]);
        assert_eq!(balance(&[3, 3], 1), vec![vec![0, 1]]);
    }

    #[test]
    fn a_version_line_is_found_among_other_strings() {
        let bin = b"\0usage: tile [--version]\ntile: map xid=\ntile 0.1.87\n\0";
        assert_eq!(version_in(bin, "tile").as_deref(), Some("tile 0.1.87"));
        assert_eq!(version_in(b"frame 2 screens\n", "frame"), None);
        assert_eq!(version_in(b"frame 2\n", "frame"), None);
    }
}

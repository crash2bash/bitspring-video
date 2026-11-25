//! What plays after what: the videos of a folder in the order of their names, or a playlist.
//!
//! Opening one video gathers the other videos of its folder around it, so that the next
//! episode of a series is one key away, and starts by itself when an episode ends. Opening
//! a folder, several files, or a playlist file makes a list that plays through. A list can
//! be added to, reordered, and saved as an .m3u8 file: plain text, one path to a line.
//!
//! Folders are listed and playlists read on a thread of their own: listing a folder can
//! wait on the system asking the user for access to it, and the window must not wait with it.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use serde::Serialize;

/// The endings a folder's videos are known by.
const VIDEO_ENDINGS: [&str; 20] = [
    "mkv", "mk3d", "mp4", "m4v", "mov", "webm", "avi", "ts", "m2ts", "mts", "mpg", "mpeg", "m2v",
    "vob", "wmv", "flv", "ogv", "ogm", "3gp", "mxf",
];
const PLAYLIST_ENDINGS: [&str; 2] = ["m3u8", "m3u"];
/// A list is cut off here, whatever it was made from.
const MOST_ITEMS: usize = 5000;
const LARGEST_PLAYLIST: u64 = 4 * 1024 * 1024;

fn ending(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_lowercase())
}

/// Whether a file is a playlist the player reads, going by its name.
pub fn is_playlist(path: &Path) -> bool {
    ending(path).is_some_and(|e| PLAYLIST_ENDINGS.contains(&e.as_str()))
}

fn is_video(path: &Path) -> bool {
    ending(path).is_some_and(|e| VIDEO_ENDINGS.contains(&e.as_str()))
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| name_of(path))
}

// ---- Names ------------------------------------------------------------------------------------

/// A name cut into runs of digits and runs of everything else: "Show - 02" is
/// ["show - ", 2]. Letters are lowered, so that order and likeness ignore case.
#[derive(Debug, PartialEq)]
enum Piece {
    Text(String),
    /// A number, without its leading zeros.
    Number(String),
}

fn pieces(name: &str) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for c in name.chars() {
        let digit = c.is_ascii_digit();
        match out.last_mut() {
            Some(Piece::Number(n)) if digit => n.push(c),
            Some(Piece::Text(t)) if !digit => t.extend(c.to_lowercase()),
            _ if digit => out.push(Piece::Number(c.to_string())),
            _ => out.push(Piece::Text(c.to_lowercase().collect())),
        }
    }
    for piece in &mut out {
        if let Piece::Number(n) = piece {
            let trimmed = n.trim_start_matches('0');
            *n = if trimmed.is_empty() { "0" } else { trimmed }.to_string();
        }
    }
    out
}

/// The order a person expects of numbered files: "Episode 2" before "Episode 10".
pub fn natural(a: &str, b: &str) -> Ordering {
    let (left, right) = (pieces(a), pieces(b));
    for (x, y) in left.iter().zip(&right) {
        let order = match (x, y) {
            (Piece::Number(x), Piece::Number(y)) => x.len().cmp(&y.len()).then_with(|| x.cmp(y)),
            (Piece::Text(x), Piece::Text(y)) => x.cmp(y),
            (Piece::Number(_), Piece::Text(_)) => Ordering::Less,
            (Piece::Text(_), Piece::Number(_)) => Ordering::Greater,
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len()).then_with(|| a.cmp(b))
}

/// Whether `next` reads as the continuation of `this`: the two names are the same up to a
/// number, and differ in it ("Show - 02" and "Show - 03", "S01E12 Finale" and "S02E01 Back").
/// Two films that merely share a folder do not pass.
pub fn continues(this: &str, next: &str) -> bool {
    let (left, right) = (pieces(this), pieces(next));
    let differ = left.iter().zip(&right).find(|(x, y)| x != y);
    matches!(differ, Some((Piece::Number(_), Piece::Number(_))))
}

/// What to call each file in a list. Files of one series share a long beginning that says
/// nothing in a list of them, so it is left out: "Show - 01 [1080p]" becomes "01 [1080p]".
/// Each name loses what it shares with a neighbour, so a film in the same list keeps its own.
pub fn labels(stems: &[String]) -> Vec<String> {
    /// Shorter than this, what the names share is as likely a word ("The ") as a series.
    const WORTH_CUTTING: usize = 6;
    let letters: Vec<Vec<char>> = stems.iter().map(|s| s.chars().collect()).collect();
    let shared = |a: &[char], b: &[char]| a.iter().zip(b).take_while(|(x, y)| x == y).count();
    (0..stems.len())
        .map(|at| {
            let name = &letters[at];
            let before = at.checked_sub(1).map_or(0, |i| shared(name, &letters[i]));
            let after = letters.get(at + 1).map_or(0, |next| shared(name, next));
            // Back to the last gap between words, so that a number is not cut in two.
            let cut = name[..before.max(after)]
                .iter()
                .rposition(|c| c.is_whitespace() || matches!(c, '.' | '_' | '-'))
                .map_or(0, |gap| gap + 1);
            let label: String = name[cut..].iter().collect();
            let label = label.trim();
            if cut < WORTH_CUTTING || label.is_empty() {
                stems[at].clone()
            } else {
                label.to_string()
            }
        })
        .collect()
}

// ---- Folders and playlist files ---------------------------------------------------------------

/// The videos in a folder, in the order of their names. `keep` is listed whatever its ending.
fn videos_in(folder: &Path, keep: Option<&Path>) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        // Not the "._name" files a Mac leaves beside real ones on disks of other systems.
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .filter(|e| is_video(&e.path()) || keep == Some(e.path().as_path()))
        // A link among the folder's files is listed only if it leads to a file on this
        // machine: nobody chose it by hand, and playing it would go wherever it leads.
        .filter(|e| {
            let link = e.file_type().is_ok_and(|t| t.is_symlink());
            !link || keep == Some(e.path().as_path()) || stays_here(&e.path())
        })
        .map(|e| e.path())
        .collect();
    found.sort_by(|a, b| natural(&stem_of(a), &stem_of(b)).then_with(|| a.cmp(b)));
    found.truncate(MOST_ITEMS);
    found
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|d| d as u8)
}

/// "%20" back to a space, for paths written as file: addresses.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let pair = (bytes.get(at + 1).copied(), bytes.get(at + 2).copied());
        if let (b'%', (Some(high), Some(low))) = (bytes[at], pair)
            && let (Some(high), Some(low)) = (hex(high), hex(low))
        {
            out.push(high * 16 + low);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A path that leads to another machine: a share ("\\server\films", "//server/films"), or one
/// of the names Windows reaches devices and shares by ("\\?\UNC\server\films"). Only asking
/// whether such a file exists already makes a connection, and signs in to whoever answers.
fn leads_elsewhere(path: &str) -> bool {
    let bytes = path.as_bytes();
    let separator = |at: usize| matches!(bytes.get(at), Some(b'/' | b'\\'));
    if !(separator(0) && separator(1)) {
        return false;
    }
    // "\\?\C:\films" is the long way of writing a path on a disk of this machine.
    let on_a_disk = bytes.get(2) == Some(&b'?')
        && separator(3)
        && bytes.get(4).is_some_and(u8::is_ascii_alphabetic)
        && bytes.get(5) == Some(&b':')
        && separator(6);
    !on_a_disk
}

/// The folders where a Mac mounts another machine's files the moment a name under them is
/// asked for, each as its names in small letters: the ones every Mac may have, and whatever
/// else this one's /etc/auto_master sets up.
fn mounted_on_demand() -> &'static [Vec<String>] {
    static FOLDERS: std::sync::OnceLock<Vec<Vec<String>>> = std::sync::OnceLock::new();
    FOLDERS.get_or_init(|| {
        let mut folders: Vec<Vec<String>> = ["net", "network", "home"]
            .iter()
            .map(|name| vec![name.to_string()])
            .collect();
        let configured = std::fs::read_to_string("/etc/auto_master").unwrap_or_default();
        for line in configured.lines() {
            // "/home  auto_home  -nobrowse": where, from what, how. "/-" is no folder.
            let place = line.split_whitespace().next().unwrap_or("");
            let names = names_of(Path::new(place));
            if place.starts_with('/') && !names.is_empty() && !folders.contains(&names) {
                folders.push(names);
            }
        }
        folders
    })
}

/// The names in a path, in small letters: a Mac's own disks compare them without their case.
fn names_of(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|part| match part {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect()
}

/// Whether a path is, by its names, in one of the folders that mount other machines.
fn mounts_on_demand(path: &Path) -> bool {
    let names = names_of(path);
    // The same folders are also reached through the name of the disk they are on.
    let names = match names.as_slice() {
        [system, volumes, data, rest @ ..]
            if system == "system" && volumes == "volumes" && data == "data" =>
        {
            rest
        }
        all => all,
    };
    mounted_on_demand()
        .iter()
        .any(|folder| names.starts_with(folder))
}

/// Whether a whole path leads to another machine, going by how it is written.
fn elsewhere(path: &Path) -> bool {
    // On Windows a path begins with what kind of place it is: only disks are this machine.
    #[cfg(windows)]
    if let Some(std::path::Component::Prefix(prefix)) = path.components().next()
        && !matches!(
            prefix.kind(),
            std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
        )
    {
        return true;
    }
    leads_elsewhere(&path.to_string_lossy())
        || (cfg!(target_os = "macos") && mounts_on_demand(path))
}

/// One step of a path, kept apart from the path it came from.
enum Step {
    /// A disk's name or the root: the path starts again from here.
    Start(std::ffi::OsString),
    Up,
    Name(std::ffi::OsString),
}

/// The steps of a path, last first, to be taken off the end one at a time.
fn steps_of(path: &Path) -> Vec<Step> {
    path.components()
        .rev()
        .filter_map(|part| match part {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                Some(Step::Start(part.as_os_str().to_os_string()))
            }
            std::path::Component::CurDir => None,
            std::path::Component::ParentDir => Some(Step::Up),
            std::path::Component::Normal(name) => Some(Step::Name(name.to_os_string())),
        })
        .collect()
}

/// Check that a path stays on this machine with every link in it followed. It is walked the
/// way the system walks it, one name at a time: a link is read and its own steps taken in
/// its place, and ".." steps back from where the links before it really led. Each place is
/// judged by its name before anything is asked about it, so no connection is made on the
/// way. `false` for a path that leads elsewhere, or that cannot be walked.
pub fn stays_here(path: &Path) -> bool {
    /// More links than this in one path is a loop.
    const MOST_LINKS: usize = 40;
    let mut ahead = steps_of(path);
    let mut here = PathBuf::new();
    let mut links = 0;
    // Past the last thing that exists there are only names: no links to follow.
    let mut exists = true;
    while let Some(step) = ahead.pop() {
        let name = match step {
            Step::Start(start) => {
                here.push(start);
                continue;
            }
            Step::Up => {
                here.pop();
                // Back among things that may exist: what follows is looked at again.
                exists = true;
                continue;
            }
            Step::Name(name) => name,
        };
        here.push(name);
        if elsewhere(&here) {
            return false;
        }
        if !exists {
            continue;
        }
        match std::fs::symlink_metadata(&here) {
            Ok(found) if found.file_type().is_symlink() => {
                links += 1;
                let Ok(target) = std::fs::read_link(&here) else {
                    return false;
                };
                // Windows may write where a link leads as "\\??\\C:\\films". Only a disk of this
                // machine may follow that beginning: a share or a device may not.
                let written = target.to_string_lossy().into_owned();
                let target = match written.strip_prefix("\\??\\") {
                    Some(rest) => {
                        let bytes = rest.as_bytes();
                        let on_a_disk = bytes.first().is_some_and(u8::is_ascii_alphabetic)
                            && bytes.get(1) == Some(&b':')
                            && matches!(bytes.get(2), Some(b'\\' | b'/'));
                        if !on_a_disk {
                            return false;
                        }
                        PathBuf::from(rest)
                    }
                    None => target,
                };
                if links > MOST_LINKS || leads_elsewhere(&target.to_string_lossy()) {
                    return false;
                }
                // The link's own steps come next, from the folder the link is in.
                here.pop();
                ahead.extend(steps_of(&target));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => exists = false,
            // Not allowed to look, or the disk failed: not a path to go down.
            Err(_) => return false,
        }
    }
    !elsewhere(&here)
}

/// A path without its "." and "..", worked out from the names alone: nothing is looked up.
fn tidy(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The files a playlist names, as written: lines that are not comments. Names of files on
/// this machine only: an address of anything else, or a path to another machine, is passed
/// over, never opened and never looked for.
pub fn parse_playlist(text: &str, folder: &Path) -> Vec<PathBuf> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let path = match line.strip_prefix("file://") {
                // "file:///films/a.mkv", "file://localhost/films/a.mkv", "file:///C:/films/a.mkv".
                Some(rest) => {
                    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
                    // Anything else before the path is the name of another machine.
                    if !rest.starts_with('/') {
                        return None;
                    }
                    let path = unescape(rest);
                    let drive = path.as_bytes().get(2) == Some(&b':');
                    if cfg!(windows) && drive {
                        path[1..].to_string()
                    } else {
                        path
                    }
                }
                None if line.contains("://") => return None,
                None => line.to_string(),
            };
            if leads_elsewhere(&path) {
                return None;
            }
            let path = PathBuf::from(path);
            let path = tidy(&if path.is_absolute() {
                path
            } else {
                folder.join(path)
            });
            (!elsewhere(&path)).then_some(path)
        })
        .take(MOST_ITEMS)
        .collect()
}

/// A playlist as text. Files in the playlist's own folder or below it are written by their
/// path from there, so that the folder can be moved with its playlist; others in full.
pub fn write_playlist(items: &[PathBuf], folder: &Path) -> String {
    let mut text = String::from("#EXTM3U\n");
    for item in items {
        let line = match item.strip_prefix(folder) {
            Ok(inside) if !inside.as_os_str().is_empty() => inside.to_string_lossy().into_owned(),
            _ => item.to_string_lossy().into_owned(),
        };
        // One path to a line: a name with a line break in it cannot be written.
        if !line.contains(['\n', '\r']) {
            text.push_str(&line);
            text.push('\n');
        }
    }
    text
}

fn read_playlist(file: &Path) -> Result<Vec<PathBuf>, String> {
    let name = name_of(file);
    let size = std::fs::metadata(file)
        .map_err(|e| format!("{name}: {e}"))?
        .len();
    if size > LARGEST_PLAYLIST {
        return Err(format!("{name}: too large to be a playlist"));
    }
    let bytes = std::fs::read(file).map_err(|e| format!("{name}: {e}"))?;
    // In full, so that what the playlist names is judged by where it really is.
    let file = std::path::absolute(file).map_err(|e| format!("{name}: {e}"))?;
    let folder = file.parent().unwrap_or(Path::new("/"));
    let items: Vec<PathBuf> = parse_playlist(&String::from_utf8_lossy(&bytes), folder)
        .into_iter()
        // First that the path stays on this machine, and only then whether the file is there.
        .filter(|p| stays_here(p) && p.is_file())
        .collect();
    if items.is_empty() {
        return Err(format!("{name}: none of the files it names are here"));
    }
    Ok(items)
}

/// Everything a list of opened things stands for: a folder gives its videos, a playlist
/// the files it names, a file itself.
fn expand(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut items = Vec::new();
    let mut problem = None;
    for path in paths {
        if path.is_dir() {
            items.extend(videos_in(path, None));
        } else if is_playlist(path) {
            match read_playlist(path) {
                Ok(named) => items.extend(named),
                Err(e) => problem = Some(e),
            }
        } else {
            items.push(path.clone());
        }
    }
    items.truncate(MOST_ITEMS);
    match problem {
        Some(e) if items.is_empty() => Err(e),
        _ if items.is_empty() => Err(match paths {
            [one] => format!("{}: no videos in the folder", name_of(one)),
            _ => "nothing to play".to_string(),
        }),
        _ => Ok(items),
    }
}

// ---- The queue --------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// The videos of the folder of the one file that was opened. The next of them starts by
    /// itself only when its name continues this one's: a series, not films that share a folder.
    Around,
    /// A folder opened whole: it plays through.
    Folder,
    /// A playlist file, or several files put together: it plays through.
    List,
}

/// What a listing thread comes back with.
enum Loaded {
    /// The folder of the file now playing.
    Around(Vec<PathBuf>),
    /// A new list, to be played from where it was left or from its start. `source` is the
    /// folder or the playlist file, when one thing alone was opened.
    Fresh {
        source: Option<PathBuf>,
        items: Result<Vec<PathBuf>, String>,
    },
    /// More for the end of the list there is.
    More(Result<Vec<PathBuf>, String>),
}

/// What [`Queue::poll`] found.
#[derive(Debug, PartialEq)]
pub enum Arrived {
    Nothing,
    /// A list has been read: play this file of it.
    Play(PathBuf),
    Failed(String),
}

#[derive(Default)]
pub struct Queue {
    kind: Option<Kind>,
    /// The folder, or the playlist's file; `None` for a list that has not been saved.
    source: Option<PathBuf>,
    items: Vec<PathBuf>,
    current: Option<usize>,
    /// A playlist that differs from its file.
    unsaved: bool,
    loading: Option<Receiver<Loaded>>,
    /// Counts changes, so that the description of the list is made again only after one.
    revision: u64,
    described: Option<(u64, QueueState)>,
}

/// The queue as the interface is told of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    /// "folder" or "playlist".
    pub kind: &'static str,
    /// The folder's name, or the playlist's; empty for a list that has not been saved.
    pub name: String,
    /// A playlist changed since it was read or saved.
    pub unsaved: bool,
    pub current: Option<usize>,
    /// Whether the file after this one starts by itself when this one ends.
    pub continues: bool,
    pub items: Vec<QueueItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct QueueItem {
    /// What to call it in the list.
    pub label: String,
    /// The file's whole name.
    pub name: String,
}

fn listing(work: impl FnOnce() -> Loaded + Send + 'static) -> Receiver<Loaded> {
    let (tx, rx) = channel();
    thread::Builder::new()
        .name("file list".into())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .expect("spawn the file listing");
    rx
}

impl Queue {
    fn changed(&mut self) {
        self.revision += 1;
    }

    /// Something was opened. One video is played at once (the answer says which) and its
    /// folder gathered around it; anything else is read first, and [`poll`](Self::poll)
    /// says what to play once it has been.
    pub fn open(&mut self, paths: Vec<PathBuf>) -> Option<PathBuf> {
        if let [one] = paths.as_slice()
            && !is_playlist(one)
            && !one.is_dir()
        {
            let file = one.clone();
            let folder = match file.parent() {
                Some(folder) if !folder.as_os_str().is_empty() => folder.to_path_buf(),
                _ => PathBuf::from("."),
            };
            self.kind = Some(Kind::Around);
            self.source = Some(folder.clone());
            self.items = vec![file.clone()];
            self.current = Some(0);
            self.unsaved = false;
            self.changed();
            let keep = file.clone();
            self.loading = Some(listing(move || {
                Loaded::Around(videos_in(&folder, Some(&keep)))
            }));
            return Some(file);
        }
        // What is playing stays as it is until the new list is known to hold something.
        let source = match paths.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        };
        self.loading = Some(listing(move || Loaded::Fresh {
            items: expand(&paths),
            source,
        }));
        None
    }

    /// Put more at the end of the list: files, folders, playlists.
    pub fn add(&mut self, paths: Vec<PathBuf>) {
        self.loading = Some(listing(move || Loaded::More(expand(&paths))));
    }

    /// Take what a listing thread has brought. `left_at` says which file a folder or a
    /// playlist was left on the last time, if it is remembered.
    pub fn poll(&mut self, left_at: impl Fn(&Path) -> Option<PathBuf>) -> Arrived {
        let Some(loading) = &self.loading else {
            return Arrived::Nothing;
        };
        let loaded = match loading.try_recv() {
            Ok(loaded) => loaded,
            Err(std::sync::mpsc::TryRecvError::Empty) => return Arrived::Nothing,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.loading = None;
                return Arrived::Nothing;
            }
        };
        self.loading = None;
        match loaded {
            Loaded::Around(items) => {
                // Still the same file, alone in its list: put its folder around it.
                let playing = self.current_path().map(Path::to_path_buf);
                if self.kind == Some(Kind::Around)
                    && let Some(at) = playing.and_then(|p| items.iter().position(|i| *i == p))
                {
                    self.items = items;
                    self.current = Some(at);
                    self.changed();
                }
                Arrived::Nothing
            }
            Loaded::Fresh { source, items } => match items {
                Ok(items) => {
                    let at = source
                        .as_deref()
                        .and_then(left_at)
                        .and_then(|left| items.iter().position(|i| *i == left))
                        .unwrap_or(0);
                    self.kind = Some(match &source {
                        Some(folder) if !is_playlist(folder) => Kind::Folder,
                        _ => Kind::List,
                    });
                    self.source = source;
                    self.items = items;
                    self.current = Some(at);
                    self.unsaved = false;
                    self.changed();
                    Arrived::Play(self.items[at].clone())
                }
                Err(e) => Arrived::Failed(e),
            },
            Loaded::More(items) => match items {
                Ok(mut items) => {
                    items.truncate(MOST_ITEMS.saturating_sub(self.items.len()));
                    self.items.append(&mut items);
                    self.edited();
                    // Nothing was playing: begin with the first of what came.
                    if self.current.is_none() && !self.items.is_empty() {
                        self.current = Some(0);
                        return Arrived::Play(self.items[0].clone());
                    }
                    Arrived::Nothing
                }
                Err(e) => Arrived::Failed(e),
            },
        }
    }

    /// The list was changed by hand: from here on it is a playlist, whatever it was.
    fn edited(&mut self) {
        if self.kind != Some(Kind::List) {
            // No longer what is in the folder.
            self.source = None;
        }
        self.kind = Some(Kind::List);
        self.unsaved = self.source.is_some();
        self.changed();
    }

    pub fn current_path(&self) -> Option<&Path> {
        self.items.get(self.current?).map(PathBuf::as_path)
    }

    /// What a folder opened whole or a playlist is remembered by, to come back to the file
    /// it was left on.
    pub fn remembered_as(&self) -> Option<&Path> {
        match self.kind {
            Some(Kind::Folder | Kind::List) => self.source.as_deref(),
            _ => None,
        }
    }

    pub fn next(&self) -> Option<usize> {
        self.current
            .map(|at| at + 1)
            .filter(|&next| next < self.items.len())
    }

    pub fn previous(&self) -> Option<usize> {
        self.current.and_then(|at| at.checked_sub(1))
    }

    /// The file that starts by itself when this one ends, if any.
    pub fn after_end(&self) -> Option<usize> {
        let (at, next) = (self.current?, self.next()?);
        let goes_on = match self.kind? {
            Kind::Around => continues(&stem_of(&self.items[at]), &stem_of(&self.items[next])),
            Kind::Folder | Kind::List => true,
        };
        goes_on.then_some(next)
    }

    /// Make this file of the list the current one, and say which it is.
    pub fn go(&mut self, index: usize) -> Option<PathBuf> {
        let path = self.items.get(index)?.clone();
        self.current = Some(index);
        self.changed();
        Some(path)
    }

    /// Take a file out of the list. The one playing stays.
    pub fn remove(&mut self, index: usize) {
        if index >= self.items.len() || self.current == Some(index) {
            return;
        }
        self.items.remove(index);
        if let Some(at) = &mut self.current
            && index < *at
        {
            *at -= 1;
        }
        self.edited();
    }

    /// Move a file to another place in the list: `to` is where it stands afterwards.
    pub fn move_item(&mut self, from: usize, to: usize) {
        let to = to.min(self.items.len().saturating_sub(1));
        if from >= self.items.len() || from == to {
            return;
        }
        let playing = self.current_path().map(Path::to_path_buf);
        let item = self.items.remove(from);
        self.items.insert(to, item);
        // The same file is still the one playing, wherever it stands now.
        if let Some(at) = self.current {
            self.current = Some(if at == from {
                to
            } else if from < at && to >= at {
                at - 1
            } else if from > at && to <= at {
                at + 1
            } else {
                at
            });
            debug_assert_eq!(self.current_path().map(Path::to_path_buf), playing);
        }
        self.edited();
    }

    /// Write the list to a playlist file, which it is from then on.
    pub fn save(&mut self, file: &Path) -> Result<PathBuf, String> {
        let file = if is_playlist(file) {
            file.to_path_buf()
        } else {
            let mut name = file.as_os_str().to_os_string();
            name.push(".m3u8");
            PathBuf::from(name)
        };
        let folder = file.parent().unwrap_or(Path::new("."));
        std::fs::write(&file, write_playlist(&self.items, folder))
            .map_err(|e| format!("{}: {e}", name_of(&file)))?;
        self.kind = Some(Kind::List);
        self.source = Some(file.clone());
        self.unsaved = false;
        self.changed();
        Ok(file)
    }

    /// The queue for the interface. Made again only when the queue has changed.
    pub fn state(&mut self) -> &QueueState {
        if self
            .described
            .as_ref()
            .is_none_or(|(at, _)| *at != self.revision)
        {
            let stems: Vec<String> = self.items.iter().map(|p| stem_of(p)).collect();
            let items = labels(&stems)
                .into_iter()
                .zip(&self.items)
                .map(|(label, path)| QueueItem {
                    label,
                    name: name_of(path),
                })
                .collect();
            let state = QueueState {
                kind: match self.kind {
                    Some(Kind::List) => "playlist",
                    _ => "folder",
                },
                name: match (self.kind, &self.source) {
                    (Some(Kind::List), Some(file)) => stem_of(file),
                    (_, Some(folder)) => name_of(folder),
                    _ => String::new(),
                },
                unsaved: self.unsaved,
                current: self.current,
                continues: self.after_end().is_some(),
                items,
            };
            self.described = Some((self.revision, state));
        }
        &self.described.as_ref().expect("just made").1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("bsv-queue-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        // The names a folder gives back are the real ones (/private/var for /var on a Mac).
        folder.canonicalize().unwrap()
    }

    fn touch(path: PathBuf) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        path
    }

    /// Wait for the listing thread.
    fn arrive(queue: &mut Queue, left_at: Option<&Path>) -> Arrived {
        let began = Instant::now();
        while queue.loading.is_some() && began.elapsed() < Duration::from_secs(5) {
            match queue.poll(|_| left_at.map(Path::to_path_buf)) {
                Arrived::Nothing => thread::sleep(Duration::from_millis(2)),
                other => return other,
            }
        }
        Arrived::Nothing
    }

    fn names(queue: &Queue) -> Vec<String> {
        queue.items.iter().map(|p| name_of(p)).collect()
    }

    #[test]
    fn numbers_in_names_are_put_in_order_as_numbers() {
        let mut names = vec!["Ep 10", "ep 2", "Ep 1", "Ep 02 extra", "Abc", "Ep 9.5"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(
            names,
            ["Abc", "Ep 1", "ep 2", "Ep 02 extra", "Ep 9.5", "Ep 10"]
        );
    }

    #[test]
    fn an_episode_continues_into_the_next_and_a_film_does_not() {
        assert!(continues("Show - 02 [1080p]", "Show - 03 [1080p]"));
        assert!(continues("Show S01E12 Finale", "Show S02E01 Back"));
        assert!(continues("show.s01e09.web", "Show.S01E10.WEB"));
        assert!(continues("01", "02"));
        assert!(!continues("Alien 1979", "Aliens 1986"));
        assert!(!continues("Dune Part Two", "Oppenheimer"));
        assert!(!continues("Film 1 Extended", "Film 1 Theatrical"));
        assert!(!continues("Film", "Film"));
    }

    #[test]
    fn what_a_series_shares_is_left_out_of_its_labels() {
        let stems = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            labels(&stems(&[
                "[G] Show - 01 [1080p]",
                "[G] Show - 02 [1080p]",
                "[G] Show - 10 [1080p]"
            ])),
            ["01 [1080p]", "02 [1080p]", "10 [1080p]"]
        );
        assert_eq!(
            labels(&stems(&["Show.S01E01.Pilot", "Show.S01E02.Next"])),
            ["Show.S01E01.Pilot", "Show.S01E02.Next"]
        );
        assert_eq!(
            labels(&stems(&["Long Show S01E01 Pilot", "Long Show S01E02 Next"])),
            ["S01E01 Pilot", "S01E02 Next"]
        );
        // Films keep their names, and so does a file on its own.
        assert_eq!(
            labels(&stems(&["The Thing", "The Fly"])),
            ["The Thing", "The Fly"]
        );
        assert_eq!(labels(&stems(&["Show - 01"])), ["Show - 01"]);
        assert_eq!(labels(&stems(&["Show - 01", "Show - "])), ["01", "Show - "]);
        // A film beside a series does not take the series' names back.
        assert_eq!(
            labels(&stems(&["Show - 09", "Show - 10", "Zebra"])),
            ["09", "10", "Zebra"]
        );
    }

    #[test]
    fn one_file_gathers_its_folder_and_goes_on_only_as_a_series() {
        let folder = scratch("around");
        touch(folder.join("Show - 01.mkv"));
        let second = touch(folder.join("Show - 02.mkv"));
        touch(folder.join("Show - 10.mkv"));
        touch(folder.join("Show - 02.ass"));
        touch(folder.join("._Show - 03.mkv"));
        touch(folder.join("Zebra.mp4"));

        let mut queue = Queue::default();
        assert_eq!(queue.open(vec![second.clone()]), Some(second));
        assert_eq!(queue.state().items.len(), 1);
        assert_eq!(arrive(&mut queue, None), Arrived::Nothing);
        assert_eq!(
            names(&queue),
            [
                "Show - 01.mkv",
                "Show - 02.mkv",
                "Show - 10.mkv",
                "Zebra.mp4"
            ]
        );
        assert_eq!(queue.current, Some(1));
        assert_eq!(queue.after_end(), Some(2));
        let state = queue.state().clone();
        assert_eq!((state.kind, state.continues), ("folder", true));
        assert_eq!(state.items[2].label, "10");
        assert_eq!(queue.remembered_as(), None);

        // The last episode is followed by a film: reachable, but it does not start by itself.
        queue.go(2);
        assert_eq!((queue.next(), queue.after_end()), (Some(3), None));
        assert!(!queue.state().continues);
    }

    #[test]
    fn a_folder_opened_whole_plays_through_from_where_it_was_left() {
        let folder = scratch("whole");
        touch(folder.join("Dune.mkv"));
        let second = touch(folder.join("Heat.mkv"));
        let mut queue = Queue::default();
        assert_eq!(queue.open(vec![folder.clone()]), None);
        assert_eq!(arrive(&mut queue, Some(&second)), Arrived::Play(second));
        assert_eq!(queue.current, Some(1));
        assert_eq!(queue.remembered_as(), Some(folder.as_path()));
        queue.go(0);
        assert_eq!(queue.after_end(), Some(1));

        let empty = scratch("empty");
        assert_eq!(queue.open(vec![empty]), None);
        assert!(matches!(arrive(&mut queue, None), Arrived::Failed(_)));
        // What was there stays.
        assert_eq!(names(&queue), ["Dune.mkv", "Heat.mkv"]);
    }

    #[test]
    fn a_playlist_is_made_changed_saved_and_read_back() {
        let folder = scratch("list");
        let a = touch(folder.join("a.mkv"));
        let b = touch(folder.join("sub/b.mkv"));
        let elsewhere = scratch("list-elsewhere");
        let c = touch(elsewhere.join("c.mkv"));

        let mut queue = Queue::default();
        assert_eq!(queue.open(vec![a.clone(), b.clone()]), None);
        assert_eq!(arrive(&mut queue, None), Arrived::Play(a.clone()));
        let state = queue.state().clone();
        assert_eq!(
            (state.kind, state.name.as_str(), state.unsaved),
            ("playlist", "", false)
        );

        queue.add(vec![c.clone()]);
        assert_eq!(arrive(&mut queue, None), Arrived::Nothing);
        queue.move_item(2, 0);
        assert_eq!(names(&queue), ["c.mkv", "a.mkv", "b.mkv"]);
        assert_eq!(queue.current_path(), Some(a.as_path()));
        queue.move_item(1, 2);
        assert_eq!(queue.current_path(), Some(a.as_path()));
        queue.remove(2); // the one playing: stays
        queue.remove(0);
        assert_eq!(names(&queue), ["b.mkv", "a.mkv"]);
        assert_eq!(queue.current, Some(1));
        queue.add(vec![c.clone()]);
        arrive(&mut queue, None);

        let file = queue.save(&folder.join("evening")).unwrap();
        assert_eq!(name_of(&file), "evening.m3u8");
        let text = std::fs::read_to_string(&file).unwrap();
        let separator = std::path::MAIN_SEPARATOR;
        assert_eq!(
            text,
            format!("#EXTM3U\nsub{separator}b.mkv\na.mkv\n{}\n", c.display())
        );
        assert_eq!(queue.state().name, "evening");

        let mut again = Queue::default();
        assert_eq!(again.open(vec![file.clone()]), None);
        assert_eq!(arrive(&mut again, Some(&a)), Arrived::Play(a));
        assert_eq!(names(&again), ["b.mkv", "a.mkv", "c.mkv"]);
        assert_eq!(again.remembered_as(), Some(file.as_path()));
        again.remove(0);
        assert!(again.state().unsaved);
    }

    #[test]
    fn a_playlist_names_files_on_this_machine_and_nothing_else() {
        let folder = Path::new(if cfg!(windows) { "C:\\lists" } else { "/lists" });
        let text = "\u{feff}#EXTM3U\n#EXTINF:1,One\none.mkv\n\n  sub/two.mkv  \nhttps://example.com/three.mkv\n\
                    rtsp://camera/stream\nfile:///films/four%20and%20a%20half.mkv\n";
        let found = parse_playlist(text, folder);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0], folder.join("one.mkv"));
        assert_eq!(found[1], folder.join("sub/two.mkv"));
        assert!(found[2].to_string_lossy().ends_with("four and a half.mkv"));
    }

    #[test]
    fn a_playlist_cannot_send_the_player_to_another_machine() {
        let folder = Path::new(if cfg!(windows) { "C:\\lists" } else { "/lists" });
        let text = "\\\\server\\share\\a.mkv\n//server/share/b.mkv\n\\\\?\\UNC\\server\\share\\c.mkv\n\
                    \\\\.\\pipe\\d\nfile://server/share/e.mkv\nfile:////server/share/f.mkv\n\
                    file://localhost//server/share/g.mkv\nfile:///%5C%5Cserver/share/h.mkv\n/\\server/i.mkv\n\
                    here.mkv\n";
        assert_eq!(parse_playlist(text, folder), [folder.join("here.mkv")]);
        // The long way of writing a path on a disk of this machine is not a way out of it.
        assert!(!leads_elsewhere("\\\\?\\C:\\films\\a.mkv"));
        assert!(leads_elsewhere(
            "\\\\?\\GLOBALROOT\\Device\\Mup\\server\\a.mkv"
        ));
        assert!(!leads_elsewhere("\\films\\a.mkv"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_playlist_cannot_reach_folders_that_mount_other_machines() {
        let text = "/net/server/a.mkv\n../net/server/b.mkv\nsub/../../Network/Servers/s/c.mkv\n\
                    /System/Volumes/Data/net/server/d.mkv\n/NET/server/e.mkv\n/network/f.mkv\n\
                    /system/volumes/DATA/Net/g.mkv\nsub/./h.mkv\n../i.mkv\n/networks/j.mkv\n\
                    /home/someone/k.mkv\n/System/Volumes/Data/Home/someone/l.mkv\n";
        assert_eq!(
            parse_playlist(text, Path::new("/lists")),
            [
                PathBuf::from("/lists/sub/h.mkv"),
                PathBuf::from("/i.mkv"),
                PathBuf::from("/networks/j.mkv"),
            ]
        );
        // Nor from a playlist that is itself there, nor from one opened by its bare name.
        assert!(parse_playlist("a.mkv\n", Path::new("/net/server/films")).is_empty());
        assert!(parse_playlist("/net/server/a.mkv\nnet/b.mkv\n", Path::new("")).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_link_into_such_a_folder_is_not_gone_through() {
        let folder = scratch("links");
        let film = touch(folder.join("real/film.mkv"));
        std::os::unix::fs::symlink("/net/server/films", folder.join("away")).unwrap();
        std::os::unix::fs::symlink("away", folder.join("twice")).unwrap();
        std::os::unix::fs::symlink("real", folder.join("near")).unwrap();
        std::os::unix::fs::symlink("round", folder.join("round")).unwrap();
        assert!(stays_here(&film));
        assert!(stays_here(&folder.join("near/film.mkv")));
        assert!(stays_here(&folder.join("nothing/there.mkv")));
        assert!(!stays_here(&folder.join("away/film.mkv")));
        assert!(!stays_here(&folder.join("twice/film.mkv")));
        assert!(!stays_here(&folder.join("near/../away/film.mkv")));
        assert!(!stays_here(&folder.join("round/film.mkv")));

        // ".." after a link steps back from where the link led, not from where it stands:
        // "deep/.." is /net/server, though by the names alone it would be this folder.
        std::os::unix::fs::symlink("/net/server/films", folder.join("deep")).unwrap();
        std::os::unix::fs::symlink("deep/../other", folder.join("sideways")).unwrap();
        std::os::unix::fs::symlink("near/../real", folder.join("back")).unwrap();
        assert!(!stays_here(&folder.join("deep/../film.mkv")));
        assert!(!stays_here(&folder.join("sideways/film.mkv")));
        assert!(stays_here(&folder.join("back/film.mkv")));
        assert!(stays_here(&folder.join("near/../real/film.mkv")));
        // Stepping back from a name that is not there does not end the looking.
        assert!(!stays_here(&folder.join("nothing/../away/film.mkv")));

        // Read from its file, a playlist gives only what stays here and is there.
        let list = folder.join("list.m3u8");
        std::fs::write(&list, "away/film.mkv\nnear/film.mkv\ntwice/film.mkv\n").unwrap();
        assert_eq!(read_playlist(&list), Ok(vec![folder.join("near/film.mkv")]));

        // The same holds for a folder's own files: a link to a film here is one of them,
        // a link to a film elsewhere is not.
        std::os::unix::fs::symlink("film.mkv", folder.join("real/again.mkv")).unwrap();
        std::os::unix::fs::symlink("/net/server/far.mkv", folder.join("real/far.mkv")).unwrap();
        assert_eq!(
            videos_in(&folder.join("real"), None),
            [folder.join("real/again.mkv"), film.clone()]
        );
    }

    #[test]
    fn adding_to_a_folder_makes_it_a_playlist() {
        let folder = scratch("grow");
        let one = touch(folder.join("Show - 01.mkv"));
        touch(folder.join("Show - 02.mkv"));
        let other = touch(scratch("grow-other").join("Film.mkv"));
        let mut queue = Queue::default();
        queue.open(vec![one]);
        arrive(&mut queue, None);
        queue.add(vec![other]);
        arrive(&mut queue, None);
        assert_eq!(
            names(&queue),
            ["Show - 01.mkv", "Show - 02.mkv", "Film.mkv"]
        );
        let state = queue.state().clone();
        assert_eq!(
            (state.kind, state.name.as_str(), state.unsaved),
            ("playlist", "", false)
        );
        // A playlist plays through, series or not.
        queue.go(1);
        assert_eq!(queue.after_end(), Some(2));
    }
}

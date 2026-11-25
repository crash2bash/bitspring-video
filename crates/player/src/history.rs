//! What is remembered about each file: where it was stopped, and the tracks chosen for it.
//!
//! Kept in a small JSON file beside the settings. A file is known by its size and the
//! beginning of its contents, not by its name or folder, so it is still known after being
//! renamed or moved.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Files remembered at most; the ones left longest ago go first.
const MOST_FILES: usize = 500;
/// Folders and playlists remembered at most.
const MOST_LISTS: usize = 200;
/// A file stopped this close to its beginning starts from the beginning.
const TOO_EARLY: f64 = 5.0;
/// On coming back, playback starts this much before where it stopped.
const RUN_UP: f64 = 3.0;
/// While playing, the place is written down this often, so that little is lost if the player
/// is killed.
const WRITE_EVERY: Duration = Duration::from_secs(5);

/// What is kept for one file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Remembered {
    /// The file's name when it was last played, for whoever reads the JSON.
    pub name: String,
    /// Where it was stopped, in seconds; 0 once it has been watched to the end.
    pub position: f64,
    pub duration: f64,
    /// The sound track chosen by hand, by its stream number.
    pub audio_track: Option<i32>,
    /// Or the sound file next to the video chosen by hand, by its place against the video.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_place: Option<String>,
    /// The subtitle track chosen by hand, or `subtitles_off` when they were switched off.
    pub subtitle_track: Option<i32>,
    pub subtitles_off: bool,
    /// When it was last played, in seconds since 1970.
    pub updated: u64,
}

impl Remembered {
    /// Where to begin: a little before where it stopped, or `None` for the beginning (it
    /// was hardly started, or it was watched through to the credits).
    pub fn resume_point(&self) -> Option<f64> {
        (self.position >= TOO_EARLY && !near_the_end(self.position, self.duration))
            .then(|| (self.position - RUN_UP).max(0.0))
    }

    /// The subtitle choice made by hand: `Some(None)` is "off".
    pub fn subtitles(&self) -> Option<Option<i32>> {
        match (self.subtitle_track, self.subtitles_off) {
            (Some(track), _) => Some(Some(track)),
            (None, true) => Some(None),
            (None, false) => None,
        }
    }
}

/// The last twentieth of a film (at least ten seconds, at most two minutes) is its credits:
/// stopping there counts as having finished.
fn near_the_end(position: f64, duration: f64) -> bool {
    duration > 0.0 && position >= duration - (duration * 0.05).clamp(10.0, 120.0)
}

/// Where a folder opened whole, or a playlist, was left.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Left {
    /// The file of it that was playing.
    file: String,
    updated: u64,
}

/// The sound chosen by hand for the last video played from a folder. The next episode is
/// played the same way: a series is one choice of dubbing, not one for every file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Sound {
    /// The place of a sound file against its video; empty for the video's own sound.
    place: String,
    updated: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    files: BTreeMap<String, Remembered>,
    /// By the folder's path, or the playlist file's.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    lists: BTreeMap<String, Left>,
    /// By the folder's path.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    sound: BTreeMap<String, Sound>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What the player is told when it opens a file it has seen before.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recalled {
    pub start: Option<f64>,
    pub audio_track: Option<i32>,
    pub subtitles: Option<Option<i32>>,
    /// The sound file to play with it, by its place against the video: the one chosen for
    /// this file, or else for the last file played from its folder.
    pub sound_place: Option<String>,
    /// The video's own sound was chosen by hand, for this file or for the last one played
    /// from its folder.
    pub own_sound: bool,
}

/// The note the interface shows for a while when a file is picked up where it was left: it
/// names the place and offers the beginning instead.
pub struct ResumeNote {
    from: f64,
    shown_since: Option<Instant>,
}

impl ResumeNote {
    /// How long the note stays, counted from the first picture.
    const STAYS: Duration = Duration::from_secs(10);

    pub fn new(from: f64) -> ResumeNote {
        ResumeNote {
            from,
            shown_since: None,
        }
    }

    /// The position to name, while the note is due. `picture` says a picture is on screen.
    pub fn showing(&mut self, picture: bool) -> Option<f64> {
        if !picture {
            return None;
        }
        let since = *self.shown_since.get_or_insert_with(Instant::now);
        (since.elapsed() < Self::STAYS).then_some(self.from)
    }
}

/// The history, and the file being played now.
pub struct Memory {
    path: PathBuf,
    stored: Stored,
    current: Option<Current>,
}

struct Current {
    key: String,
    /// The folder the file is in.
    folder: String,
    entry: Remembered,
    /// The entry differs from what is in the file.
    unsaved: bool,
    written: Instant,
    paused: bool,
}

impl Memory {
    /// The history that goes with a settings file: "history.json" beside "settings.json",
    /// "name.history.json" beside any other "name.json".
    pub fn beside(settings: &Path) -> Memory {
        let stem = settings
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("settings");
        let name = if stem == "settings" {
            "history.json".to_string()
        } else {
            format!("{stem}.history.json")
        };
        Memory::at(settings.with_file_name(name))
    }

    pub fn at(path: PathBuf) -> Memory {
        let stored = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Memory {
            path,
            stored,
            current: None,
        }
    }

    /// A file is being opened: finish with the one before, and say what is known of this one.
    pub fn opened(&mut self, file: &str) -> Recalled {
        self.closed();
        let folder = Path::new(file)
            .parent()
            .map(|folder| folder.to_string_lossy().into_owned())
            .unwrap_or_default();
        // What was chosen for the last file played from this folder stands for this one
        // too, unless something was chosen for this very file.
        let around = self.stored.sound.get(&folder).map(|sound| &sound.place);
        let Some(key) = fingerprint(Path::new(file)) else {
            return Recalled {
                sound_place: around.filter(|place| !place.is_empty()).cloned(),
                own_sound: around.is_some_and(String::is_empty),
                ..Recalled::default()
            };
        };
        let mut entry = self.stored.files.get(&key).cloned().unwrap_or_default();
        entry.name = Path::new(file)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let chosen_here = entry.audio_track.is_some() || entry.sound_place.is_some();
        let around = around.filter(|_| !chosen_here);
        let recalled = Recalled {
            start: entry.resume_point(),
            audio_track: entry.audio_track,
            subtitles: entry.subtitles(),
            sound_place: entry
                .sound_place
                .clone()
                .or_else(|| around.filter(|place| !place.is_empty()).cloned()),
            own_sound: entry.audio_track.is_some() || around.is_some_and(String::is_empty),
        };
        self.current = Some(Current {
            key,
            folder,
            entry,
            unsaved: false,
            written: Instant::now(),
            paused: false,
        });
        recalled
    }

    /// Where playback is now. Call it often, and only once the picture is on screen: before
    /// that the position is not yet the one playback will begin at.
    pub fn note(&mut self, position: f64, duration: f64, ended: bool, paused: bool) {
        let Some(current) = &mut self.current else {
            return;
        };
        let position = if ended || near_the_end(position, duration) {
            0.0
        } else {
            position
        };
        if (current.entry.position - position).abs() > 0.5 || current.entry.duration != duration {
            current.entry.position = position;
            current.entry.duration = duration;
            current.unsaved = true;
        }
        // At once when playback is paused (the player may well be closed next), otherwise
        // every few seconds.
        let just_paused = paused && !current.paused;
        current.paused = paused;
        if current.unsaved && (just_paused || current.written.elapsed() >= WRITE_EVERY) {
            self.write();
        }
    }

    /// The sound track was chosen by hand. `place` is given for a sound file next to the
    /// video (see `Sidecar::place`), empty when the file was opened by hand from elsewhere:
    /// such a choice cannot be made again without the user, so it is not kept.
    pub fn chose_audio(&mut self, track: i32, place: Option<&str>) {
        let Some(current) = &mut self.current else {
            return;
        };
        match place {
            None => {
                current.entry.audio_track = Some(track);
                current.entry.sound_place = None;
            }
            Some("") => return,
            Some(place) => {
                current.entry.audio_track = None;
                current.entry.sound_place = Some(place.to_string());
            }
        }
        current.unsaved = true;
        // The rest of the folder goes the same way.
        self.stored.sound.insert(
            current.folder.clone(),
            Sound {
                place: place.unwrap_or_default().to_string(),
                updated: now(),
            },
        );
        while self.stored.sound.len() > MOST_LISTS {
            let oldest = self
                .stored
                .sound
                .iter()
                .min_by_key(|(_, sound)| sound.updated)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => self.stored.sound.remove(&key),
                None => break,
            };
        }
        self.write();
    }

    /// The subtitle track was chosen by hand; `None` switches subtitles off.
    pub fn chose_subtitles(&mut self, track: Option<i32>) {
        if let Some(current) = &mut self.current {
            current.entry.subtitle_track = track;
            current.entry.subtitles_off = track.is_none();
            current.unsaved = true;
            self.write();
        }
    }

    /// The file a folder or a playlist was left on, if it has been played before.
    pub fn list_left_at(&self, list: &Path) -> Option<PathBuf> {
        let left = self.stored.lists.get(list.to_string_lossy().as_ref())?;
        Some(PathBuf::from(&left.file))
    }

    /// A file of a folder or a playlist is being played: the list is picked up there next time.
    pub fn list_at(&mut self, list: &Path, file: &Path) {
        let file = file.to_string_lossy().into_owned();
        let key = list.to_string_lossy().into_owned();
        if self
            .stored
            .lists
            .get(&key)
            .is_some_and(|left| left.file == file)
        {
            return;
        }
        self.stored.lists.insert(
            key,
            Left {
                file,
                updated: now(),
            },
        );
        while self.stored.lists.len() > MOST_LISTS {
            let oldest = self
                .stored
                .lists
                .iter()
                .min_by_key(|(_, left)| left.updated)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => self.stored.lists.remove(&key),
                None => break,
            };
        }
        self.report(self.save());
    }

    fn report(&self, saved: std::io::Result<()>) {
        if let Err(e) = saved {
            eprintln!(
                "[warn] cannot write the history to {}: {e}",
                self.path.display()
            );
        }
    }

    /// The file is being closed, or the player is.
    pub fn closed(&mut self) {
        if self.current.as_ref().is_some_and(|c| c.unsaved) {
            self.write();
        }
        self.current = None;
    }

    fn write(&mut self) {
        let Some(current) = &mut self.current else {
            return;
        };
        current.entry.updated = now();
        current.unsaved = false;
        current.written = Instant::now();
        self.stored
            .files
            .insert(current.key.clone(), current.entry.clone());
        while self.stored.files.len() > MOST_FILES {
            let oldest = self
                .stored
                .files
                .iter()
                .min_by_key(|(_, entry)| entry.updated)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => self.stored.files.remove(&key),
                None => break,
            };
        }
        self.report(self.save());
    }

    /// Written whole to a file beside the real one and moved over it, so that a player killed
    /// in the middle leaves the old history, not half of a new one.
    fn save(&self) -> std::io::Result<()> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let text = serde_json::to_string_pretty(&self.stored).map_err(std::io::Error::other)?;
        let beside = self.path.with_extension("json.new");
        std::fs::write(&beside, text)?;
        std::fs::rename(&beside, &self.path)
    }
}

/// What a file is known by: its size and a hash of its first 64 KiB.
fn fingerprint(file: &Path) -> Option<String> {
    let mut opened = std::fs::File::open(file).ok()?;
    let size = opened.metadata().ok()?.len();
    let mut head = Vec::with_capacity(64 * 1024);
    opened
        .by_ref()
        .take(64 * 1024)
        .read_to_end(&mut head)
        .ok()?;
    // FNV-1a: written out here because the hash must stay the same from one build of the
    // player to the next, which the standard library's hasher does not promise.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in head {
        hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(format!("{size:x}-{hash:016x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("bsv-history-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn stopped_at(position: f64, duration: f64) -> Remembered {
        Remembered {
            position,
            duration,
            ..Remembered::default()
        }
    }

    #[test]
    fn comes_back_a_little_before_where_it_stopped() {
        assert_eq!(stopped_at(614.0, 1708.0).resume_point(), Some(611.0));
    }

    #[test]
    fn a_film_hardly_started_or_watched_to_the_credits_starts_again() {
        assert_eq!(stopped_at(3.0, 1708.0).resume_point(), None);
        // The last twentieth of 28 minutes is 85 seconds.
        assert_eq!(stopped_at(1700.0, 1708.0).resume_point(), None);
        assert_eq!(stopped_at(1630.0, 1708.0).resume_point(), None);
        assert!(stopped_at(1600.0, 1708.0).resume_point().is_some());
        // Of a two-hour film, two minutes and no more.
        assert!(stopped_at(7000.0, 7200.0).resume_point().is_some());
        assert_eq!(stopped_at(7100.0, 7200.0).resume_point(), None);
    }

    #[test]
    fn the_place_and_the_tracks_come_back_after_a_restart_and_a_rename() {
        let folder = scratch("restart");
        let film = folder.join("film.mkv");
        std::fs::write(&film, b"not really a film").unwrap();
        let settings = folder.join("settings.json");

        let mut memory = Memory::beside(&settings);
        assert_eq!(memory.opened(film.to_str().unwrap()), Recalled::default());
        memory.note(600.0, 1700.0, false, false);
        memory.chose_audio(2, None);
        memory.chose_subtitles(None);
        memory.closed();
        assert!(folder.join("history.json").exists());

        let moved = folder.join("the same film.mkv");
        std::fs::rename(&film, &moved).unwrap();
        let mut memory = Memory::beside(&settings);
        assert_eq!(
            memory.opened(moved.to_str().unwrap()),
            Recalled {
                start: Some(597.0),
                audio_track: Some(2),
                subtitles: Some(None),
                sound_place: None,
                own_sound: true,
            }
        );
    }

    #[test]
    fn watching_to_the_end_forgets_the_place_but_not_the_tracks() {
        let folder = scratch("end");
        let film = folder.join("film.mkv");
        std::fs::write(&film, b"another film").unwrap();
        let settings = folder.join("quiet.json");

        let mut memory = Memory::beside(&settings);
        memory.opened(film.to_str().unwrap());
        memory.chose_subtitles(Some(4));
        memory.note(900.0, 1700.0, false, false);
        memory.note(1700.0, 1700.0, true, false);
        memory.closed();
        assert!(folder.join("quiet.history.json").exists());

        let mut memory = Memory::beside(&settings);
        assert_eq!(
            memory.opened(film.to_str().unwrap()),
            Recalled {
                start: None,
                audio_track: None,
                subtitles: Some(Some(4)),
                ..Recalled::default()
            }
        );
    }

    #[test]
    fn the_dubbing_chosen_for_one_episode_is_played_with_the_next() {
        let folder = scratch("dubbing");
        let episode = |n: u32| {
            let file = folder.join(format!("Show - {n:02}.mkv"));
            std::fs::write(&file, format!("episode {n}")).unwrap();
            file.to_string_lossy().into_owned()
        };
        let (first, second, third) = (episode(1), episode(2), episode(3));
        let elsewhere = scratch("dubbing-elsewhere").join("Film.mkv");
        std::fs::write(&elsewhere, b"a film").unwrap();
        let settings = folder.join("settings.json");

        let mut memory = Memory::beside(&settings);
        assert_eq!(memory.opened(&first), Recalled::default());
        memory.chose_audio(2000, Some("RUS Sound/[AniDub]|.mka"));
        // A file opened by hand from somewhere else changes nothing: it cannot be found again.
        memory.chose_audio(2003, Some(""));

        let mut memory = Memory::beside(&settings);
        let with_the_dubbing = Recalled {
            sound_place: Some("RUS Sound/[AniDub]|.mka".into()),
            ..Recalled::default()
        };
        assert_eq!(memory.opened(&first), with_the_dubbing);
        assert_eq!(memory.opened(&second), with_the_dubbing);
        // A video in another folder is not part of this.
        assert_eq!(
            memory.opened(elsewhere.to_str().unwrap()),
            Recalled::default()
        );

        // Going back to the video's own sound in the second episode holds for the third,
        // and leaves the first as it was chosen.
        memory.opened(&second);
        memory.chose_audio(1, None);
        assert_eq!(
            memory.opened(&third),
            Recalled {
                own_sound: true,
                ..Recalled::default()
            }
        );
        assert_eq!(memory.opened(&first), with_the_dubbing);
        assert_eq!(
            memory.opened(&second),
            Recalled {
                audio_track: Some(1),
                own_sound: true,
                ..Recalled::default()
            }
        );
    }

    #[test]
    fn a_playlist_is_picked_up_on_the_file_it_was_left_on() {
        let folder = scratch("lists");
        let settings = folder.join("settings.json");
        let list = folder.join("evening.m3u8");
        let mut memory = Memory::beside(&settings);
        assert_eq!(memory.list_left_at(&list), None);
        memory.list_at(&list, &folder.join("1.mkv"));
        memory.list_at(&list, &folder.join("2.mkv"));
        let memory = Memory::beside(&settings);
        assert_eq!(memory.list_left_at(&list), Some(folder.join("2.mkv")));
        assert_eq!(memory.list_left_at(&folder), None);
    }

    #[test]
    fn two_files_of_one_size_are_told_apart() {
        let folder = scratch("apart");
        let (one, other) = (folder.join("1.mkv"), folder.join("2.mkv"));
        std::fs::write(&one, b"episode one").unwrap();
        std::fs::write(&other, b"episode two").unwrap();
        assert_ne!(fingerprint(&one), fingerprint(&other));
    }
}

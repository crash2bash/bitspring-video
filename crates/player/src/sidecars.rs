//! Subtitle and sound files that belong to a video without being inside it.
//!
//! A file is taken to belong when its name begins with the video's name: "film.srt",
//! "film.ru.ass", "film [Group].ass", "film.mka". It may sit beside the video or a few folders
//! below it, the way releases with several translations are laid out ("Subs/",
//! "RUS Subs/[Group]/", "RUS Sound/[Studio]/"). Fonts for styled subtitles are looked for in a
//! "Fonts" folder near the subtitle file.

use std::path::{Path, PathBuf};

use crate::language;

const ENDINGS: [&str; 4] = ["srt", "ass", "ssa", "vtt"];
const SOUND_ENDINGS: [&str; 18] = [
    "mka", "ac3", "eac3", "ec3", "dts", "dtshd", "thd", "truehd", "aac", "m4a", "flac", "opus",
    "ogg", "oga", "mp3", "mp2", "wav", "wv",
];
const FONT_ENDINGS: [&str; 4] = ["ttf", "otf", "ttc", "otc"];
/// How many folders below the video's a subtitle file may be: "Subs/Rus/[Group]".
const DEPTH: usize = 3;
/// Folders looked into at most, so that a video in a crowded folder does not start a long walk.
const MOST_FOLDERS: usize = 64;
/// With more folders than this beside the video, only those named like subtitles are opened.
const FEW_FOLDERS: usize = 24;
const MOST_FILES: usize = 32;
/// Fonts read for one subtitle file at most, and the size of the largest one read.
const MOST_FONTS: usize = 400;
const LARGEST_FONT: u64 = 48 * 1024 * 1024;

/// One subtitle or sound file found for a video.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sidecar {
    pub path: String,
    /// The language its name or its folder suggests, as an ISO 639 code; empty when neither does.
    pub language: String,
    /// What to call it in a menu. A subtitle file: its name, with its folder when it is not
    /// beside the video. A sound file below the video: its folders, which say whose it is.
    pub title: String,
    /// Where it sits against the video: its folders below the video's, and what its name adds
    /// to the video's. The next episode's file of the same translation sits in the same place.
    pub place: String,
}

/// Which of the two kinds of file is looked for.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Subtitles,
    Sound,
}

impl Kind {
    fn takes(self, path: &Path) -> bool {
        ending(path).is_some_and(|e| match self {
            Kind::Subtitles => ENDINGS.contains(&e.as_str()),
            Kind::Sound => SOUND_ENDINGS.contains(&e.as_str()),
        })
    }

    /// A folder that by its name holds files of this kind.
    fn names(self, folder: &str) -> bool {
        let folder = folder.to_lowercase();
        let words: &[&str] = match self {
            Kind::Subtitles => &["sub", "суб", "титр", "caption", "надпис"],
            Kind::Sound => &["sound", "audio", "звук", "озвуч", "dub", "дубл", "voice"],
        };
        words.iter().any(|word| folder.contains(word))
    }
}

/// Whether a file is one of the kinds of subtitle file the player reads, going by its name.
pub fn is_subtitle_file(path: &str) -> bool {
    Kind::Subtitles.takes(Path::new(path))
}

/// Whether a file holds sound and no picture, going by its name.
pub fn is_sound_file(path: &str) -> bool {
    Kind::Sound.takes(Path::new(path))
}

fn ending(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_lowercase())
}

fn visible(name: &str) -> bool {
    !name.starts_with('.')
}

/// The language a subtitle or sound file is in, as far as names can tell: first what follows the
/// video's name in its own ("film.ru.srt", "film.eng.forced.srt"), then the folders it is
/// in ("RUS Subs").
fn language_of(rest: &str, folders: &[String]) -> String {
    // Between dots, any short word is taken for a code, known or not ("film.tlh.srt"),
    // unless it is the name of a sound format ("film.dts.mka").
    const FORMATS: [&str; 11] = [
        "dts", "aac", "ogg", "mka", "wav", "dub", "thd", "pcm", "ddp", "eac", "hd",
    ];
    let between_dots = rest.split('.').find(|part| {
        (2..=3).contains(&part.len())
            && part.chars().all(|c| c.is_ascii_alphabetic())
            && !FORMATS.contains(part)
    });
    if let Some(code) = between_dots {
        return code.to_ascii_lowercase();
    }
    let words = |text: &str| -> Option<&'static str> {
        text.split(|c: char| !c.is_alphabetic())
            .find_map(language::named)
    };
    words(rest)
        .or_else(|| folders.iter().rev().find_map(|f| words(f)))
        .unwrap_or("")
        .to_string()
}

/// The subtitle files that belong to `video`: those beside it first, then those in folders
/// below it, each group in the order of their names.
pub fn find(video: &str) -> Vec<Sidecar> {
    find_of(Kind::Subtitles, video)
}

/// The sound files that belong to `video`, in the same order.
pub fn find_sound(video: &str) -> Vec<Sidecar> {
    find_of(Kind::Sound, video)
}

fn find_of(kind: Kind, video: &str) -> Vec<Sidecar> {
    let path = Path::new(video);
    let (Some(folder), Some(stem)) = (path.parent(), path.file_stem().and_then(|s| s.to_str()))
    else {
        return Vec::new();
    };
    let folder = if folder.as_os_str().is_empty() {
        Path::new(".")
    } else {
        folder
    };
    let stem = stem.to_lowercase();
    let mut found: Vec<(usize, Sidecar)> = Vec::new();
    let mut folders_left = MOST_FOLDERS;
    look_in(
        kind,
        folder,
        &stem,
        &mut Vec::new(),
        &mut folders_left,
        &mut found,
    );
    found.sort_by(|a, b| (a.0.min(1), &a.1.path).cmp(&(b.0.min(1), &b.1.path)));
    found.truncate(MOST_FILES);
    found.into_iter().map(|(_, sidecar)| sidecar).collect()
}

fn look_in(
    kind: Kind,
    folder: &Path,
    stem: &str,
    below: &mut Vec<String>,
    folders_left: &mut usize,
    found: &mut Vec<(usize, Sidecar)>,
) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut folders = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !visible(&name) {
            continue;
        }
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            folders.push((name, path));
            continue;
        }
        if !kind.takes(&path) {
            continue;
        }
        // A link is taken only if it leads to a file on this machine.
        if entry.file_type().is_ok_and(|t| t.is_symlink()) && !crate::queue::stays_here(&path) {
            continue;
        }
        let lower = name.to_lowercase();
        let Some(rest) = lower.strip_prefix(stem) else {
            continue;
        };
        // "Show - 1.mkv" is not the video of "Show - 10.ass".
        if rest.chars().next().is_some_and(char::is_alphanumeric) {
            continue;
        }
        // What stands between the video's name and the ending.
        let middle = rest.rsplit_once('.').map_or("", |(middle, _)| middle);
        let title = match kind {
            _ if below.is_empty() => name.clone(),
            Kind::Subtitles => format!("{}/{name}", below.join("/")),
            // A sound file is named as the video is: its folders are what tells it apart.
            Kind::Sound => {
                let own = name
                    .get(stem.len()..name.len() + middle.len() - rest.len())
                    .unwrap_or(middle)
                    .trim_matches(|c: char| c.is_whitespace() || ".-_".contains(c));
                [below.join("/"), own.to_string()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        };
        found.push((
            below.len(),
            Sidecar {
                path: path.to_string_lossy().into_owned(),
                language: language_of(middle, below),
                title,
                place: format!("{}|{rest}", below.join("/")),
            },
        ));
    }
    if below.len() >= DEPTH {
        return;
    }
    // Beside the video there may be a great many folders that have nothing to do with it.
    if below.is_empty() && folders.len() > FEW_FOLDERS {
        folders.retain(|(name, _)| kind.names(name) || name.to_lowercase() == stem);
    }
    folders.sort();
    for (name, path) in folders {
        if *folders_left == 0 {
            return;
        }
        *folders_left -= 1;
        below.push(name);
        look_in(kind, &path, stem, below, folders_left, found);
        below.pop();
    }
}

/// The font files that come with a subtitle file: everything in folders named "Fonts" from
/// the subtitle file's own folder up to `top` (the video's folder), or in its own folder
/// only when `top` is not above it.
pub fn fonts_for(subtitles: &Path, top: Option<&Path>) -> Vec<PathBuf> {
    let mut fonts = Vec::new();
    let mut folder = subtitles.parent();
    let mut levels = 0;
    while let Some(here) = folder {
        if let Ok(entries) = std::fs::read_dir(here) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if matches!(name.as_str(), "fonts" | "font" | "шрифты")
                    && entry.file_type().is_ok_and(|t| t.is_dir())
                {
                    font_files(&entry.path(), &mut fonts);
                }
            }
        }
        levels += 1;
        if top.is_none_or(|top| here == top) || levels > DEPTH {
            break;
        }
        folder = here.parent();
    }
    fonts
}

fn font_files(folder: &Path, fonts: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut here: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(visible))
        .filter(|e| {
            e.metadata()
                .is_ok_and(|m| m.is_file() && m.len() <= LARGEST_FONT)
        })
        .map(|e| e.path())
        .filter(|p| ending(p).is_some_and(|e| FONT_ENDINGS.contains(&e.as_str())))
        .collect();
    here.sort();
    here.truncate(MOST_FONTS.saturating_sub(fonts.len()));
    fonts.append(&mut here);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("bsv-sidecars-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn touch(path: PathBuf) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        path
    }

    fn titles(found: &[Sidecar]) -> Vec<(&str, &str)> {
        found
            .iter()
            .map(|s| (s.title.as_str(), s.language.as_str()))
            .collect()
    }

    #[test]
    fn files_beside_the_video_are_found_with_their_language() {
        let folder = scratch("beside");
        let video = touch(folder.join("Film.mkv"));
        touch(folder.join("Film.srt"));
        touch(folder.join("Film.ru.ass"));
        touch(folder.join("film.ENG.forced.srt"));
        touch(folder.join("Film [Group].ass"));
        touch(folder.join("Filmography.srt"));
        touch(folder.join("Other.srt"));
        touch(folder.join("Film.txt"));
        touch(folder.join("._Film.srt"));
        assert_eq!(
            titles(&find(video.to_str().unwrap())),
            [
                ("Film [Group].ass", ""),
                ("Film.ru.ass", "ru"),
                ("Film.srt", ""),
                ("film.ENG.forced.srt", "eng"),
            ]
        );
    }

    #[test]
    fn episode_one_does_not_take_episode_ten() {
        let folder = scratch("ten");
        let video = touch(folder.join("Show - 1.mkv"));
        touch(folder.join("Show - 1.ass"));
        touch(folder.join("Show - 10.ass"));
        touch(folder.join("Show - 1v2.ass"));
        assert_eq!(
            titles(&find(video.to_str().unwrap())),
            [("Show - 1.ass", "")]
        );
    }

    #[test]
    fn folders_below_the_video_are_looked_into() {
        let folder = scratch("below");
        let video = touch(folder.join("[G] Show - 02 [1080p].mkv"));
        touch(folder.join("RUS Subs/[Anku]/[G] Show - 02 [1080p].ass"));
        touch(folder.join("RUS Subs/[Crunchyroll]/[G] Show - 02 [1080p].ass"));
        touch(folder.join("ENG Subs/[G] Show - 02 [1080p].ass"));
        touch(folder.join("Subs/Japanese/[G] Show - 02 [1080p].srt"));
        touch(folder.join("RUS Subs/[Anku]/[G] Show - 03 [1080p].ass"));
        touch(folder.join("a/b/c/d/[G] Show - 02 [1080p].ass"));
        touch(folder.join("[G] Show - 02 [1080p].srt"));
        assert_eq!(
            titles(&find(video.to_str().unwrap())),
            [
                ("[G] Show - 02 [1080p].srt", ""),
                ("ENG Subs/[G] Show - 02 [1080p].ass", "eng"),
                ("RUS Subs/[Anku]/[G] Show - 02 [1080p].ass", "rus"),
                ("RUS Subs/[Crunchyroll]/[G] Show - 02 [1080p].ass", "rus"),
                ("Subs/Japanese/[G] Show - 02 [1080p].srt", "jpn"),
            ]
        );
    }

    #[test]
    fn in_a_crowded_folder_only_subtitle_folders_are_opened() {
        let folder = scratch("crowded");
        let video = touch(folder.join("Film.mkv"));
        for n in 0..40 {
            touch(folder.join(format!("Series {n:02}/Film.srt")));
        }
        touch(folder.join("Субтитры/Film.srt"));
        touch(folder.join("Film/Film.en.srt"));
        assert_eq!(
            titles(&find(video.to_str().unwrap())),
            [("Film/Film.en.srt", "en"), ("Субтитры/Film.srt", "")]
        );
    }

    #[test]
    fn fonts_are_taken_from_fonts_folders_up_to_the_video() {
        let folder = scratch("fonts");
        let video = touch(folder.join("Show - 01.mkv"));
        let subtitles = touch(folder.join("Subs/[G]/Show - 01.ass"));
        let near = touch(folder.join("Subs/[G]/Fonts/A.ttf"));
        let above = touch(folder.join("Subs/fonts/B.OTF"));
        let top = touch(folder.join("Fonts/C.ttc"));
        touch(folder.join("Fonts/readme.txt"));
        touch(folder.join("Subs/Other/Fonts/D.ttf"));
        assert_eq!(
            fonts_for(&subtitles, video.parent()),
            [near.clone(), above, top]
        );
        // A subtitle file from somewhere else brings only what is in its own folder.
        assert_eq!(fonts_for(&subtitles, None), [near]);
    }

    #[test]
    fn sound_files_are_found_and_named_by_where_they_are() {
        let folder = scratch("sound");
        let video = touch(folder.join("[G] Show - 02 [1080p].mkv"));
        touch(folder.join("RUS Sound/[AniLibria]/[G] Show - 02 [1080p].mka"));
        touch(folder.join("RUS Sound/[AniDub]/[G] Show - 02 [1080p].AC3"));
        touch(folder.join("Sound/[G] Show - 02 [1080p].Studio Band.mka"));
        touch(folder.join("[G] Show - 02 [1080p].eng.flac"));
        touch(folder.join("RUS Sound/[AniLibria]/[G] Show - 03 [1080p].mka"));
        touch(folder.join("RUS Subs/[G] Show - 02 [1080p].ass"));
        touch(folder.join("[G] Show - 02 [1080p].txt"));
        let found = find_sound(video.to_str().unwrap());
        assert_eq!(
            titles(&found),
            [
                ("[G] Show - 02 [1080p].eng.flac", "eng"),
                ("RUS Sound/[AniDub]", "rus"),
                ("RUS Sound/[AniLibria]", "rus"),
                ("Sound Studio Band", ""),
            ]
        );
        // The same translation of the next episode sits in the same place.
        let next = touch(folder.join("[G] Show - 03 [1080p].mkv"));
        let places =
            |found: &[Sidecar]| -> Vec<String> { found.iter().map(|s| s.place.clone()).collect() };
        assert_eq!(
            places(&found),
            [
                "|.eng.flac",
                "RUS Sound/[AniDub]|.ac3",
                "RUS Sound/[AniLibria]|.mka",
                "Sound|.studio band.mka"
            ]
        );
        assert_eq!(
            places(&find_sound(next.to_str().unwrap())),
            ["RUS Sound/[AniLibria]|.mka"]
        );
        // A sound format in the name is not a language: the folder still says which it is.
        let named = touch(folder.join("RUS Sound/[AniDub]/[G] Show - 03 [1080p].dts.mka"));
        let found = find_sound(next.to_str().unwrap());
        assert_eq!(Path::new(&found[0].path), named);
        assert_eq!(found[0].language, "rus");
        std::fs::remove_file(named).unwrap();
        // Subtitle files are not sound files, and the other way round.
        assert_eq!(find(video.to_str().unwrap()).len(), 1);
    }

    #[test]
    fn subtitle_files_are_known_by_their_ending() {
        assert!(is_sound_file("/films/Film.MKA"));
        assert!(!is_sound_file("film.mkv"));
        assert!(!is_sound_file("film.srt"));
        assert!(is_subtitle_file("/films/Film.RU.ASS"));
        assert!(is_subtitle_file("film.srt"));
        assert!(!is_subtitle_file("film.mkv"));
        assert!(!is_subtitle_file("srt"));
    }
}

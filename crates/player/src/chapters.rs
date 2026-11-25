//! Chapters: which one is playing, and where the next and the previous begin.

use bsv_engine::Chapter;

/// A position this close before a chapter's start already counts as in it: a jump lands on
/// the picture at or just before the time asked for.
const SLACK: f64 = 0.4;

/// Into a chapter by more than this, "previous" goes back to its own start, as a CD player
/// does; nearer its start, to the chapter before.
const RESTART: f64 = 3.0;

/// The chapters a file lists, made fit to show and to step through: in order of their
/// starts, none before the film's beginning or after its end, no two at one moment.
/// Files list chapters in any order, sometimes the same one twice, and some keep a last
/// marker for the very end.
pub fn tidy(mut chapters: Vec<Chapter>, duration: f64) -> Vec<Chapter> {
    for chapter in &mut chapters {
        chapter.start = chapter.start.max(0.0);
    }
    chapters.retain(|c| c.start.is_finite() && (duration <= 0.0 || c.start < duration - 1.0));
    chapters.sort_by(|a, b| a.start.total_cmp(&b.start));
    chapters.dedup_by(|later, earlier| later.start - earlier.start < 0.5);
    // A file may mark only what comes late, such as the credits: what is before them is a
    // chapter too, if a nameless one.
    if chapters.first().is_some_and(|c| c.start > 1.0) {
        chapters.insert(
            0,
            Chapter {
                start: 0.0,
                end: None,
                title: String::new(),
            },
        );
    }
    // A single chapter says nothing a seek bar does not.
    if chapters.len() < 2 {
        chapters.clear();
    }
    chapters
}

/// The chapter playing at `position`, by its place in the list.
pub fn current(chapters: &[Chapter], position: f64) -> Option<usize> {
    chapters.iter().rposition(|c| c.start <= position + SLACK)
}

/// Where to go for the next chapter (`step` 1) or the previous one (`step` -1). `None` when
/// there is none that way: after the last chapter, or with no chapters at all.
pub fn target(chapters: &[Chapter], position: f64, step: i32) -> Option<f64> {
    if chapters.is_empty() {
        return None;
    }
    let now = current(chapters, position);
    if step > 0 {
        let next = now.map_or(0, |n| n + 1);
        return chapters.get(next).map(|c| c.start);
    }
    match now {
        // Before the first chapter: back to the film's beginning.
        None => Some(0.0),
        Some(n) if position - chapters[n].start > RESTART => Some(chapters[n].start),
        Some(0) => (position > SLACK).then_some(0.0),
        Some(n) => Some(chapters[n - 1].start),
    }
}

/// What a "next track" or "previous track" key does in a film.
#[derive(Debug, PartialEq)]
pub enum Skip {
    /// Go to this place in the file playing.
    Seek(f64),
    /// Go to the next file, or the previous one.
    File,
}

/// The next chapter, or once there is none the next file; backwards, the start of the
/// chapter (or of a film without chapters) unless it has only just begun, then the one before.
pub fn skip(chapters: &[Chapter], position: f64, step: i32) -> Skip {
    match target(chapters, position, step) {
        Some(at) => Skip::Seek(at),
        None if step < 0 && position > RESTART => Skip::Seek(0.0),
        None => Skip::File,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(starts: &[f64]) -> Vec<Chapter> {
        starts
            .iter()
            .map(|&start| Chapter {
                start,
                end: None,
                title: String::new(),
            })
            .collect()
    }

    #[test]
    fn tidying_orders_and_drops_what_cannot_be_stepped_to() {
        let tidy_starts = |starts: &[f64], duration| -> Vec<f64> {
            tidy(at(starts), duration).iter().map(|c| c.start).collect()
        };
        assert_eq!(
            tidy_starts(&[600.0, 0.0, 90.0, 90.2], 1200.0),
            [0.0, 90.0, 600.0]
        );
        // A marker for the end itself is not a chapter one can watch.
        assert_eq!(tidy_starts(&[0.0, 600.0, 1199.8], 1200.0), [0.0, 600.0]);
        assert_eq!(tidy_starts(&[-0.04, 300.0], 1200.0), [0.0, 300.0]);
        assert!(tidy_starts(&[0.0], 1200.0).is_empty());
        // Only the credits marked: the film before them is the first chapter.
        assert_eq!(tidy_starts(&[3362.0], 3427.0), [0.0, 3362.0]);
        assert_eq!(tidy_starts(&[1.0, 62.0], 3400.0), [1.0, 62.0]);
        // A length unknown: nothing is taken for being after it.
        assert_eq!(tidy_starts(&[0.0, 5000.0], 0.0), [0.0, 5000.0]);
    }

    #[test]
    fn the_chapter_playing_counts_a_landing_just_short_of_its_start() {
        let chapters = at(&[0.0, 90.0, 600.0]);
        assert_eq!(current(&chapters, 10.0), Some(0));
        assert_eq!(current(&chapters, 89.8), Some(1));
        assert_eq!(current(&chapters, 700.0), Some(2));
        assert_eq!(current(&at(&[30.0, 90.0]), 10.0), None);
    }

    #[test]
    fn next_goes_to_the_following_start_and_stops_at_the_last() {
        let chapters = at(&[0.0, 90.0, 600.0]);
        assert_eq!(target(&chapters, 10.0, 1), Some(90.0));
        // Having just jumped to 90, the next press goes on, not to 90 again.
        assert_eq!(target(&chapters, 89.9, 1), Some(600.0));
        assert_eq!(target(&chapters, 700.0, 1), None);
        assert_eq!(target(&at(&[30.0, 90.0]), 10.0, 1), Some(30.0));
        assert_eq!(target(&[], 10.0, 1), None);
    }

    #[test]
    fn previous_restarts_a_chapter_unless_it_has_only_just_begun() {
        let chapters = at(&[0.0, 90.0, 600.0]);
        assert_eq!(target(&chapters, 650.0, -1), Some(600.0));
        assert_eq!(target(&chapters, 601.0, -1), Some(90.0));
        assert_eq!(target(&chapters, 91.0, -1), Some(0.0));
        assert_eq!(target(&chapters, 1.0, -1), Some(0.0));
        // At the very beginning there is nowhere further back.
        assert_eq!(target(&chapters, 0.1, -1), None);
        assert_eq!(target(&at(&[30.0, 90.0]), 10.0, -1), Some(0.0));
        assert_eq!(target(&[], 10.0, -1), None);
    }

    #[test]
    fn a_track_key_walks_chapters_then_files() {
        let chapters = at(&[0.0, 90.0, 600.0]);
        assert_eq!(skip(&chapters, 10.0, 1), Skip::Seek(90.0));
        assert_eq!(skip(&chapters, 700.0, 1), Skip::File);
        assert_eq!(skip(&chapters, 0.1, -1), Skip::File);
        // A film without chapters is one track: back to its start, and from there to the one before.
        assert_eq!(skip(&[], 700.0, 1), Skip::File);
        assert_eq!(skip(&[], 700.0, -1), Skip::Seek(0.0));
        assert_eq!(skip(&[], 2.0, -1), Skip::File);
    }
}

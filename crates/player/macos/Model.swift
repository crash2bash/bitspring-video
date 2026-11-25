// What the interface knows: the player's state as the engine reports it, and the few bits of
// state that belong to the interface alone (are the controls showing, is the panel open).

import AppKit
import Foundation
import SwiftUI

// MARK: - State sent by the engine (JSON, see `ui.rs`)

struct VideoInfo: Codable, Equatable {
    var codec: String
    var width: Int
    var height: Int
    var bitDepth: Int
    var fps: Double
    /// "BT.709", "Display P3", "BT.2020".
    var primaries: String
    /// "SDR", "PQ", "HLG".
    var transfer: String
    var masteringMin: Double?
    var masteringMax: Double?
    var maxCll: Double?
    /// "5", "8.1"; nil when the file is not Dolby Vision.
    var dolbyVision: String?
    /// What a Dolby Vision file is without its metadata ("HDR10", "HLG", "SDR"), when it can
    /// be played that way; nil otherwise.
    var baseLayer: String?
    var enhancementLayer: Bool
    var hardware: Bool
}

struct SignalInfo: Codable, Equatable {
    var dolbyVision: Bool
    var hdr10Plus: Bool
    /// Peak of the current scene from dynamic metadata, 0 when there is none.
    var scenePeak: Double
    /// The surface the system gave: "SDR", "PQ", "HLG".
    var outputTransfer: String
    var outputPrimaries: String
    var outputBits: Int
    var outputPeak: Double
    var passthrough: Bool
    /// The picture on screen was doubled by the neural network before being fitted to the window.
    var upscaled: Bool
    /// The video is interlaced; its fields are made whole before they are shown.
    var interlaced: Bool
    /// Frames in between: "off", "on", "unsuited" (not for this video), "slow" (given up),
    /// "unavailable" (the method does not run on this machine).
    var inBetween: String
}

struct SoundInfo: Codable, Equatable {
    var codec: String
    var profile: String
    var layout: String
    var language: String
    var sampleRate: Int
    var channels: Int
    var device: String
    var deviceRate: Int
    var deviceChannels: Int
}

/// A sound or subtitle track the file offers.
struct TrackInfo: Codable, Equatable, Identifiable {
    /// The stream's number in the file.
    var id: Int
    var codec: String
    var profile: String
    /// ISO 639 code as the file gives it: "eng", "rus", "und".
    var language: String
    var title: String
    var layout: String
    var forced: Bool
    var hearingImpaired: Bool
    var pictures: Bool
}

/// How the picture is placed in the window.
struct PictureInfo: Codable, Equatable {
    /// Width over height it is shown at; 0 for its own.
    var shape: Double
    /// The shape it is cut down to; 0 for none.
    var cut: Double
    var fill: Bool
}

/// A small picture of the video, for over the seek bar.
struct ThumbnailInfo: Codable, Equatable {
    var at: Double
    var path: String
}

/// A chapter of the file.
struct ChapterInfo: Codable, Equatable {
    /// Seconds from the start.
    var start: Double
    /// Empty when the file gives it none.
    var title: String
}

struct DisplayInfo: Codable, Equatable {
    var name: String
    /// "on", "off" (supported but switched off in the system), "unsupported".
    var hdr: String
    var peak: Double
    var min: Double?
    /// Extended-range headroom the system is giving right now, 1 when none.
    var headroom: Double
    var peakKnown: Bool
}

/// The preferences the settings panel shows and changes.
struct SettingsInfo: Codable, Equatable {
    /// The user's own figure for the display's peak, cd/m²; nil to believe the display.
    var displayPeak: Double?
    /// What the display itself reports.
    var reportedPeak: Double
    /// "high", "balanced", "fast".
    var quality: String
    /// "off", "faithful", "restoring": a neural network doubles pictures smaller than the window.
    var upscaling: String
    /// "off", "fast", "neural": a frame is made between every two; and which this build can do.
    var interpolation: String
    var interpolations: [String]
    var hardwareDecoding: Bool
    /// A file that is both Dolby Vision and a plain signal is played as Dolby Vision.
    var dolbyVision: Bool
    var subtitleScale: Double
    var subtitleNits: Double
    /// ISO 639 code, or empty for forced subtitles only.
    var subtitleLanguage: String
    /// ISO 639 code, or empty for the file's own choice.
    var audioLanguage: String
    /// Milliseconds the picture is held back.
    var audioDelay: Double
    /// When a file ends, the next one of its series or its playlist starts by itself.
    var autoNext: Bool
    /// Loud passages brought down towards the speech, for listening late or quietly.
    var nightMode: Bool

    static let standard = SettingsInfo(
        displayPeak: nil, reportedPeak: 0, quality: "high", upscaling: "off", interpolation: "off", interpolations: [], hardwareDecoding: true, dolbyVision: true,
        subtitleScale: 1,
        subtitleNits: 203, subtitleLanguage: "", audioLanguage: "", audioDelay: 0, autoNext: true, nightMode: false)
}

/// One file of the queue.
struct QueueItem: Codable, Equatable {
    /// What to call it in the list: its name without what it shares with its neighbours.
    var label: String
    var name: String
}

/// What plays after what: the videos of the open file's folder, or a playlist.
struct QueueInfo: Codable, Equatable {
    /// "folder" or "playlist".
    var kind: String
    /// The folder's name or the playlist's; empty for a list not saved yet.
    var name: String
    /// A playlist changed since it was read or saved.
    var unsaved: Bool
    var current: Int?
    /// Whether the next file starts by itself when this one ends.
    var continues: Bool
    var items: [QueueItem]
}

/// A file played lately.
struct RecentInfo: Codable, Equatable, Identifiable {
    var name: String
    var path: String
    /// Where it was stopped, 0 once watched to the end; and how long it is.
    var position: Double
    var duration: Double
    var id: String { path }
}

struct Status: Codable, Equatable {
    /// Name of the open file; nil when nothing is open.
    var file: String?
    /// Its whole path.
    var path: String?
    /// Files played lately, for when nothing is open.
    var recent: [RecentInfo]
    var error: String?
    var position: Double
    var duration: Double
    var paused: Bool
    var ended: Bool
    /// Where this file was picked up from, for the few seconds the note about it shows.
    var resumedFrom: Double?
    var volume: Double
    var muted: Bool
    /// How fast it plays, 1 as made.
    var speed: Double
    /// Seconds subtitles are shown late (negative: early).
    var subtitleDelay: Double
    var picture: PictureInfo
    /// The latest small picture made for over the seek bar.
    var thumbnail: ThumbnailInfo?
    /// Frames saved so far, and the last one's file or why it could not be saved.
    var savedFrames: Int
    var savedFrame: String?
    var savedFrameError: String?
    var fullscreen: Bool
    /// "auto", "passthrough", "sdr".
    var mode: String
    var video: VideoInfo?
    var signal: SignalInfo?
    var sound: SoundInfo?
    var audioTracks: [TrackInfo]
    var subtitleTracks: [TrackInfo]
    /// The tracks in use, by `TrackInfo.id`; nil for none.
    var audioTrack: Int?
    var subtitleTrack: Int?
    /// In order; empty when the file has fewer than two.
    var chapters: [ChapterInfo]
    /// The chapter playing, by its place in `chapters`.
    var chapter: Int?
    var display: DisplayInfo
    var settings: SettingsInfo
    var queue: QueueInfo?

    static let empty = Status(
        file: nil, path: nil, recent: [], error: nil, position: 0, duration: 0, paused: false, ended: false,
        resumedFrom: nil, volume: 1,
        muted: false, speed: 1, subtitleDelay: 0, picture: PictureInfo(shape: 0, cut: 0, fill: false),
        thumbnail: nil, savedFrames: 0, savedFrame: nil, savedFrameError: nil, fullscreen: false, mode: "auto", video: nil, signal: nil, sound: nil,
        audioTracks: [], subtitleTracks: [], audioTrack: nil, subtitleTrack: nil,
        chapters: [], chapter: nil,
        display: DisplayInfo(name: "", hdr: "unsupported", peak: 0, min: nil, headroom: 1, peakKnown: false),
        settings: .standard)
}

// MARK: - What is being done to the picture

/// The one judgement the interface makes: is the picture shown as it was mastered, or changed
/// to fit the display. Colour in the interface means this and nothing else.
enum Treatment: Equatable {
    /// SDR shown as SDR.
    case asIs
    /// HDR whose range fits the display: nothing is compressed.
    case asMastered
    /// HDR sent on untouched for the display to map.
    case passedThrough
    /// HDR brighter than the display: highlights are mapped into its range.
    case mapped(to: Double)
    /// HDR on a display that is showing SDR.
    case toSdr

    var untouched: Bool {
        switch self {
        case .asIs, .asMastered, .passedThrough: return true
        case .mapped, .toSdr: return false
        }
    }
}

extension Status {
    /// The chapter at this time, by its place in `chapters`. Like the player, a time just
    /// short of a chapter's start counts as in it.
    func chapter(at seconds: Double) -> Int? {
        chapters.lastIndex { $0.start <= seconds + 0.4 }
    }

    /// What to call the n-th chapter: its own title, with the plain numbered ones a file
    /// often carries ("Chapter 03") said in the interface's language.
    func chapterName(_ n: Int) -> String {
        let title = chapters[n].title.trimmingCharacters(in: .whitespaces)
        if let match = title.wholeMatch(of: #/(?i)chapter\s*0*(\d+)/#) {
            return L("Chapter \(match.1)", "Глава \(match.1)")
        }
        return title.isEmpty ? L("Chapter \(n + 1)", "Глава \(n + 1)") : title
    }

    var isHdrSource: Bool { video.map { $0.transfer != "SDR" } ?? false }

    /// Brightest level the content claims, in cd/m².
    var contentPeak: Double? { video?.maxCll ?? video?.masteringMax }

    /// "Dolby Vision", "HDR10+", "HDR10", "HLG", "SDR".
    var formatName: String {
        guard let video else { return "" }
        if playsAsDolbyVision { return "Dolby Vision" }
        if signal?.hdr10Plus == true { return "HDR10+" }
        switch video.transfer {
        case "PQ": return "HDR10"
        case "HLG": return "HLG"
        default: return "SDR"
        }
    }

    /// The file is Dolby Vision and is shown as such, not as the plain signal it also is.
    var playsAsDolbyVision: Bool {
        guard let video, video.dolbyVision != nil else { return false }
        // What reached the screen, once something has; until then, what was asked for.
        return signal?.dolbyVision ?? (video.baseLayer == nil || settings.dolbyVision)
    }

    var treatment: Treatment? {
        guard let video, let signal else { return nil }
        if video.transfer == "SDR" { return .asIs }
        if signal.outputTransfer == "SDR" { return .toSdr }
        if signal.passthrough { return .passedThrough }
        if let peak = contentPeak, peak > signal.outputPeak * 1.05 { return .mapped(to: signal.outputPeak) }
        return .asMastered
    }
}

// MARK: - Interface state

@MainActor
final class PlayerModel: ObservableObject {
    @Published var status = Status.empty
    @Published var controlsVisible = true
    /// The panel open at the top right, if any. One at a time.
    enum Panel { case none, signal, settings, queue }
    @Published var panel = Panel.none

    func toggle(_ wanted: Panel) {
        withAnimation(.smooth(duration: 0.4)) { panel = panel == wanted ? .none : wanted }
    }

    /// Change a preference ("subtitleScale", "1.2"). The player keeps it and puts it into effect.
    func set(_ key: String, _ value: String) {
        send("set \(key) \(value)")
    }
    @Published var dropTargeted = false
    /// A seek the user asked for that the engine has not caught up with yet.
    @Published var seekTarget: Double?
    @Published var showsRemaining = false

    /// Sends a command to the engine ("toggle-pause", "seek-to 12.5", ...).
    var send: (String) -> Void = { _ in }
    /// Called when the controls appear or disappear, to fade the window's own buttons with them.
    var onControlsVisibility: (Bool) -> Void = { _ in }

    /// The pieces of glass the pointer is over. While there is one, the controls stay.
    private var hovered: Set<String> = []
    private var hovering: Bool { !hovered.isEmpty }

    func hover(_ piece: String, _ inside: Bool) {
        if inside {
            hovered.insert(piece)
        } else {
            hovered.remove(piece)
            if hovered.isEmpty { scheduleHide() }
        }
    }
    /// Keep the controls up regardless (for screenshots and tests).
    var pinned = false

    private var hideTimer: Timer?
    private var seekTimer: Timer?

    /// Where the seek bar should stand: the requested position until playback gets there.
    var shownPosition: Double { seekTarget ?? status.position }

    /// The window is small, in a corner, above everything: the player as a picture in a picture.
    @Published var mini = false
    /// The window stays above other windows.
    @Published var onTop = false

    /// A line said for a moment above the controls: what a key just changed.
    @Published var note: String?
    private var noteTimer: Timer?

    private func say(_ text: String) {
        withAnimation(.smooth(duration: 0.25)) { note = text }
        noteTimer?.invalidate()
        noteTimer = Timer.scheduledTimer(withTimeInterval: 1.6, repeats: false) { [weak self] _ in
            Task { @MainActor in withAnimation(.smooth(duration: 0.4)) { self?.note = nil } }
        }
    }

    /// Small pictures over the seek bar, by their file, once read.
    private var thumbnails: [String: NSImage] = [:]
    private var thumbnailAsked: (at: Double, when: Date) = (-1, .distantPast)

    /// The pointer is over the seek bar at `seconds` (nil: it left). A small picture of that
    /// moment is asked for, a few times a second at most and only once it has moved on.
    func pointing(at seconds: Double?) {
        guard let seconds, status.duration > 0 else { return }
        let step = max(status.duration / 300, 1)
        if abs(seconds - thumbnailAsked.at) < step / 2 || Date().timeIntervalSince(thumbnailAsked.when) < 0.08 {
            return
        }
        thumbnailAsked = (seconds, Date())
        send("thumbnail \(seconds)")
    }

    /// The latest small picture made, if there is one yet.
    var thumbnail: NSImage? {
        guard let path = status.thumbnail?.path else { return nil }
        if let image = thumbnails[path] { return image }
        guard let image = NSImage(contentsOfFile: path) else { return nil }
        if thumbnails.count > 400 { thumbnails.removeAll() }
        thumbnails[path] = image
        return image
    }

    func update(_ new: Status) {
        if new.file != status.file { thumbnails.removeAll(); thumbnailAsked = (-1, .distantPast) }
        // The system's own list of recent files (the Dock's menu) hears of each one opened.
        if let path = new.path, path != status.path {
            NSDocumentController.shared.noteNewRecentDocumentURL(URL(fileURLWithPath: path))
        }
        if let target = seekTarget, abs(new.position - target) < 1.0 { seekTarget = nil }
        // Changed while the same file plays: by a key, a menu, the system's controls.
        if new.file != nil, new.file == status.file {
            if new.speed != status.speed { say(speedNote(new.speed)) }
            if new.subtitleDelay != status.subtitleDelay { say(subtitleDelayNote(new.subtitleDelay)) }
            if new.settings.nightMode != status.settings.nightMode {
                say(new.settings.nightMode ? L("Night sound on", "Ночной звук включён") : L("Night sound off", "Ночной звук выключен"))
            }
            if new.savedFrames != status.savedFrames {
                say(new.savedFrameError == nil
                    ? L("Frame saved to Pictures", "Кадр сохранён в «Изображения»")
                    : L("The frame could not be saved", "Кадр не сохранился"))
            }
        }
        let wasPlaying = isPlaying
        if new != status { status = new }
        if status.file == nil {
            if panel == .signal || panel == .queue { panel = .none }
            hovered.removeAll()
        }
        if isPlaying != wasPlaying { isPlaying ? scheduleHide() : reveal() }
        keepAwake(isPlaying)
    }

    var isPlaying: Bool { status.file != nil && !status.paused && !status.ended }

    /// Held while a film plays: nobody touches the keyboard for two hours, and without this
    /// the display dims and the Mac goes to sleep in the middle of it. Let go on pause and at
    /// the end, so that a paused film does not keep the screen on all night.
    private var awake: NSObjectProtocol?

    private func keepAwake(_ wanted: Bool) {
        if wanted, awake == nil {
            awake = ProcessInfo.processInfo.beginActivity(
                options: [.idleDisplaySleepDisabled, .idleSystemSleepDisabled],
                reason: "Playing a film")
        } else if !wanted, let held = awake {
            ProcessInfo.processInfo.endActivity(held)
            awake = nil
        }
    }

    func seek(to seconds: Double) {
        let target = max(0, min(seconds, status.duration))
        seekTarget = target
        send("seek-to \(target)")
        seekTimer?.invalidate()
        seekTimer = Timer.scheduledTimer(withTimeInterval: 2.5, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.seekTarget = nil }
        }
    }

    /// Step to the next subtitle track, ending with "off"; and the same for sound, without the off.
    func cycleSubtitles() {
        let ids = status.subtitleTracks.map(\.id)
        guard !ids.isEmpty else { return }
        if let current = status.subtitleTrack, let at = ids.firstIndex(of: current) {
            send(at + 1 < ids.count ? "subtitle-track \(ids[at + 1])" : "subtitle-track off")
        } else {
            send("subtitle-track \(ids[0])")
        }
    }

    /// Move a file of the queue. Shown at once, without waiting for the player to say so.
    func moveInQueue(from: Int, to: Int) {
        if var queue = status.queue, queue.items.indices.contains(from), queue.items.indices.contains(to) {
            let playing = queue.current.map { queue.items[$0] }
            queue.items.insert(queue.items.remove(at: from), at: to)
            if let playing { queue.current = queue.items.firstIndex(of: playing) }
            status.queue = queue
        }
        send("queue-move \(from) \(to)")
    }

    func cycleSound() {
        let ids = status.audioTracks.map(\.id)
        guard ids.count > 1, let current = status.audioTrack, let at = ids.firstIndex(of: current) else { return }
        send("audio-track \(ids[(at + 1) % ids.count])")
    }

    /// The pointer moved or a key was pressed: show the controls and restart the countdown.
    func reveal() {
        if !controlsVisible {
            withAnimation(.smooth(duration: 0.3)) { controlsVisible = true }
            onControlsVisibility(true)
        }
        scheduleHide()
    }

    private func scheduleHide() {
        hideTimer?.invalidate()
        hideTimer = Timer.scheduledTimer(withTimeInterval: 2.6, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.hideIfIdle() }
        }
    }

    private func hideIfIdle() {
        // While paused, or with the pointer on a control or the panel open, the controls stay.
        guard isPlaying, !hovering, panel == .none, !pinned, controlsVisible else { return }
        hovered.removeAll()
        withAnimation(.smooth(duration: 0.5)) { controlsVisible = false }
        onControlsVisibility(false)
        NSCursor.setHiddenUntilMouseMoves(true)
    }
}

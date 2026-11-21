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

    static let standard = SettingsInfo(
        displayPeak: nil, reportedPeak: 0, quality: "high", hardwareDecoding: true, dolbyVision: true,
        subtitleScale: 1,
        subtitleNits: 203, subtitleLanguage: "", audioLanguage: "", audioDelay: 0)
}

struct Status: Codable, Equatable {
    /// Name of the open file; nil when nothing is open.
    var file: String?
    var error: String?
    var position: Double
    var duration: Double
    var paused: Bool
    var ended: Bool
    /// Where this file was picked up from, for the few seconds the note about it shows.
    var resumedFrom: Double?
    var volume: Double
    var muted: Bool
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
    var display: DisplayInfo
    var settings: SettingsInfo

    static let empty = Status(
        file: nil, error: nil, position: 0, duration: 0, paused: false, ended: false,
        resumedFrom: nil, volume: 1,
        muted: false, fullscreen: false, mode: "auto", video: nil, signal: nil, sound: nil,
        audioTracks: [], subtitleTracks: [], audioTrack: nil, subtitleTrack: nil,
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
    enum Panel { case none, signal, settings }
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

    func update(_ new: Status) {
        if let target = seekTarget, abs(new.position - target) < 1.0 { seekTarget = nil }
        let wasPlaying = isPlaying
        if new != status { status = new }
        if status.file == nil {
            if panel == .signal { panel = .none }
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

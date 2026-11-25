// The system's "Now Playing": what plays is shown in Control Center and on the lock screen,
// and the keys that control playback anywhere (the play key on the keyboard, a tap on the
// headphones, Control Center's own buttons) reach this player.

import AppKit
import MediaPlayer

@MainActor
final class NowPlaying {
    private let model: PlayerModel
    private var targets: [(MPRemoteCommand, Any)] = []

    /// What the system was last told, to tell it again only when that is out of date: it runs
    /// the clock between updates itself from the time and the rate it was given.
    private struct Told: Equatable {
        var file: String
        var duration: Double
        var paused: Bool
        var subtitle: String
    }
    private var told: Told?
    private var toldPosition = 0.0
    private var toldAt = Date()

    init(model: PlayerModel) {
        self.model = model
        let center = MPRemoteCommandCenter.shared()
        handle(center.togglePlayPauseCommand) { model in
            model.send(model.status.ended ? "seek-to 0" : "toggle-pause")
        }
        handle(center.playCommand) { model in
            if model.status.ended {
                model.send("seek-to 0")
            } else if model.status.paused {
                model.send("toggle-pause")
            }
        }
        handle(center.pauseCommand) { model in
            if !model.status.paused { model.send("toggle-pause") }
        }
        // In a film a track is a chapter; after the last, the next file of the series.
        handle(center.nextTrackCommand) { $0.send("skip-next") }
        handle(center.previousTrackCommand) { $0.send("skip-previous") }
        let seek = center.changePlaybackPositionCommand
        targets.append((seek, seek.addTarget { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else { return .commandFailed }
            let seconds = event.positionTime
            DispatchQueue.main.async { self?.model.seek(to: seconds) }
            return .success
        }))
    }

    private func handle(_ command: MPRemoteCommand, _ action: @escaping @MainActor (PlayerModel) -> Void) {
        targets.append((command, command.addTarget { [weak self] _ in
            DispatchQueue.main.async {
                guard let self, self.model.status.file != nil else { return }
                action(self.model)
                self.model.reveal()
            }
            return .success
        }))
    }

    func update(_ status: Status) {
        let center = MPNowPlayingInfoCenter.default()
        guard let file = status.file, status.error == nil else {
            if told != nil {
                center.nowPlayingInfo = nil
                center.playbackState = .stopped
                told = nil
            }
            return
        }
        let paused = status.paused || status.ended
        let now = Told(file: file, duration: status.duration, paused: paused, subtitle: subtitle(status))
        // Where the system thinks playback is by now; a jump away from that is a seek.
        let expected = toldPosition + (told?.paused == false ? Date().timeIntervalSince(toldAt) : 0)
        guard now != told || abs(status.position - expected) > 1.5 else { return }

        var info: [String: Any] = [
            MPMediaItemPropertyTitle: (file as NSString).deletingPathExtension,
            MPMediaItemPropertyPlaybackDuration: status.duration,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: status.position,
            MPNowPlayingInfoPropertyPlaybackRate: paused ? 0.0 : 1.0,
            MPNowPlayingInfoPropertyDefaultPlaybackRate: 1.0,
            MPNowPlayingInfoPropertyMediaType: MPNowPlayingInfoMediaType.video.rawValue,
        ]
        if !now.subtitle.isEmpty { info[MPMediaItemPropertyArtist] = now.subtitle }
        if let icon = NSApp.applicationIconImage {
            info[MPMediaItemPropertyArtwork] = MPMediaItemArtwork(boundsSize: icon.size) { _ in icon }
        }
        center.nowPlayingInfo = info
        center.playbackState = paused ? .paused : .playing
        told = now
        toldPosition = status.position
        toldAt = Date()
    }

    /// The line under the title: the chapter playing, or else the series the file belongs to.
    private func subtitle(_ status: Status) -> String {
        if let n = status.chapter { return status.chapterName(n) }
        if let queue = status.queue, queue.items.count > 1 { return queue.name }
        return ""
    }

    func stop() {
        for (command, target) in targets { command.removeTarget(target) }
        targets.removeAll()
        MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
        MPNowPlayingInfoCenter.default().playbackState = .stopped
    }
}

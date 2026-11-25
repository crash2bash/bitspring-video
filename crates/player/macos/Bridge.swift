// The seam between the Rust player and the native interface.
//
// Rust owns the window and the picture. It calls `bsv_ui_attach` once with the window's content
// view, then `bsv_ui_update` a few times a second with the player's state as JSON. The interface
// answers through one callback carrying short text commands ("toggle-pause", "seek-to 12.5").
// Everything here runs on the main thread.

import AppKit
import SwiftUI
import UniformTypeIdentifiers

typealias CommandCallback = @convention(c) (UnsafePointer<CChar>, UnsafeMutableRawPointer?) -> Void

/// Hosts the SwiftUI controls above the video layer without taking the keyboard from the window.
final class OverlayView: NSHostingView<PlayerOverlay> {
    override var acceptsFirstResponder: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }
}

@MainActor
final class InterfaceController {
    let model = PlayerModel()
    private let view: OverlayView
    private var monitors: [Any] = []
    private var observers: [NSObjectProtocol] = []
    private let decoder = JSONDecoder()
    private var nowPlaying: NowPlaying?

    init(parent: NSView, callback: @escaping CommandCallback, user: UnsafeMutableRawPointer?) {
        view = OverlayView(rootView: PlayerOverlay(model: model))
        view.sizingOptions = []
        view.frame = parent.bounds
        view.autoresizingMask = [.width, .height]
        parent.addSubview(view)

        // A stage, not a document window: black wherever there is no picture.
        parent.window?.backgroundColor = .black
        // An empty toolbar makes the title bar tall enough for the system to centre the close,
        // minimise and zoom buttons on the same line as the top row of glass.
        if let window = parent.window {
            let toolbar = NSToolbar(identifier: "player")
            toolbar.showsBaselineSeparator = false
            window.toolbar = toolbar
            window.toolbarStyle = .unified
            window.titlebarSeparatorStyle = .none
            // In full screen the system draws that toolbar as a solid bar across the top of
            // the picture, over the top row of glass. There are no window buttons to line up
            // there, so it goes for as long as full screen lasts.
            let center = NotificationCenter.default
            observers.append(center.addObserver(
                forName: NSWindow.willEnterFullScreenNotification, object: window, queue: .main
            ) { [weak window] _ in
                MainActor.assumeIsolated { window?.toolbar?.isVisible = false }
            })
            observers.append(center.addObserver(
                forName: NSWindow.didExitFullScreenNotification, object: window, queue: .main
            ) { [weak window] _ in
                MainActor.assumeIsolated { window?.toolbar?.isVisible = true }
            })
        }

        model.send = { [weak self] command in
            switch command {
            case "open-dialog": self?.chooseFiles(adding: false)
            case "add-dialog": self?.chooseFiles(adding: true)
            case "save-dialog": self?.savePlaylist()
            case "mini": self?.toggleMini()
            case "on-top": self?.toggleOnTop()
            default: command.withCString { callback($0, user) }
            }
        }
        model.onControlsVisibility = { [weak self] visible in
            self?.showWindowButtons(visible)
            // Subtitles move up out of the controls' way, and back down when they go.
            (visible ? "controls 1" : "controls 0").withCString { callback($0, user) }
        }

        nowPlaying = NowPlaying(model: model)

        // A file may already be waiting: the one whose double-click launched the app.
        Arrivals.controller = self
        let waiting = Arrivals.waiting
        Arrivals.waiting = []
        if !waiting.isEmpty {
            DispatchQueue.main.async { Arrivals.open(waiting) }
        }

        // Keys are read here whatever has the focus, so the picture never needs to be clicked first.
        monitors.append(NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, event.window === self.view.window else { return event }
            return self.handleKey(event) ? nil : event
        } as Any)
        monitors.append(NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved, .leftMouseDown, .scrollWheel]) { [weak self] event in
            if let self, event.window === self.view.window { self.model.reveal() }
            return event
        } as Any)
    }

    func update(json: String) {
        guard let data = json.data(using: .utf8) else { return }
        do {
            model.update(try decoder.decode(Status.self, from: data))
            nowPlaying?.update(model.status)
            syncWindowButtons()
        } catch {
            NSLog("BitSpring Video: unreadable state from the player: \(error)")
        }
    }

    /// Interface-only actions, for scripted runs and screenshots.
    func perform(_ action: String) {
        switch action {
        case "panel":
            model.toggle(.signal)
            model.reveal()
        case "settings":
            model.toggle(.settings)
            model.reveal()
        case "queue":
            model.toggle(.queue)
            model.reveal()
        case "pin":
            model.pinned = true
            model.reveal()
        case "mini":
            toggleMini()
        case "on-top":
            toggleOnTop()
        default:
            break
        }
    }

    func detach() {
        nowPlaying?.stop()
        nowPlaying = nil
        monitors.forEach(NSEvent.removeMonitor)
        monitors.removeAll()
        observers.forEach(NotificationCenter.default.removeObserver)
        observers.removeAll()
        view.removeFromSuperview()
    }

    private func handleKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if flags.contains(.command) {
            if event.charactersIgnoringModifiers == "o" {
                chooseFiles(adding: false)
                return true
            }
            if event.keyCode == 35, flags.contains(.control) { // ⌃⌘P, as other players have it
                toggleMini()
                return true
            }
            if event.keyCode == 1 { // S
                model.send("save-frame")
                return true
            }
            if event.charactersIgnoringModifiers == "," {
                model.reveal()
                model.toggle(.settings)
                return true
            }
            return false // the system's own shortcuts: quit, close, minimise
        }
        model.reveal()
        // Option with an arrow, as it jumps a word in text, jumps a chapter here.
        if flags.contains(.option), event.keyCode == 123 || event.keyCode == 124 {
            model.send(event.keyCode == 124 ? "chapter-next" : "chapter-previous")
            return true
        }
        let step = flags.contains(.shift) ? 60 : 5
        // By the key's place, not its letter: the same keys whatever the keyboard's language.
        let shift = flags.contains(.shift)
        switch event.keyCode {
        case 43: model.send("step -1"); return true              // ,
        case 47: model.send("step 1"); return true               // .
        case 33: model.send("speed-by -0.25"); return true       // [
        case 30: model.send("speed-by 0.25"); return true        // ]
        case 51: model.send("speed 1"); return true              // delete
        case 6: model.send("subtitle-delay-by \(shift ? -1 : -0.1)"); return true   // Z
        case 7: model.send("subtitle-delay-by \(shift ? 1 : 0.1)"); return true     // X
        default: break
        }
        switch event.keyCode {
        case 49: model.send("toggle-pause")                 // space
        case 121: model.send("chapter-next")                // page down
        case 116: model.send("chapter-previous")            // page up
        case 123: model.send("seek-by -\(step)")            // left
        case 124: model.send("seek-by \(step)")             // right
        case 126: model.send("volume-by 0.05")              // up
        case 125: model.send("volume-by -0.05")             // down
        case 36, 76: model.send("toggle-fullscreen")        // return, enter
        case 53:                                            // escape
            if model.panel != .none {
                model.toggle(model.panel)
            } else {
                model.send("exit-fullscreen")
            }
        default:
            switch event.charactersIgnoringModifiers?.lowercased() {
            case "f": model.send("toggle-fullscreen")
            case "m": model.send("toggle-mute")
            case "s": model.cycleSubtitles()
            case "a": model.cycleSound()
            case "n": model.send("next")
            case "p": model.send("previous")
            case "l":
                guard model.status.queue != nil else { return false }
                model.toggle(.queue)
            case "i":
                guard model.status.video != nil else { return false }
                model.toggle(.signal)
            default: return false
            }
        }
        return true
    }

    /// Choose what to play, or what to put at the end of the list: videos, a folder of
    /// them, a playlist, or a subtitle file for the video that is playing.
    private func chooseFiles(adding: Bool) {
        guard let window = view.window else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.message = adding
            ? L("Choose what to add to the list", "Выберите, что добавить в список")
            : L("Choose a video, several, or a folder of them", "Выберите видео, несколько или папку с ними")
        panel.prompt = adding ? L("Add", "Добавить") : L("Play", "Играть")
        panel.beginSheetModal(for: window) { [weak self] response in
            let paths = panel.urls.map(\.path)
            guard response == .OK, !paths.isEmpty else { return }
            self?.model.send((adding ? "queue-add " : "open ") + paths.joined(separator: "\n"))
        }
    }

    /// Write the list to a playlist file: plain text, one path to a line.
    private func savePlaylist() {
        guard let window = view.window, let queue = model.status.queue else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "m3u8") ?? .plainText]
        panel.nameFieldStringValue = (queue.name.isEmpty ? L("Playlist", "Плейлист") : queue.name) + ".m3u8"
        panel.message = L(
            "The playlist is a text file that names the videos. They stay where they are.",
            "Плейлист — текстовый файл с именами видео. Сами видео остаются на месте.")
        panel.prompt = L("Save", "Сохранить")
        panel.beginSheetModal(for: window) { [weak self] response in
            guard response == .OK, let url = panel.url else { return }
            self?.model.send("queue-save \(url.path)")
        }
    }

    // MARK: Above everything

    /// The window's place before it was made small, to go back to.
    private var bigFrame: NSRect?
    private var bigBehavior: NSWindow.CollectionBehavior = []

    /// Keep the window above other windows, or let it go back among them.
    private func toggleOnTop() {
        guard let window = view.window else { return }
        model.onTop.toggle()
        if !model.mini { window.level = model.onTop ? .floating : .normal }
    }

    /// The player made small in a corner of the screen and kept above everything, on every
    /// desktop and over other programs in full screen; and back to where it was.
    private func toggleMini() {
        guard let window = view.window else { return }
        if window.styleMask.contains(.fullScreen) {
            // Out of full screen first; made small once that is over.
            model.send("exit-fullscreen")
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in self?.toggleMini() }
            return
        }
        if model.mini, let frame = bigFrame {
            model.mini = false
            window.level = model.onTop ? .floating : .normal
            window.collectionBehavior = bigBehavior
            window.toolbar?.isVisible = true
            window.setFrame(frame, display: true, animate: true)
            bigFrame = nil
        } else if !model.mini {
            guard let screen = window.screen ?? NSScreen.main else { return }
            bigFrame = window.frame
            bigBehavior = window.collectionBehavior
            model.panel = .none
            model.mini = true
            // The window's shape, which is the picture's (it runs under the title bar), and a
            // quarter of the screen's width at most.
            let whole = window.frame.size
            let shape = whole.height > 0 ? whole.width / whole.height : 16.0 / 9
            let width = min(480, screen.visibleFrame.width / 4)
            let size = NSSize(width: width, height: (width / shape).rounded())
            let area = screen.visibleFrame
            let frame = NSRect(x: area.maxX - size.width - 20, y: area.minY + 20, width: size.width, height: size.height)
            window.toolbar?.isVisible = false
            window.level = .floating
            window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            window.setFrame(frame, display: true, animate: true)
        }
    }

    /// The close, minimise and zoom buttons come and go with the rest of the controls.
    private func showWindowButtons(_ visible: Bool) {
        guard let window = view.window, !window.styleMask.contains(.fullScreen) else { return }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = visible ? 0.3 : 0.5
            for kind in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
                window.standardWindowButton(kind)?.animator().alphaValue = visible ? 1 : 0
            }
        }
    }

    /// The system redraws the title bar on its own schedule and brings the buttons back; put
    /// them back the way the controls are.
    private func syncWindowButtons() {
        guard let window = view.window, !window.styleMask.contains(.fullScreen) else { return }
        let wanted: CGFloat = model.controlsVisible || model.status.file == nil ? 1 : 0
        for kind in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            if let button = window.standardWindowButton(kind), button.alphaValue != wanted, button.layer?.animationKeys()?.isEmpty ?? true {
                button.alphaValue = wanted
            }
        }
    }
}

// MARK: - Files opened from outside

/// Files the system asks the app to open: a double-click in Finder, "Open With", a drop on
/// the Dock icon. When that is what launches the app, the request arrives before there is an
/// interface to act on it, so it waits here.
@MainActor
enum Arrivals {
    static var waiting: [String] = []
    static weak var controller: InterfaceController?

    static func open(_ paths: [String]) {
        guard !paths.isEmpty else { return }
        if let controller {
            controller.model.send("open " + paths.joined(separator: "\n"))
        } else {
            waiting = paths
        }
    }
}

final class OpenDocumentsHandler: NSObject {
    static let shared = OpenDocumentsHandler()

    @objc func handle(_ event: NSAppleEventDescriptor, withReply reply: NSAppleEventDescriptor) {
        guard let direct = event.paramDescriptor(forKeyword: keyDirectObject) else { return }
        var paths: [String] = []
        if direct.numberOfItems > 0 {
            for index in 1...direct.numberOfItems {
                if let url = direct.atIndex(index)?.fileURLValue { paths.append(url.path) }
            }
        } else if let url = direct.fileURLValue {
            paths.append(url.path)
        }
        MainActor.assumeIsolated { Arrivals.open(paths) }
    }
}

/// Called once, before the application starts running. The handler has to go in while the
/// application is finishing its launch: earlier and AppKit replaces it with its own, later
/// and the request that launched the app has already gone by.
@_cdecl("bsv_app_prepare")
public func bsvAppPrepare() {
    NotificationCenter.default.addObserver(
        forName: NSApplication.willFinishLaunchingNotification, object: nil, queue: nil
    ) { _ in
        NSAppleEventManager.shared().setEventHandler(
            OpenDocumentsHandler.shared,
            andSelector: #selector(OpenDocumentsHandler.handle(_:withReply:)),
            forEventClass: AEEventClass(kCoreEventClass),
            andEventID: AEEventID(kAEOpenDocuments))
    }
}

// MARK: - C interface

// Raw pointers cross into the main actor as plain addresses: the compiler cannot know that
// Rust only ever calls these functions on the main thread.

@_cdecl("bsv_ui_attach")
public func bsvUIAttach(
    _ parent: UnsafeMutableRawPointer,
    _ callback: @escaping @convention(c) (UnsafePointer<CChar>, UnsafeMutableRawPointer?) -> Void,
    _ user: UnsafeMutableRawPointer?
) -> UnsafeMutableRawPointer {
    let parentAddress = UInt(bitPattern: parent)
    let userAddress = UInt(bitPattern: user)
    let address: UInt = MainActor.assumeIsolated {
        let view = Unmanaged<NSView>.fromOpaque(UnsafeRawPointer(bitPattern: parentAddress)!).takeUnretainedValue()
        let controller = InterfaceController(
            parent: view, callback: callback, user: UnsafeMutableRawPointer(bitPattern: userAddress))
        return UInt(bitPattern: Unmanaged.passRetained(controller).toOpaque())
    }
    return UnsafeMutableRawPointer(bitPattern: address)!
}

private func controller(at address: UInt) -> Unmanaged<InterfaceController> {
    Unmanaged<InterfaceController>.fromOpaque(UnsafeRawPointer(bitPattern: address)!)
}

@_cdecl("bsv_ui_update")
public func bsvUIUpdate(_ ui: UnsafeMutableRawPointer, _ json: UnsafePointer<CChar>) {
    let text = String(cString: json)
    let address = UInt(bitPattern: ui)
    MainActor.assumeIsolated { controller(at: address).takeUnretainedValue().update(json: text) }
}

@_cdecl("bsv_ui_perform")
public func bsvUIPerform(_ ui: UnsafeMutableRawPointer, _ action: UnsafePointer<CChar>) {
    let text = String(cString: action)
    let address = UInt(bitPattern: ui)
    MainActor.assumeIsolated { controller(at: address).takeUnretainedValue().perform(text) }
}

@_cdecl("bsv_ui_detach")
public func bsvUIDetach(_ ui: UnsafeMutableRawPointer) {
    let address = UInt(bitPattern: ui)
    MainActor.assumeIsolated { controller(at: address).takeRetainedValue().detach() }
}

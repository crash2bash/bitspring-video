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
    private let decoder = JSONDecoder()

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
        }

        model.send = { [weak self] command in
            if command == "open-dialog" {
                self?.chooseFile()
            } else {
                command.withCString { callback($0, user) }
            }
        }
        model.onControlsVisibility = { [weak self] visible in
            self?.showWindowButtons(visible)
            // Subtitles move up out of the controls' way, and back down when they go.
            (visible ? "controls 1" : "controls 0").withCString { callback($0, user) }
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
        case "pin":
            model.pinned = true
            model.reveal()
        default:
            break
        }
    }

    func detach() {
        monitors.forEach(NSEvent.removeMonitor)
        monitors.removeAll()
        view.removeFromSuperview()
    }

    private func handleKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if flags.contains(.command) {
            if event.charactersIgnoringModifiers == "o" {
                chooseFile()
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
        let step = flags.contains(.shift) ? 60 : 5
        switch event.keyCode {
        case 49: model.send("toggle-pause")                 // space
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
            case "i":
                guard model.status.video != nil else { return false }
                model.toggle(.signal)
            default: return false
            }
        }
        return true
    }

    private func chooseFile() {
        guard let window = view.window else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.message = L("Choose a video to play", "Выберите видео")
        panel.prompt = L("Play", "Играть")
        panel.beginSheetModal(for: window) { [weak self] response in
            guard response == .OK, let url = panel.url else { return }
            self?.model.send("open \(url.path)")
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

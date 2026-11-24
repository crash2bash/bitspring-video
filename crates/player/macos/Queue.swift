// What plays after what: the lens beside the file's name, and the panel it opens into.
//
// The left of the top row says what is playing and where it stands among the others ("2 / 10");
// the right says how it is being shown. The panel lists the folder's videos or the playlist,
// says in a sentence what happens when this file ends, and is where a playlist is made: files
// are dropped or added, dragged into order, taken out, and the list saved.

import AppKit
import SwiftUI

extension QueueInfo {
    var hasNext: Bool { (current ?? items.count) + 1 < items.count }
    var hasPrevious: Bool { (current ?? 0) > 0 }
    var isPlaylist: Bool { kind == "playlist" }

    /// The folder's name or the playlist's, or what to call a list nobody has named yet.
    var title: String {
        if !name.isEmpty { return name }
        return isPlaylist ? L("New playlist", "Новый плейлист") : L("Folder", "Папка")
    }
}

func videos(_ count: Int) -> String {
    L(count == 1 ? "1 video" : "\(count) videos", "\(count) видео")
}

/// The lens, or the panel it opens into.
struct QueueCorner: View {
    @ObservedObject var model: PlayerModel
    let glass: Namespace.ID
    /// The room there is for the list in this window.
    let listHeight: CGFloat

    var body: some View {
        if let queue = model.status.queue {
            if model.panel == .queue {
                QueuePanel(model: model, queue: queue, listHeight: listHeight)
                    .glassEffect(playerGlass(), in: .rect(cornerRadius: 24, style: .continuous))
                    .glassEffectID("queue", in: glass)
            } else {
                Button {
                    model.toggle(.queue)
                } label: {
                    Group {
                        if queue.items.count > 1, let current = queue.current {
                            Text("\(current + 1) / \(queue.items.count)")
                                .font(.figures(14))
                                .padding(.horizontal, 14)
                        } else {
                            Image(systemName: "list.bullet")
                                .font(.system(size: 13, weight: .medium))
                                .frame(width: Layout.topHeight)
                        }
                    }
                    .frame(height: Layout.topHeight)
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .fixedSize()
                .glassEffect(playerGlass(interactive: true), in: .capsule)
                .glassEffectID("queue", in: glass)
                .help(L("What plays next", "Что играет дальше"))
                .accessibilityLabel(L("What plays next", "Что играет дальше"))
            }
        }
    }
}

private struct QueuePanel: View {
    @ObservedObject var model: PlayerModel
    let queue: QueueInfo
    let listHeight: CGFloat

    private let rowHeight: CGFloat = 30
    /// The row being dragged to another place, and how far it has been taken.
    @State private var dragged: Int?
    @State private var travel: CGFloat = 0
    @State private var hovered: Int?
    @State private var dropTargeted = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Button {
                model.toggle(.queue)
            } label: {
                HStack(spacing: 8) {
                    Text(queue.title)
                        .font(.system(size: 15, weight: .semibold))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 8)
                    Image(systemName: "chevron.up")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Color.quiet)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Text(explanation)
                .font(.system(size: 13))
                .foregroundStyle(Color.quiet)
                .fixedSize(horizontal: false, vertical: true)

            list

            if queue.items.count > 1 {
                VStack(alignment: .leading, spacing: 6) {
                    Text(L("When this one ends", "Когда это видео закончится"))
                        .font(.system(size: 12))
                        .foregroundStyle(Color.quiet)
                    Choice(
                        options: [
                            ("1", L("Play the next", "Играть следующее")),
                            ("0", L("Stop", "Остановиться")),
                        ],
                        selected: model.status.settings.autoNext ? "1" : "0",
                        onSelect: { model.set("autoNext", $0) })
                }
            }

            HStack(spacing: 8) {
                action(L("Add…", "Добавить…")) { model.send("add-dialog") }
                action(queue.isPlaylist && !queue.name.isEmpty
                    ? L("Save as…", "Сохранить как…")
                    : L("Save as a playlist…", "Сохранить как плейлист…")
                ) { model.send("save-dialog") }
                Spacer(minLength: 0)
            }
        }
        .padding(16)
        .frame(width: 372, alignment: .leading)
        .overlay {
            if dropTargeted {
                RoundedRectangle(cornerRadius: 20, style: .continuous)
                    .strokeBorder(Color.porcelain.opacity(0.8), style: StrokeStyle(lineWidth: 2, dash: [8, 6]))
                    .padding(5)
                    .allowsHitTesting(false)
            }
        }
        // Dropped on the list, files join it; dropped on the picture, they replace it.
        .onDrop(of: [.fileURL], isTargeted: $dropTargeted) { providers in
            droppedPaths(providers) { model.send("queue-add " + $0.joined(separator: "\n")) }
            return true
        }
    }

    /// One or two sentences: what this list is, and what happens when the file ends.
    private var explanation: String {
        let count = videos(queue.items.count)
        let what = queue.isPlaylist
            ? (queue.unsaved
                ? L("A playlist of \(count), changed since it was saved.", "Плейлист, \(count), изменён после сохранения.")
                : queue.name.isEmpty
                    ? L("A playlist of \(count), not saved yet.", "Плейлист, \(count), пока не сохранён.")
                    : L("A playlist of \(count).", "Плейлист, \(count)."))
            : L("A folder with \(count).", "Папка, \(count).")
        guard queue.items.count > 1 else {
            return queue.isPlaylist
                ? what
                : L("The only video in its folder. Add others to make a playlist.",
                    "Единственное видео в папке. Добавьте другие, чтобы собрать плейлист.")
        }
        let then: String
        if !queue.hasNext {
            then = L("This is the last one.", "Это последнее.")
        } else if !model.status.settings.autoNext {
            then = L("The player stops after each one.", "После каждого плеер останавливается.")
        } else if queue.continues {
            then = L("The next one starts by itself.", "Следующее начнётся само.")
        } else {
            then = L(
                "The next one is not a continuation of this one by its name, so the player stops after it.",
                "Следующее по названию не продолжает это, поэтому после него плеер остановится.")
        }
        return what + " " + then
    }

    private var list: some View {
        ScrollViewReader { reader in
            ScrollView(.vertical) {
                VStack(spacing: 0) {
                    ForEach(Array(queue.items.enumerated()), id: \.offset) { index, item in
                        row(index, item)
                            .id(index)
                    }
                }
            }
            .scrollIndicators(.never)
            // Half a row showing at the bottom says there is more below.
            .frame(height: min(CGFloat(queue.items.count) * rowHeight, max(2.5 * rowHeight, min(listHeight, 8.5 * rowHeight))))
            .onAppear {
                if let current = queue.current { reader.scrollTo(current, anchor: .center) }
            }
            .onChange(of: queue.current) { _, current in
                if let current { withAnimation(.smooth(duration: 0.3)) { reader.scrollTo(current, anchor: .center) } }
            }
        }
        .padding(.horizontal, -8)
    }

    /// Where the dragged row would land if it were let go now.
    private var landing: Int? {
        guard let dragged else { return nil }
        let moved = Int((travel / rowHeight).rounded())
        return min(max(dragged + moved, 0), queue.items.count - 1)
    }

    private func offset(of index: Int) -> CGFloat {
        guard let dragged, let landing else { return 0 }
        if index == dragged { return travel }
        if dragged < index && index <= landing { return -rowHeight }
        if landing <= index && index < dragged { return rowHeight }
        return 0
    }

    private func row(_ index: Int, _ item: QueueItem) -> some View {
        let current = index == queue.current
        let lifted = index == dragged
        return HStack(spacing: 8) {
            Image(systemName: "play.fill")
                .font(.system(size: 9, weight: .bold))
                .opacity(current ? 1 : 0)
                .frame(width: 12)
            Text(item.label)
                .font(.system(size: 13, weight: current ? .semibold : .regular))
                .foregroundStyle(current ? Color.porcelain : Color.quiet)
                .lineLimit(1)
                .truncationMode(.tail)
                .accessibilityLabel(item.name)
                .accessibilityAddTraits(current ? [.isButton, .isSelected] : .isButton)
                .accessibilityAction { if !current { model.send("queue-play \(index)") } }
            Spacer(minLength: 4)
            if hovered == index, !current, dragged == nil {
                Button {
                    model.send("queue-remove \(index)")
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(Color.quiet)
                        .frame(width: 22, height: 22)
                        .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .help(L("Take out of the list", "Убрать из списка"))
            }
        }
        .padding(.horizontal, 8)
        .frame(height: rowHeight)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(Color.black.opacity(lifted ? 0.4 : (current || hovered == index ? 0.22 : 0))))
        .contentShape(Rectangle())
        .offset(y: offset(of: index))
        .zIndex(lifted ? 1 : 0)
        .animation(lifted ? nil : .smooth(duration: 0.2), value: landing)
        .onHover { inside in
            if inside { hovered = index } else if hovered == index { hovered = nil }
        }
        .help(item.name)
        .onTapGesture {
            if !current { model.send("queue-play \(index)") }
        }
        .gesture(
            DragGesture(minimumDistance: 5)
                .onChanged { value in
                    dragged = index
                    travel = value.translation.height
                }
                .onEnded { _ in
                    if let landing, landing != index {
                        model.moveInQueue(from: index, to: landing)
                    }
                    dragged = nil
                    travel = 0
                }
        )
    }

    private func action(_ title: String, _ perform: @escaping () -> Void) -> some View {
        Button(action: perform) {
            Text(title)
                .font(.system(size: 12, weight: .medium))
                .padding(.horizontal, 12)
                .frame(height: 26)
                .background(Capsule().fill(Color.black.opacity(0.22)))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
    }
}

/// The paths of everything in a drop, in the order they were dropped.
@MainActor
func droppedPaths(_ providers: [NSItemProvider], then: @escaping @MainActor ([String]) -> Void) {
    let group = DispatchGroup()
    let found = Found(count: providers.count)
    for (index, provider) in providers.enumerated() {
        group.enter()
        _ = provider.loadObject(ofClass: URL.self) { url, _ in
            if let url { found.put(url.path, at: index) }
            group.leave()
        }
    }
    group.notify(queue: .main) {
        let paths = found.all()
        MainActor.assumeIsolated {
            if !paths.isEmpty { then(paths) }
        }
    }
}

/// Paths arriving from several threads, each into its own place.
private final class Found: @unchecked Sendable {
    private let lock = NSLock()
    private var paths: [String?]

    init(count: Int) { paths = Array(repeating: nil, count: count) }

    func put(_ path: String, at index: Int) {
        lock.lock()
        paths[index] = path
        lock.unlock()
    }

    func all() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return paths.compactMap { $0 }
    }
}

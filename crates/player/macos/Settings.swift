// Settings: a glass panel in the player's own window, so that every change is seen on the
// picture as it is made. It opens from the gear at the top right, or with Cmd+comma.
//
// The limits of the sliders are the same as in `prefs.rs`, which enforces them.

import SwiftUI

/// The gear, or the panel it opens into.
struct SettingsCorner: View {
    @ObservedObject var model: PlayerModel
    let glass: Namespace.ID

    var body: some View {
        if model.panel == .settings {
            SettingsPanel(model: model)
                // A click between the panel's own controls stays in the panel: without this
                // it would reach the picture behind and close the settings.
                .contentShape(Rectangle())
                .onTapGesture {}
                .glassEffect(playerGlass(), in: .rect(cornerRadius: 24, style: .continuous))
                .glassEffectID("settings", in: glass)
        } else {
            Button {
                model.toggle(.settings)
            } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 14, weight: .medium))
                    .frame(width: Layout.topHeight, height: Layout.topHeight)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .glassEffect(playerGlass(interactive: true), in: .circle)
            .glassEffectID("settings", in: glass)
            .help(L("Settings", "Настройки"))
        }
    }
}

private struct SettingsPanel: View {
    @ObservedObject var model: PlayerModel

    private var settings: SettingsInfo { model.status.settings }

    var body: some View {
        // In a short window the panel scrolls instead of running under the transport bar.
        ViewThatFits(in: .vertical) {
            content
            ScrollView(.vertical) { content }
                .scrollIndicators(.never)
        }
        .frame(width: 372)
        // Outputs come and go (headphones, a receiver switched on): looked at each time.
        .onAppear { model.send("sound-devices") }
    }

    /// The first choice of the output menu: whatever the system uses, named.
    private var systemChoice: String {
        let base = L("As the system", "Как в системе")
        guard let name = settings.systemDevice else { return base }
        return "\(base) (\(name))"
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 18) {
            Button {
                model.toggle(.settings)
            } label: {
                HStack {
                    Text(L("Settings", "Настройки"))
                        .font(.system(size: 15, weight: .semibold))
                    Spacer()
                    Image(systemName: "chevron.up")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Color.quiet)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            group(L("Picture", "Картинка")) {
                displayPeak
                row(L("Rendering", "Обработка")) {
                    Choice(
                        options: [
                            ("high", L("Best", "Лучшая")),
                            ("balanced", L("Balanced", "Средняя")),
                            ("fast", L("Fast", "Быстрая")),
                        ],
                        selected: settings.quality,
                        onSelect: { model.set("quality", $0) })
                        .frame(width: 236)
                }
                row(L("Upscaling", "Апскейлинг")) {
                    Choice(
                        options: [
                            ("off", L("Off", "Выкл")),
                            ("faithful", L("Faithful", "Точный")),
                            ("restoring", L("Restoring", "С очисткой")),
                        ],
                        selected: settings.upscaling,
                        onSelect: { model.set("upscaling", $0) })
                        .frame(width: 236)
                }
                Text(upscalingNote)
                    .font(.system(size: 11))
                    .foregroundStyle(Color.quiet)
                    .fixedSize(horizontal: false, vertical: true)
                if !settings.interpolations.isEmpty {
                    row(L("Motion", "Плавность")) {
                        Choice(
                            options: [("off", L("Off", "Выкл"))]
                                + settings.interpolations.map {
                                    ($0, $0 == "fast" ? L("Fast", "Быстрая") : L("Neural", "Нейросеть"))
                                },
                            selected: settings.interpolation,
                            onSelect: { model.set("interpolation", $0) })
                            .frame(width: 236)
                    }
                    Text(motionNote)
                        .font(.system(size: 11))
                        .foregroundStyle(Color.quiet)
                        .fixedSize(horizontal: false, vertical: true)
                }
                row(
                    L("The Mac's video decoder", "Аппаратный декодер"),
                    note: L("From the next file", "Со следующего файла")
                ) {
                    Toggle("", isOn: Binding(
                        get: { settings.hardwareDecoding },
                        set: { model.set("hardwareDecoding", $0 ? "1" : "0") }))
                        .toggleStyle(.switch)
                        .controlSize(.small)
                        .labelsHidden()
                        .tint(Color.porcelain.opacity(0.85))
                }
            }

            group(L("Subtitles", "Субтитры")) {
                slider(
                    L("Size", "Размер"),
                    value: settings.subtitleScale, in: 0.5...2, step: 0.05,
                    text: { "\(Int(($0 * 100).rounded()))%" },
                    send: { model.set("subtitleScale", String(format: "%.2f", $0)) })
                slider(
                    L("Brightness in HDR", "Яркость в HDR"),
                    value: settings.subtitleNits, in: 60...400, step: 1,
                    text: { "\(nits($0)) \(nitsUnit)" },
                    send: { model.set("subtitleNits", String(format: "%.0f", $0)) })
            }

            group(L("Sound", "Звук")) {
                row(L("Output", "Вывод")) {
                    PickMenu(
                        options: [("", systemChoice)] + settings.soundDevices.map { ($0.id, $0.name) }
                            + (settings.soundDeviceMissing
                                ? [(settings.soundDevice, settings.soundDeviceName)] : []),
                        selected: settings.soundDevice,
                        title: settings.soundDevice.isEmpty
                            ? L("As the system", "Как в системе")
                            : settings.soundDeviceName,
                        onSelect: { model.set("soundDevice", $0) })
                }
                if settings.soundDeviceMissing {
                    Text(L(
                        "\(settings.soundDeviceName) is not connected: sound plays through \(settings.systemDevice ?? "the system's output").",
                        "Устройство «\(settings.soundDeviceName)» не подключено: звук идёт через \(settings.systemDevice ?? "системный вывод")."))
                        .font(.system(size: 11))
                        .foregroundStyle(Color.quiet)
                        .fixedSize(horizontal: false, vertical: true)
                }
                row(L("Night sound", "Ночной звук")) {
                    Toggle("", isOn: Binding(
                        get: { settings.nightMode },
                        set: { model.set("nightMode", $0 ? "1" : "0") }))
                        .toggleStyle(.switch)
                        .controlSize(.small)
                        .labelsHidden()
                        .tint(Color.porcelain.opacity(0.85))
                }
                Text(L(
                    "Quiet speech raised and loud passages brought down, for late evenings and thin walls.",
                    "Тихая речь громче, громкие сцены тише: для позднего вечера и тонких стен."))
                    .font(.system(size: 11))
                    .foregroundStyle(Color.quiet)
                    .fixedSize(horizontal: false, vertical: true)
                slider(
                    L("Picture delay", "Задержка картинки"),
                    value: settings.audioDelay, in: -200...500, step: 10,
                    text: { "\(Int($0.rounded())) " + L("ms", "мс") },
                    send: { model.set("audioDelay", String(format: "%.0f", $0)) })
                Text(L(
                    "For a TV or receiver that delays sound.",
                    "Для телевизора или ресивера, который задерживает звук."))
                    .font(.system(size: 11))
                    .foregroundStyle(Color.quiet)
            }

            // Rules for files yet to be opened, kept apart from what acts on the one playing:
            // beside the size and the delay they read as a choice for the film on screen.
            group(L("When a file opens", "При открытии файла")) {
                row(L("Sound track", "Звуковая дорожка")) {
                    LanguageMenu(
                        none: L("As the file prefers", "Как в файле"),
                        selected: settings.audioLanguage,
                        onSelect: { model.set("audioLanguage", $0) })
                }
                row(L("Subtitles", "Субтитры")) {
                    LanguageMenu(
                        none: L("Only when forced", "Только форсированные"),
                        selected: settings.subtitleLanguage,
                        onSelect: { model.set("subtitleLanguage", $0) })
                }
                Text(L(
                    "Which tracks a file starts with. While it plays, change them from the bar below.",
                    "С какими дорожками файл начинается. Во время просмотра они меняются на панели внизу."))
                    .font(.system(size: 11))
                    .foregroundStyle(Color.quiet)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(16)
    }

    // MARK: What the chosen preset does

    /// One or two sentences for the preset that is on: what it does and what it is for.
    private var upscalingNote: String {
        switch settings.upscaling {
        case "faithful":
            return L(
                "Faithful: a neural network doubles a picture smaller than the window and adds nothing of its own. For good sources.",
                "Точный: нейросеть удваивает картинку, которая меньше окна, и ничего не добавляет от себя. Для хороших источников.")
        case "restoring":
            return L(
                "Restoring: doubles the picture and takes away noise and what compression left. For weak sources.",
                "С очисткой: удваивает картинку и заодно убирает шум и следы сжатия. Для слабых источников.")
        default:
            return L(
                "A neural network can double a picture before it is fitted to the screen. It works when the window is at least 1.3 times the picture's size.",
                "Нейросеть может удваивать картинку перед подгонкой под экран. Работает, когда окно больше картинки хотя бы в 1,3 раза.")
        }
    }

    private var motionNote: String {
        switch settings.interpolation {
        case "fast":
            return L(
                "Fast: macOS itself makes a frame between every two, so 25 a second become 50. For 8-bit video up to 1080p and 30 a second.",
                "Быстрая: macOS сама делает кадр между каждыми двумя, и 25 кадров в секунду становятся 50. Для 8-битного видео до 1080p и 30 кадр/с.")
        case "neural":
            return L(
                "Neural: a neural network (RIFE) makes the frames. 8-bit video of any size, but heavy: when the graphics card cannot keep up, frames stop being added and the signal panel says so.",
                "Нейросеть (RIFE) делает кадры сама. Подходит 8-битное видео любого размера, но это тяжело: если видеокарта не успевает, кадры перестают добавляться, и панель сигнала об этом скажет.")
        default:
            return L(
                "A frame can be made between every two: 25 pictures a second become 50. For videos at up to 30 a second.",
                "Между каждыми двумя кадрами можно добавлять промежуточный: 25 кадров в секунду становятся 50. Для видео до 30 кадр/с.")
        }
    }

    // MARK: The display's peak

    /// The level HDR is fitted to: the display's own figure, or the user's.
    private var displayPeak: some View {
        let low = 200.0, high = 4000.0
        let shown = settings.displayPeak ?? max(settings.reportedPeak, low)
        // Brightness is judged in ratios, so the slider moves in them too.
        let fraction = log(min(max(shown, low), high) / low) / log(high / low)
        return VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline) {
                Text(L("Display peak", "Пик дисплея"))
                    .font(.system(size: 12))
                    .foregroundStyle(Color.quiet)
                Spacer()
                Text("\(nits(shown)) \(nitsUnit)")
                    .font(.figures(13))
            }
            Track(fraction: fraction, fill: .porcelain, label: nil, height: 22) { position in
                let value = (low * pow(high / low, position) / 10).rounded() * 10
                model.set("displayPeak", String(format: "%.0f", value))
            }
            HStack(alignment: .firstTextBaseline) {
                Text(L(
                    "HDR is fitted to this level. Set it yourself if highlights clip or the picture looks dim.",
                    "Под этот уровень подгоняется HDR. Задайте его сами, если света срезаются или картинка тусклая."))
                    .font(.system(size: 11))
                    .foregroundStyle(Color.quiet)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                if settings.displayPeak != nil {
                    Button {
                        model.set("displayPeak", "auto")
                    } label: {
                        Text(L(
                            "Use the display's \(nits(settings.reportedPeak))",
                            "Вернуть \(nits(settings.reportedPeak))"))
                            .font(.system(size: 11, weight: .medium))
                            .padding(.horizontal, 10)
                            .frame(height: 24)
                            .background(Capsule().fill(Color.black.opacity(0.22)))
                            .contentShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .fixedSize()
                }
            }
        }
    }

    // MARK: Building blocks

    private func group(_ title: String, @ViewBuilder _ rows: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title)
                .font(.system(size: 13, weight: .semibold))
            rows()
        }
    }

    private func row(_ label: String, note: String? = nil, @ViewBuilder _ control: () -> some View) -> some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 1) {
                Text(label)
                    .font(.system(size: 12))
                    .foregroundStyle(Color.quiet)
                if let note {
                    Text(note)
                        .font(.system(size: 10))
                        .foregroundStyle(Color.porcelain.opacity(0.45))
                }
            }
            Spacer(minLength: 8)
            control()
        }
    }

    private func slider(
        _ label: String, value: Double, in range: ClosedRange<Double>, step: Double,
        text: (Double) -> String, send: @escaping (Double) -> Void
    ) -> some View {
        let span = range.upperBound - range.lowerBound
        // A scale that crosses zero is filled from its zero, so "none" looks like none.
        let origin = range.lowerBound < 0 ? -range.lowerBound / span : 0
        return VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline) {
                Text(label)
                    .font(.system(size: 12))
                    .foregroundStyle(Color.quiet)
                Spacer()
                Text(text(value))
                    .font(.figures(13))
            }
            Track(
                fraction: (value - range.lowerBound) / span, fill: .porcelain, label: nil, height: 22, origin: origin
            ) { position in
                send(((range.lowerBound + position * span) / step).rounded() * step)
            }
        }
    }
}

/// A menu in a capsule, for choices too many or too long for buttons side by side.
private struct PickMenu: View {
    /// Tag and title of each choice, in order.
    let options: [(String, String)]
    let selected: String
    /// What the capsule says.
    let title: String
    let onSelect: (String) -> Void

    var body: some View {
        Menu {
            Picker(title, selection: Binding(get: { selected }, set: onSelect)) {
                ForEach(options, id: \.0) { option in
                    Text(option.1).tag(option.0)
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } label: {
            MenuCapsule(text: title)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
    }
}

/// How a menu looks before it is opened: its choice, and the sign that it opens.
private struct MenuCapsule: View {
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            Text(text)
                .font(.system(size: 12))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: 200)
                .fixedSize(horizontal: true, vertical: false)
            Image(systemName: "chevron.up.chevron.down")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(Color.quiet)
        }
        .padding(.horizontal, 10)
        .frame(height: 26)
        .background(Capsule().fill(Color.black.opacity(0.22)))
        .contentShape(Capsule())
    }
}

/// A menu of languages, with one more choice at the top for "no preference".
private struct LanguageMenu: View {
    let none: String
    /// ISO 639 code, or empty.
    let selected: String
    let onSelect: (String) -> Void

    /// The languages films usually come in. The player matches any spelling of a code.
    private static let codes = [
        "eng", "rus", "ukr", "kat", "deu", "fra", "spa", "ita", "por", "pol", "tur", "jpn", "kor", "zho",
    ]

    private var languages: [(String, String)] {
        Self.codes
            .map { ($0, languageName($0) ?? $0) }
            .sorted { $0.1.localizedCaseInsensitiveCompare($1.1) == .orderedAscending }
    }

    var body: some View {
        Menu {
            Picker(none, selection: Binding(get: { selected }, set: onSelect)) {
                Text(none).tag("")
                ForEach(languages, id: \.0) { language in
                    Text(language.1).tag(language.0)
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } label: {
            MenuCapsule(text: selected.isEmpty ? none : (languageName(selected) ?? selected))
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
    }
}

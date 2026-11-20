// The player's controls: three pieces of system glass floating over the picture.
//
//   top left      the file's name; opens another file
//   top right     the signal lens: what the video is and what is being done to it;
//                 opens into the signal panel with the light ruler
//   bottom        transport: play, time, seek, volume, full screen
//
// One rule carries over from the BitSpring audio player: colour means the signal and nothing
// else. Brass when the picture reaches the screen as it was mastered, violet when it is being
// changed to fit the display. Everything you can press is porcelain.

import AppKit
import SwiftUI

// MARK: - Tokens

extension Color {
    init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255)
    }

    static let porcelain = Color(hex: 0xE8EDEA)
    static let brass = Color(hex: 0xC9A45C)
    static let violet = Color(hex: 0x8C7AE6)
    static let alarm = Color(hex: 0xE5625E)
    static let slate = Color(hex: 0x121D23)
    /// Secondary text. Kept as dimmed porcelain so it holds up over any picture.
    static let quiet = Color.porcelain.opacity(0.66)
}

extension Font {
    /// Numbers: condensed and fixed-width, so times do not twitch as they count.
    static func figures(_ size: CGFloat, weight: Font.Weight = .medium) -> Font {
        .system(size: size, weight: weight).monospacedDigit().width(.condensed)
    }
}

/// The one glass used everywhere: the system's, toned towards slate so that porcelain text
/// stays readable over the brightest highlights and the panel itself never glares.
func playerGlass(interactive: Bool = false) -> Glass {
    Glass.regular.tint(Color.slate.opacity(0.38)).interactive(interactive)
}

extension Treatment {
    var color: Color { untouched ? .brass : .violet }
}

enum Layout {
    /// Gap above the top row of glass.
    static let topInset: CGFloat = 8
    /// Height of the capsules in the top row.
    static let topHeight: CGFloat = 36
}

// MARK: - Words

/// The system's first language, unless `BSV_LANG` names another (for trying a translation).
private let russian = (ProcessInfo.processInfo.environment["BSV_LANG"] ?? Locale.preferredLanguages.first ?? "en")
    .hasPrefix("ru")

func L(_ en: String, _ ru: String) -> String { russian ? ru : en }

let nitsUnit = L("cd/m²", "кд/м²")

/// A number with at most `places` decimals and no trailing zeros.
func trimmed(_ value: Double, places: Int = 3) -> String {
    String(format: "%.\(places)f", value)
        .replacingOccurrences(of: "\\.?0+$", with: "", options: .regularExpression)
}

func nits(_ value: Double) -> String {
    if value >= 10 { return String(format: "%.0f", value) }
    if value >= 0.1 { return trimmed(value, places: 2) }
    return trimmed(value, places: 4)
}

/// Codec names the way they are written on a box, not the way FFmpeg spells them.
func codecName(_ raw: String) -> String {
    switch raw.lowercased() {
    case "h264": return "H.264"
    case "hevc": return "HEVC"
    case "av1": return "AV1"
    case "vp9": return "VP9"
    case "vvc": return "VVC"
    case "mpeg2video": return "MPEG-2"
    case "eac3": return "E-AC-3"
    case "ac3": return "AC-3"
    case "truehd": return "TrueHD"
    case "dts": return "DTS"
    case "aac": return "AAC"
    case "flac": return "FLAC"
    case "opus": return "Opus"
    case "vorbis": return "Vorbis"
    case "mp3": return "MP3"
    default: return raw.hasPrefix("pcm_") ? "PCM" : raw.uppercased()
    }
}

/// "English", "Русский": the language a track is in, from the code the file carries.
func languageName(_ code: String) -> String? {
    guard !code.isEmpty, code != "und" else { return nil }
    // Files often use the older bibliographic codes, which the system does not know.
    let modern = [
        "ger": "de", "fre": "fr", "chi": "zh", "cze": "cs", "dut": "nl", "gre": "el", "rum": "ro",
        "per": "fa", "slo": "sk", "ice": "is", "mac": "mk", "may": "ms", "arm": "hy", "baq": "eu",
        "bur": "my", "geo": "ka", "tib": "bo", "wel": "cy", "alb": "sq",
    ]
    let locale = Locale(identifier: russian ? "ru" : "en")
    guard let name = locale.localizedString(forLanguageCode: modern[code] ?? code) else { return code }
    return name.prefix(1).uppercased() + name.dropFirst()
}

extension TrackInfo {
    /// How a sound track reads in the menu: "English, Surround, E-AC-3 5.1".
    var soundLabel: String {
        var parts: [String] = []
        if let language = languageName(language) { parts.append(language) }
        if !title.isEmpty { parts.append(title) }
        var format = codecName(codec)
        if profile.contains("Atmos") { format += " Atmos" }
        if profile.contains("HD MA") { format += "-HD MA" }
        let speakers = layout.replacingOccurrences(of: "\\(.*\\)", with: "", options: .regularExpression)
        parts.append(speakers.isEmpty ? format : "\(format) \(speakers)")
        return parts.joined(separator: ", ")
    }

    /// How a subtitle track reads in the menu: "Russian, forced".
    var subtitleLabel: String {
        var parts: [String] = []
        parts.append(languageName(language) ?? L("Track \(id)", "Дорожка \(id)"))
        if !title.isEmpty { parts.append(title) }
        if forced { parts.append(L("forced", "форсированные")) }
        if hearingImpaired { parts.append(L("for the hard of hearing", "для слабослышащих")) }
        return parts.joined(separator: ", ")
    }
}

func kilohertz(_ rate: Int) -> String {
    trimmed(Double(rate) / 1000, places: 1) + L(" kHz", " кГц")
}

func clock(_ seconds: Double) -> String {
    let s = Int(max(0, seconds.rounded(.down)))
    return s >= 3600
        ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60)
        : String(format: "%d:%02d", s / 60, s % 60)
}

extension Treatment {
    /// The two or three words next to the format name in the lens.
    var short: String {
        switch self {
        case .asIs: return L("as is", "как есть")
        case .asMastered: return L("as mastered", "как задумано")
        case .passedThrough: return L("passed through", "без обработки")
        case .mapped(let peak): return L("mapped to \(nits(peak)) \(nitsUnit)", "сведено к \(nits(peak)) \(nitsUnit)")
        case .toSdr: return L("mapped down to SDR", "сведено в SDR")
        }
    }
}

// MARK: - Root

struct PlayerOverlay: View {
    @ObservedObject var model: PlayerModel
    @Namespace private var glass

    var body: some View {
        ZStack {
            // The picture itself: drag it to move the window, double-click for full screen.
            Color.clear
                .contentShape(Rectangle())
                .gesture(WindowDragGesture())
                .onTapGesture(count: 2) { model.send("toggle-fullscreen") }

            // With the settings open, a click on the picture puts them away and does nothing
            // else: the second click of a double-click finds them already closed.
            if model.panel == .settings {
                Color.clear
                    .contentShape(Rectangle())
                    .gesture(WindowDragGesture())
                    .onTapGesture { model.toggle(.settings) }
            }

            if model.status.file == nil {
                EmptyState(model: model)
                // Settings are within reach before anything is open.
                GlassEffectContainer(spacing: 18) {
                    SettingsCorner(model: model, glass: glass)
                        .padding(.trailing, 20)
                        .padding(.top, Layout.topInset)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                }
            } else if model.controlsVisible {
                controls.transition(.opacity)
            }

            if model.dropTargeted {
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .strokeBorder(Color.porcelain.opacity(0.8), style: StrokeStyle(lineWidth: 2, dash: [10, 8]))
                    .padding(14)
                    .allowsHitTesting(false)
            }
        }
        .ignoresSafeArea()
        .environment(\.colorScheme, .dark)
        .foregroundStyle(Color.porcelain)
        .onDrop(of: [.fileURL], isTargeted: $model.dropTargeted) { providers in
            guard let provider = providers.first else { return false }
            _ = provider.loadObject(ofClass: URL.self) { url, _ in
                guard let url else { return }
                DispatchQueue.main.async { model.send("open \(url.path)") }
            }
            return true
        }
    }

    private var controls: some View {
        GlassEffectContainer(spacing: 18) {
            VStack(spacing: 0) {
                HStack(alignment: .top, spacing: 12) {
                    TitleCapsule(model: model)
                        .onHover { model.hover("title", $0) }
                    Spacer(minLength: 12)
                    SettingsCorner(model: model, glass: glass)
                        .onHover { model.hover("settings", $0) }
                    if model.panel == .signal {
                        SignalPanel(model: model)
                            .glassEffect(playerGlass(), in: .rect(cornerRadius: 24, style: .continuous))
                            .glassEffectID("signal", in: glass)
                            .onHover { model.hover("signal", $0) }
                    } else if model.status.video != nil {
                        SignalLens(model: model)
                            .glassEffect(playerGlass(interactive: true), in: .capsule)
                            .glassEffectID("signal", in: glass)
                            .onHover { model.hover("signal", $0) }
                    }
                }
                // The window's own close, minimise and zoom buttons sit top left unless in full screen.
                .padding(.leading, model.status.fullscreen ? 20 : 92)
                .padding(.trailing, 20)
                .padding(.top, Layout.topInset)

                Spacer(minLength: 0)

                Transport(model: model)
                    .onHover { model.hover("transport", $0) }
                    .frame(maxWidth: 820)
                    .padding(.horizontal, 24)
                    .padding(.bottom, 22)
            }
        }
    }
}

// MARK: - Title

private struct TitleCapsule: View {
    @ObservedObject var model: PlayerModel

    var body: some View {
        Button {
            model.send("open-dialog")
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "folder")
                    .font(.system(size: 13, weight: .medium))
                Text(model.status.file ?? "")
                    .font(.system(size: 13, weight: .medium))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .padding(.horizontal, 14)
            .frame(height: 36)
            .frame(maxWidth: 420)
            .fixedSize(horizontal: true, vertical: false)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .glassEffect(playerGlass(interactive: true), in: .capsule)
        .help(L("Open another file", "Открыть другой файл"))
    }
}

// MARK: - Signal lens and panel

private struct SignalLens: View {
    @ObservedObject var model: PlayerModel

    var body: some View {
        Button {
            model.toggle(.signal)
        } label: {
            HStack(spacing: 8) {
                SignalDot(treatment: model.status.treatment)
                Text(model.status.formatName)
                    .font(.system(size: 13, weight: .semibold))
                if let treatment = model.status.treatment {
                    Text(treatment.short)
                        .font(.system(size: 13))
                        .foregroundStyle(Color.quiet)
                }
            }
            .padding(.horizontal, 14)
            .frame(height: 36)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .help(L("How the picture is being shown", "Как показывается картинка"))
    }
}

private struct SignalDot: View {
    let treatment: Treatment?

    var body: some View {
        Circle()
            .fill(treatment?.color ?? Color.quiet)
            .frame(width: 8, height: 8)
    }
}

private struct SignalPanel: View {
    @ObservedObject var model: PlayerModel

    private var status: Status { model.status }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Button {
                model.toggle(.signal)
            } label: {
                HStack(spacing: 8) {
                    SignalDot(treatment: status.treatment)
                    Text(headline)
                        .font(.system(size: 15, weight: .semibold))
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

            if status.isHdrSource {
                LightRuler(status: status)
            }

            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
                ForEach(rows, id: \.0) { row in
                    GridRow {
                        Text(row.0)
                            .foregroundStyle(Color.quiet)
                            .gridColumnAlignment(.leading)
                        Text(row.1)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
            }
            .font(.system(size: 12))

            if status.isHdrSource {
                VStack(alignment: .leading, spacing: 6) {
                    Text(L("Who fits HDR to the screen", "Кто подгоняет HDR под экран"))
                        .font(.system(size: 12))
                        .foregroundStyle(Color.quiet)
                    Choice(
                        options: [
                            ("auto", L("The player", "Плеер")),
                            ("passthrough", L("The display", "Дисплей")),
                            ("sdr", L("Nobody: SDR", "Никто: SDR")),
                        ],
                        selected: status.mode == "tonemap" ? "auto" : status.mode,
                        onSelect: { model.send("mode \($0)") })
                }
            }
        }
        .padding(16)
        .frame(width: 372, alignment: .leading)
    }

    private var headline: String {
        guard let treatment = status.treatment else { return status.formatName }
        return "\(status.formatName), \(treatment.short)"
    }

    /// One or two sentences saying, in numbers, why the picture is treated the way it is.
    private var explanation: String {
        let display = status.display
        let peak = status.contentPeak.map(nits)
        switch status.treatment {
        case .none:
            return L("Waiting for the first frame.", "Ждём первый кадр.")
        case .asIs:
            return L(
                "An SDR video is shown as SDR. Nothing is stretched to look like HDR.",
                "SDR-видео показывается как SDR. Ничего не растягивается под HDR.")
        case .asMastered:
            if let peak {
                return L(
                    "The video goes up to \(peak) \(nitsUnit), this display to \(nits(display.peak)). Every level is shown as it was graded.",
                    "Видео доходит до \(peak) \(nitsUnit), дисплей до \(nits(display.peak)). Каждый уровень показан так, как его выставили при цветокоррекции.")
            }
            return L(
                "The video fits within this display's \(nits(display.peak)) \(nitsUnit).",
                "Видео укладывается в \(nits(display.peak)) \(nitsUnit) этого дисплея.")
        case .passedThrough:
            return L(
                "The video and its own metadata are sent on unchanged. Mapping to the screen is left to the display.",
                "Видео и его метаданные передаются без изменений. Сведение под экран делает сам дисплей.")
        case .mapped(let to):
            return L(
                "The video goes up to \(peak ?? "?") \(nitsUnit), this display to \(nits(to)). Highlights above that are compressed scene by scene; everything below stays where it was graded.",
                "Видео доходит до \(peak ?? "?") \(nitsUnit), дисплей до \(nits(to)). Света выше этого сжимаются по сценам; всё, что ниже, остаётся на своём месте.")
        case .toSdr:
            if display.hdr == "off" {
                return L(
                    "HDR is switched off for \(display.name), so the picture is mapped down to SDR. Turn it on in System Settings, Displays.",
                    "HDR для \(display.name) выключен в системе, поэтому картинка сводится в SDR. Включите его в Системных настройках, раздел «Дисплеи».")
            }
            if status.mode == "sdr" {
                return L("You chose SDR output: the HDR picture is mapped down.", "Выбран вывод в SDR: HDR-картинка сводится вниз.")
            }
            return L(
                "\(display.name) shows SDR only, so the HDR picture is mapped down.",
                "\(display.name) показывает только SDR, поэтому HDR-картинка сводится вниз.")
        }
    }

    private var rows: [(String, String)] {
        var out: [(String, String)] = []
        if let v = status.video {
            let fps = v.fps > 0 ? ", " + trimmed(v.fps) + L(" fps", " кадр/с") : ""
            out.append((
                L("Video", "Видео"),
                "\(codecName(v.codec)), \(v.width) × \(v.height), \(v.bitDepth) " + L("bit", "бит") + fps))

            var colour = "\(v.primaries) \(v.transfer)"
            if let min = v.masteringMin, let max = v.masteringMax {
                colour += L(", mastered for \(nits(min))–\(nits(max)) \(nitsUnit)", ", мастер \(nits(min))–\(nits(max)) \(nitsUnit)")
            }
            if let profile = v.dolbyVision {
                colour += L(", Dolby Vision profile \(profile)", ", Dolby Vision, профиль \(profile)")
                if v.enhancementLayer {
                    colour += L(" (base layer only)", " (только базовый слой)")
                }
            }
            out.append((L("Colour", "Цвет"), colour))
            out.append((
                L("Decoding", "Декодер"),
                v.hardware ? L("The Mac's video decoder", "Аппаратный декодер Mac") : L("Software", "Программный")))
        }
        if let s = status.signal {
            var text = "\(s.outputPrimaries) \(s.outputTransfer), \(s.outputBits) " + L("bit", "бит")
            if s.outputTransfer != "SDR" && s.outputPeak > 0 {
                text += L(", up to \(nits(s.outputPeak)) \(nitsUnit)", ", до \(nits(s.outputPeak)) \(nitsUnit)")
            }
            out.append((L("Output", "Вывод"), text))
        }
        let d = status.display
        if !d.name.isEmpty {
            var text = d.name
            switch d.hdr {
            case "on":
                text += L(", HDR on", ", HDR включён")
                if d.headroom > 1.05 {
                    text += L(
                        ", \(trimmed(d.headroom, places: 1))× above white in use",
                        ", запас \(trimmed(d.headroom, places: 1))× над белым")
                }
            case "off": text += L(", HDR switched off", ", HDR выключен")
            default: text += L(", SDR only", ", только SDR")
            }
            out.append((L("Display", "Дисплей"), text))
        }
        if let a = status.sound {
            var text = codecName(a.codec)
            if a.profile.contains("Atmos") { text += " Atmos" }
            if a.profile.contains("HD MA") { text += "-HD MA" }
            // "5.1(side)" is FFmpeg's way of saying which speakers; the count is what matters here.
            let layout = a.layout.replacingOccurrences(of: "\\(.*\\)", with: "", options: .regularExpression)
            text += " \(layout), \(kilohertz(a.sampleRate))"
            out.append((L("Sound", "Звук"), text))

            var device = a.device
            if a.channels > a.deviceChannels {
                device += a.deviceChannels == 2
                    ? L(", mixed down to stereo", ", сведено в стерео")
                    : L(", mixed down to \(a.deviceChannels) channels", ", сведено в \(a.deviceChannels) канала")
            }
            if a.sampleRate != a.deviceRate {
                device += L(", resampled to \(kilohertz(a.deviceRate))", ", пересчитано в \(kilohertz(a.deviceRate))")
            }
            out.append((L("Plays on", "Играет на"), device))
        }
        return out
    }
}

/// A row of choices where one is on. Selection is porcelain, like everything you can press.
struct Choice: View {
    let options: [(String, String)]
    let selected: String
    let onSelect: (String) -> Void

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.0) { option in
                let on = option.0 == selected
                Button {
                    onSelect(option.0)
                } label: {
                    Text(option.1)
                        .font(.system(size: 12, weight: on ? .semibold : .regular))
                        .foregroundStyle(on ? Color.slate : Color.porcelain)
                        .frame(maxWidth: .infinity)
                        .frame(height: 26)
                        .background(Capsule().fill(on ? Color.porcelain : Color.clear))
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .background(Capsule().fill(Color.black.opacity(0.22)))
    }
}

/// The video's range of light against the display's, on one logarithmic scale.
///
/// The bar for the video is brass where the display shows it as graded and violet where it
/// has to be compressed. A tick marks the current scene's peak when the file says what it is.
private struct LightRuler: View {
    let status: Status

    private let low = 0.001
    private let high = 10_000.0
    private let labelWidth: CGFloat = 62

    var body: some View {
        Canvas { context, size in
            let scaleWidth = size.width - labelWidth
            func x(_ value: Double) -> CGFloat {
                let clamped = min(max(value, low), high)
                return labelWidth + CGFloat((log10(clamped) - log10(low)) / (log10(high) - log10(low))) * scaleWidth
            }
            func bar(_ from: Double, _ to: Double, y: CGFloat, color: Color) {
                guard to > from else { return }
                let rect = CGRect(x: x(from), y: y, width: max(x(to) - x(from), 2), height: 8)
                context.fill(Path(roundedRect: rect, cornerRadius: 4), with: .color(color))
            }
            func label(_ text: String, at point: CGPoint, anchor: UnitPoint, color: Color = .quiet) {
                context.draw(
                    Text(text).font(.figures(11)).foregroundStyle(color),
                    at: point, anchor: anchor)
            }

            // Decade lines, with a figure under a few of them.
            for exponent in -3...4 {
                let value = pow(10, Double(exponent))
                var line = Path()
                line.move(to: CGPoint(x: x(value), y: 0))
                line.addLine(to: CGPoint(x: x(value), y: 40))
                context.stroke(line, with: .color(Color.porcelain.opacity(0.14)), lineWidth: 1)
            }
            for (value, text) in [(0.01, "0.01"), (1.0, "1"), (100.0, "100"), (10_000.0, "10 000")] {
                let anchor: UnitPoint = value == high ? .topTrailing : .top
                label(text, at: CGPoint(x: x(value), y: 44), anchor: anchor)
            }
            label(nitsUnit, at: CGPoint(x: 0, y: 44), anchor: .topLeading)

            // The video.
            let contentMin = status.video?.masteringMin ?? 0.005
            let contentMax = status.contentPeak ?? 1000
            let limit: Double
            switch status.treatment {
            case .mapped(let to): limit = to
            case .toSdr: limit = 203
            default: limit = .infinity
            }
            label(L("Video", "Видео"), at: CGPoint(x: 0, y: 10), anchor: .leading, color: .porcelain)
            bar(contentMin, min(contentMax, limit), y: 6, color: .brass)
            if contentMax > limit {
                bar(limit, contentMax, y: 6, color: .violet)
            }
            if let scene = status.signal?.scenePeak, scene > 0 {
                var tick = Path()
                tick.move(to: CGPoint(x: x(scene), y: 2))
                tick.addLine(to: CGPoint(x: x(scene), y: 18))
                context.stroke(tick, with: .color(.porcelain), lineWidth: 2)
            }

            // The display.
            let display = status.display
            let displayMax = display.hdr == "on" ? display.peak : 203
            label(L("Display", "Дисплей"), at: CGPoint(x: 0, y: 30), anchor: .leading, color: .porcelain)
            bar(display.min ?? 0.05, displayMax, y: 26, color: Color.porcelain.opacity(0.42))
        }
        .frame(height: 60)
        .accessibilityLabel(L("Light range of the video and the display", "Диапазон яркости видео и дисплея"))
    }
}

// MARK: - Transport

private struct Transport: View {
    @ObservedObject var model: PlayerModel

    private var status: Status { model.status }
    private var live: Color { status.treatment?.color ?? .porcelain }

    var body: some View {
        HStack(spacing: 12) {
            Button {
                model.send(status.ended ? "seek-to 0" : "toggle-pause")
            } label: {
                // The one filled shape in the interface: the key takes the signal's colour.
                Image(systemName: status.ended ? "arrow.counterclockwise" : (status.paused ? "play.fill" : "pause.fill"))
                    .font(.system(size: 15, weight: .bold))
                    .foregroundStyle(Color.slate)
                    .frame(width: 38, height: 38)
                    .background(Circle().fill(live))
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .help(status.paused ? L("Play", "Играть") : L("Pause", "Пауза"))

            Text(clock(model.shownPosition))
                .font(.figures(15))
                .frame(minWidth: 44, alignment: .trailing)

            Track(
                fraction: status.duration > 0 ? model.shownPosition / status.duration : 0,
                fill: live,
                label: { clock($0 * status.duration) },
                onChange: { model.seek(to: $0 * status.duration) })

            Button {
                model.showsRemaining.toggle()
            } label: {
                Text(model.showsRemaining ? "−" + clock(status.duration - model.shownPosition) : clock(status.duration))
                    .font(.figures(15))
                    .foregroundStyle(Color.quiet)
                    .frame(minWidth: 44, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(L("Switch between length and time left", "Длительность или оставшееся время"))

            if status.sound != nil {
                Button {
                    model.send("toggle-mute")
                } label: {
                    Image(systemName: speakerSymbol)
                        .font(.system(size: 14, weight: .medium))
                        .frame(width: 26, height: 40)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(status.muted ? L("Unmute", "Включить звук") : L("Mute", "Без звука"))

                Track(
                    fraction: status.muted ? 0 : status.volume,
                    fill: .porcelain,
                    label: nil,
                    onChange: { model.send("volume \($0)") })
                    .frame(width: 68)
            }

            if !status.subtitleTracks.isEmpty {
                Menu {
                    Picker(L("Subtitles", "Субтитры"), selection: subtitleChoice) {
                        Text(L("Off", "Выключены")).tag(-1)
                        ForEach(status.subtitleTracks) { track in
                            Text(track.subtitleLabel).tag(track.id)
                        }
                    }
                    .pickerStyle(.inline)
                } label: {
                    Image(systemName: status.subtitleTrack == nil ? "captions.bubble" : "captions.bubble.fill")
                        .font(.system(size: 14, weight: .medium))
                        .frame(width: 30, height: 40)
                        .contentShape(Rectangle())
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
                .help(L("Subtitles", "Субтитры"))
            }

            if status.audioTracks.count > 1 {
                Menu {
                    Picker(L("Sound", "Звук"), selection: soundChoice) {
                        ForEach(status.audioTracks) { track in
                            Text(track.soundLabel).tag(track.id)
                        }
                    }
                    .pickerStyle(.inline)
                } label: {
                    Image(systemName: "waveform")
                        .font(.system(size: 14, weight: .medium))
                        .frame(width: 30, height: 40)
                        .contentShape(Rectangle())
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
                .help(L("Sound track", "Звуковая дорожка"))
            }

            Button {
                model.send("toggle-fullscreen")
            } label: {
                Image(systemName: status.fullscreen
                    ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right")
                    .font(.system(size: 13, weight: .semibold))
                    .frame(width: 34, height: 40)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(status.fullscreen ? L("Leave full screen", "Выйти из полного экрана") : L("Full screen", "На весь экран"))
        }
        .padding(.leading, 9)
        .padding(.trailing, 10)
        .frame(height: 56)
        .glassEffect(playerGlass(), in: .capsule)
    }

    private var subtitleChoice: Binding<Int> {
        Binding(
            get: { status.subtitleTrack ?? -1 },
            set: { model.send($0 < 0 ? "subtitle-track off" : "subtitle-track \($0)") })
    }

    private var soundChoice: Binding<Int> {
        Binding(
            get: { status.audioTrack ?? -1 },
            set: { model.send("audio-track \($0)") })
    }

    private var speakerSymbol: String {
        if status.muted || status.volume <= 0.001 { return "speaker.slash.fill" }
        if status.volume < 0.4 { return "speaker.wave.1.fill" }
        if status.volume < 0.8 { return "speaker.wave.2.fill" }
        return "speaker.wave.3.fill"
    }
}

/// A thin track you can click or drag: the seek bar, the volume, and the sliders in settings.
struct Track: View {
    let fraction: Double
    let fill: Color
    /// Text for the bubble shown while pointing at the track, given a position from 0 to 1.
    let label: ((Double) -> String)?
    var height: CGFloat = 40
    /// Where the filled part starts, 0 to 1: the left end, or the zero of a scale that runs both ways.
    var origin: Double = 0
    let onChange: (Double) -> Void

    @State private var hover: CGFloat?
    @State private var drag: Double?
    @State private var lastSent = Date.distantPast

    var body: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            let shown = min(max(drag ?? fraction, 0), 1)
            let active = hover != nil || drag != nil
            let thickness: CGFloat = active ? 6 : 4

            ZStack(alignment: .leading) {
                Capsule().fill(Color.black.opacity(0.28)).frame(height: thickness)
                Capsule().fill(Color.porcelain.opacity(0.3)).frame(height: thickness)
                Capsule().fill(fill)
                    .frame(width: max(width * abs(shown - origin), thickness), height: thickness)
                    .offset(x: width * min(shown, origin))
                Circle()
                    .fill(Color.porcelain)
                    .frame(width: 13, height: 13)
                    .shadow(color: .black.opacity(0.35), radius: 2, y: 1)
                    .offset(x: width * shown - 6.5)
                    .opacity(active ? 1 : 0)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .contentShape(Rectangle())
            .animation(.easeOut(duration: 0.12), value: active)
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        let position = min(max(value.location.x / width, 0), 1)
                        drag = position
                        // While dragging, follow along a few times a second.
                        if Date().timeIntervalSince(lastSent) > 0.15 {
                            lastSent = Date()
                            onChange(position)
                        }
                    }
                    .onEnded { value in
                        onChange(min(max(value.location.x / width, 0), 1))
                        drag = nil
                    }
            )
            .onContinuousHover { phase in
                switch phase {
                case .active(let point): hover = point.x
                case .ended: hover = nil
                }
            }
            .overlay(alignment: .topLeading) {
                if let label, let at = drag.map({ CGFloat($0) * width }) ?? hover {
                    Text(label(Double(min(max(at / width, 0), 1))))
                        .font(.figures(13))
                        .padding(.horizontal, 10)
                        .frame(height: 26)
                        .glassEffect(playerGlass(), in: .capsule)
                        .fixedSize()
                        .alignmentGuide(.leading) { $0.width / 2 }
                        .offset(x: at, y: -40)
                        .allowsHitTesting(false)
                }
            }
        }
        .frame(height: height)
    }
}

// MARK: - Nothing open

private struct EmptyState: View {
    @ObservedObject var model: PlayerModel

    var body: some View {
        VStack(spacing: 14) {
            Text(L("Drop a video here", "Перетащите сюда видео"))
                .font(.system(size: 30, weight: .semibold))

            Text(displayLine)
                .font(.system(size: 14))
                .foregroundStyle(model.status.display.hdr == "off" ? Color.brass : Color.quiet)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 460)

            if let error = model.status.error {
                Text(error)
                    .font(.system(size: 13))
                    .foregroundStyle(Color.alarm)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 460)
            }

            Button {
                model.send("open-dialog")
            } label: {
                Text(L("Open a file…", "Открыть файл…"))
                    .font(.system(size: 14, weight: .medium))
                    .padding(.horizontal, 20)
                    .frame(height: 38)
                    .contentShape(Capsule())
            }
            .buttonStyle(.plain)
            .glassEffect(Glass.regular.interactive(), in: .capsule)
            .padding(.top, 8)
        }
        .padding(40)
    }

    /// The first thing the player says is what the connected display can do.
    private var displayLine: String {
        let d = model.status.display
        guard !d.name.isEmpty else { return "" }
        switch d.hdr {
        case "on":
            return d.peakKnown
                ? L("\(d.name) is showing HDR, up to \(nits(d.peak)) \(nitsUnit).", "\(d.name) показывает HDR, до \(nits(d.peak)) \(nitsUnit).")
                : L("\(d.name) is showing HDR. It does not say how bright it gets.", "\(d.name) показывает HDR. Свою яркость он не сообщает.")
        case "off":
            return L(
                "\(d.name) can show HDR, but it is switched off. Turn it on in System Settings, Displays.",
                "\(d.name) умеет HDR, но он выключен. Включите его в Системных настройках, раздел «Дисплеи».")
        default:
            return L("\(d.name) shows SDR. HDR video will be mapped down to it.", "\(d.name) показывает SDR. HDR-видео будет сведено под него.")
        }
    }
}

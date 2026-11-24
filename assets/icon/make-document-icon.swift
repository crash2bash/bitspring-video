// Draws the icon shown on video files the player opens by default:
//
//   swift assets/icon/make-document-icon.swift MKV Document-MKV.iconset   every size, for iconutil
//   swift assets/icon/make-document-icon.swift MKV Document-MKV.ico       every size, for Windows
//   swift assets/icon/make-document-icon.swift MKV preview.png            one picture, 1024 wide
//   swift assets/icon/make-document-icon.swift "" Document.iconset        with no format on it
//
// The Mac's are drawn when the application is bundled (scripts/bundle-macos.sh). Windows is
// built where there is no Swift, so its pictures are kept in windows/BitSpringVideo/Assets;
// after changing this file, draw them again:
//
//   for f in MKV WEBM MP4 M4V MOV TS M2TS AVI M3U; do
//     swift assets/icon/make-document-icon.swift $f windows/BitSpringVideo/Assets/Document-$f.ico
//   done
//   swift assets/icon/make-document-icon.swift "" windows/BitSpringVideo/Assets/Document.ico
//
// The page is the app icon's own slate with its gold winding drawn large on it, and the name of
// the format underneath: a film in a folder looks like this player's and says what it is. For
// anything that counts as a movie macOS shows its own camcorder page whatever app is chosen to
// open it, unless that app names a picture for its files.
//
// Each size is drawn for itself, not shrunk from the largest: below a certain size the winding
// would turn to mud and the letters to dust, so small pictures get fewer, heavier lines and no
// name, and the smallest a plain triangle.

import AppKit

let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    FileHandle.standardError.write("usage: make-document-icon.swift <FORMAT or \"\"> <out.iconset | out.ico | out.png>\n".data(using: .utf8)!)
    exit(2)
}
let format = arguments[1]
let output = arguments[2]

let space = CGColorSpace(name: CGColorSpace.sRGB)!

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        colorSpace: space,
        components: [CGFloat((hex >> 16) & 0xFF) / 255, CGFloat((hex >> 8) & 0xFF) / 255, CGFloat(hex & 0xFF) / 255, alpha])!
}

/// A play triangle with rounded corners, centred on its centre of mass, pointing right.
/// The same shape as in `make-icon.swift`.
func triangle(radius: CGFloat, rounding: CGFloat) -> CGPath {
    let points = (0..<3).map { i -> CGPoint in
        let angle = CGFloat(i) * 2 * .pi / 3
        return CGPoint(x: cos(angle) * radius, y: sin(angle) * radius)
    }
    let path = CGMutablePath()
    path.move(to: CGPoint(x: (points[0].x + points[1].x) / 2, y: (points[0].y + points[1].y) / 2))
    for i in 1...3 {
        path.addArc(tangent1End: points[i % 3], tangent2End: points[(i + 1) % 3], radius: rounding)
    }
    path.closeSubpath()
    return path
}

// The page, in a canvas of 1024: where the system puts its own pages, so a film stands level
// with its neighbours in a folder.
let left: CGFloat = 180, right: CGFloat = 844, bottom: CGFloat = 72, top: CGFloat = 952
let corner: CGFloat = 36
let fold: CGFloat = 212

/// The page's outline, its upper right corner cut off along the fold.
func pagePath() -> CGPath {
    let path = CGMutablePath()
    path.move(to: CGPoint(x: left + corner, y: bottom))
    path.addArc(tangent1End: CGPoint(x: right, y: bottom), tangent2End: CGPoint(x: right, y: top), radius: corner)
    path.addLine(to: CGPoint(x: right, y: top - fold))
    path.addLine(to: CGPoint(x: right - fold, y: top))
    path.addArc(tangent1End: CGPoint(x: left, y: top), tangent2End: CGPoint(x: left, y: bottom), radius: corner)
    path.addArc(tangent1End: CGPoint(x: left, y: bottom), tangent2End: CGPoint(x: right, y: bottom), radius: corner)
    path.closeSubpath()
    return path
}

/// The corner folded over: the page's underside, lying on the page.
func flapPath() -> CGPath {
    let path = CGMutablePath()
    path.move(to: CGPoint(x: right - fold, y: top))
    path.addLine(to: CGPoint(x: right, y: top - fold))
    path.addArc(
        tangent1End: CGPoint(x: right - fold, y: top - fold), tangent2End: CGPoint(x: right - fold, y: top),
        radius: corner * 1.2)
    path.closeSubpath()
    return path
}

/// `fill`: how much larger than the Mac's page to draw it. The Mac leaves a wide margin round
/// every icon; Windows draws its pages to the edge of theirs.
func draw(pixels: Int, fill: CGFloat = 1) -> CGImage {
    let context = CGContext(
        data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0, space: space,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    let scale = CGFloat(pixels) / 1024 * fill
    context.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)
    context.translateBy(x: 512, y: 512)
    context.scaleBy(x: fill, y: fill)
    context.translateBy(x: -512, y: -512)

    let named = !format.isEmpty && pixels >= 128
    let page = pagePath()

    // The page and the shadow it casts.
    context.saveGState()
    context.setShadow(offset: CGSize(width: 0, height: -10 * scale), blur: 22 * scale, color: color(0x000000, 0.34))
    context.addPath(page)
    context.setFillColor(color(0x16232A))
    context.fillPath()
    context.restoreGState()

    context.saveGState()
    context.addPath(page)
    context.clip()
    let slate = CGGradient(colorsSpace: space, colors: [color(0x2D404B), color(0x121D23)] as CFArray, locations: [0, 1])!
    context.drawLinearGradient(slate, start: CGPoint(x: 512, y: top), end: CGPoint(x: 512, y: bottom), options: [])
    // A thin light edge: on a dark folder background the slate would otherwise have no outline.
    context.addPath(page)
    context.setStrokeColor(color(0xFFFFFF, 0.16))
    context.setLineWidth(max(6, 2 / scale))
    context.strokePath()
    context.restoreGState()

    // The mark. With a name under it, it moves up and gives the name room.
    let radius: CGFloat = named ? 268 : 280
    let centre = CGPoint(x: 512 - radius * 0.126, y: named ? 600 : 512)
    let strokes = CGMutablePath()
    if pixels <= 32 {
        strokes.addPath(
            triangle(radius: radius * 0.92, rounding: radius * 0.2),
            transform: CGAffineTransform(translationX: centre.x, y: centre.y))
    } else {
        // (how many triangles, each this much smaller, turned this many degrees further, line from, to)
        let (count, shrink, turn, heavy, light): (Int, CGFloat, CGFloat, CGFloat, CGFloat) =
            pixels >= 256 ? (13, 0.86, 9.5, 9.5, 5.5) : pixels >= 128 ? (9, 0.80, 14, 13, 8) : (5, 0.68, 24, 24, 15)
        for i in 0..<count {
            let t = CGFloat(i) / CGFloat(count - 1)
            let r = radius * pow(shrink, CGFloat(i))
            let transform = CGAffineTransform(translationX: centre.x, y: centre.y)
                .rotated(by: CGFloat(i) * turn * .pi / 180)
            let outline = triangle(radius: r, rounding: r * 0.2)
                .copy(strokingWithWidth: heavy - (heavy - light) * t, lineCap: .round, lineJoin: .round, miterLimit: 4)
            strokes.addPath(outline, transform: transform)
        }
    }
    // Gold, lit from the upper left like the app icon's.
    context.saveGState()
    context.addPath(strokes)
    context.clip(using: .winding)
    let gold = CGGradient(
        colorsSpace: space,
        colors: [color(0xF0D58C), color(0xD9A441), color(0xB07E22), color(0xE2B95C)] as CFArray,
        locations: [0, 0.4, 0.75, 1])!
    context.drawLinearGradient(
        gold, start: CGPoint(x: centre.x - radius * 0.75, y: centre.y + radius),
        end: CGPoint(x: centre.x + radius * 0.9, y: centre.y - radius), options: [])
    context.restoreGState()

    // The name of the format, in the rounded letters that go with the triangle's corners.
    if named {
        var font = NSFont.systemFont(ofSize: 118, weight: .semibold)
        if let rounded = font.fontDescriptor.withDesign(.rounded) {
            font = NSFont(descriptor: rounded, size: 118) ?? font
        }
        let text = NSAttributedString(
            string: format,
            attributes: [
                .font: font, .kern: 7,
                .foregroundColor: NSColor(cgColor: color(0xEFEADF, 0.94))!,
            ])
        let line = CTLineCreateWithAttributedString(text)
        let bounds = CTLineGetBoundsWithOptions(line, .useGlyphPathBounds)
        // Centred on the page by what is seen: the last letter's spacing is not part of it.
        context.textPosition = CGPoint(x: 512 - (bounds.width - 7) / 2 - bounds.minX, y: 168)
        CTLineDraw(line, context)
    }

    // The folded corner, lighter than the page and casting a little shadow on it.
    let flap = flapPath()
    context.saveGState()
    context.addPath(page)
    context.clip()
    context.setShadow(offset: CGSize(width: -6 * scale, height: -10 * scale), blur: 26 * scale, color: color(0x000000, 0.5))
    context.addPath(flap)
    context.setFillColor(color(0x52697A))
    context.fillPath()
    context.restoreGState()
    context.saveGState()
    context.addPath(flap)
    context.clip()
    let underside = CGGradient(colorsSpace: space, colors: [color(0x6B8494), color(0x3E535F)] as CFArray, locations: [0, 1])!
    context.drawLinearGradient(
        underside, start: CGPoint(x: right - fold, y: top - fold), end: CGPoint(x: right - fold * 0.4, y: top - fold * 0.4),
        options: [.drawsAfterEndLocation])
    context.restoreGState()

    return context.makeImage()!
}

func png(_ image: CGImage) -> Data {
    NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])!
}

func write(_ image: CGImage, to path: String) {
    try! png(image).write(to: URL(fileURLWithPath: path))
}

/// A picture the way Windows icon files have always held them: a bitmap header, the pixels as
/// blue, green, red and opacity from the bottom row up, then a one-bit mask (left empty: the
/// opacity says it all). Every program on Windows reads this; a PNG inside an icon file is read
/// reliably only at the largest size.
func bitmap(_ image: CGImage) -> Data {
    let side = image.width
    var pixels = [UInt8](repeating: 0, count: side * side * 4)
    let context = CGContext(
        data: &pixels, width: side, height: side, bitsPerComponent: 8, bytesPerRow: side * 4, space: space,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.draw(image, in: CGRect(x: 0, y: 0, width: side, height: side))

    func bytes<T: FixedWidthInteger>(_ value: T) -> Data {
        withUnsafeBytes(of: value.littleEndian) { Data($0) }
    }
    // The header gives twice the height: the pixels and the mask are counted together.
    var data = bytes(UInt32(40)) + bytes(Int32(side)) + bytes(Int32(side * 2)) + bytes(UInt16(1)) + bytes(UInt16(32))
    data += Data(count: 24)
    for row in (0..<side).reversed() {
        for column in 0..<side {
            let i = (row * side + column) * 4
            let alpha = Int(pixels[i + 3])
            // Stored with colour and opacity apart; drawn here with them multiplied together.
            let plain = { (c: UInt8) -> UInt8 in alpha == 0 ? 0 : UInt8(min(255, (Int(c) * 255 + alpha / 2) / alpha)) }
            data.append(contentsOf: [plain(pixels[i + 2]), plain(pixels[i + 1]), plain(pixels[i]), UInt8(alpha)])
        }
    }
    data += Data(count: ((side + 31) / 32) * 4 * side)
    return data
}

/// A Windows icon file: a list of the sizes inside, then each picture.
func icon(_ pictures: [(pixels: Int, data: Data)]) -> Data {
    func bytes<T: FixedWidthInteger>(_ value: T) -> Data {
        withUnsafeBytes(of: value.littleEndian) { Data($0) }
    }
    var file = bytes(UInt16(0)) + bytes(UInt16(1)) + bytes(UInt16(pictures.count))
    var offset = 6 + 16 * pictures.count
    for picture in pictures {
        // One byte each for width and height, where 0 stands for 256.
        file += Data([UInt8(picture.pixels % 256), UInt8(picture.pixels % 256), 0, 0])
        file += bytes(UInt16(1)) + bytes(UInt16(32)) + bytes(UInt32(picture.data.count)) + bytes(UInt32(offset))
        offset += picture.data.count
    }
    for picture in pictures {
        file += picture.data
    }
    return file
}

if output.hasSuffix(".iconset") {
    try! FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
    for points in [16, 32, 128, 256, 512] {
        write(draw(pixels: points), to: "\(output)/icon_\(points)x\(points).png")
        write(draw(pixels: points * 2), to: "\(output)/icon_\(points)x\(points)@2x.png")
    }
} else if output.hasSuffix(".ico") {
    // The sizes Explorer asks for at its usual scales; anything between is made from the largest.
    let pictures = [16, 20, 24, 32, 40, 48, 64, 256].map { pixels -> (pixels: Int, data: Data) in
        let image = draw(pixels: pixels, fill: 1.1)
        return (pixels, pixels < 256 ? bitmap(image) : png(image))
    }
    try! icon(pictures).write(to: URL(fileURLWithPath: output))
} else {
    write(draw(pixels: 1024), to: output)
}

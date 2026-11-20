// Draws the app icon: `swift assets/icon/make-icon.swift assets/icon/icon.png`
//
// A sibling of the BitSpring audio player's icon: the same slate tile and thin gold line, but
// where that one winds a disc, this one winds a play triangle down to a point.

import AppKit

let size: CGFloat = 1024
let output = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon.png"

let space = CGColorSpace(name: CGColorSpace.sRGB)!
let context = CGContext(
    data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0, space: space,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        colorSpace: space,
        components: [CGFloat((hex >> 16) & 0xFF) / 255, CGFloat((hex >> 8) & 0xFF) / 255, CGFloat(hex & 0xFF) / 255, alpha])!
}

// The tile: the system's icon shape, 824 points in a 1024 canvas, with its soft shadow.
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let tilePath = CGPath(roundedRect: tile, cornerWidth: 186, cornerHeight: 186, transform: nil)
context.saveGState()
context.setShadow(offset: CGSize(width: 0, height: -10), blur: 28, color: color(0x000000, 0.35))
context.addPath(tilePath)
context.setFillColor(color(0x16232A))
context.fillPath()
context.restoreGState()

context.saveGState()
context.addPath(tilePath)
context.clip()
let slate = CGGradient(colorsSpace: space, colors: [color(0x2A3B45), color(0x121D23)] as CFArray, locations: [0, 1])!
context.drawLinearGradient(slate, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])
context.restoreGState()

/// A play triangle with rounded corners, centred on its centre of mass, pointing right.
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

// The winding: each triangle a little smaller and turned a little further than the last.
let strokes = CGMutablePath()
let count = 13
for i in 0..<count {
    let t = CGFloat(i) / CGFloat(count - 1)
    let radius = 350 * pow(0.86, CGFloat(i))
    let turn = CGFloat(i) * 9.5 * .pi / 180
    let transform = CGAffineTransform(translationX: 468, y: 512).rotated(by: turn)
    let outline = triangle(radius: radius, rounding: radius * 0.2)
        .copy(strokingWithWidth: 12 - 5 * t, lineCap: .round, lineJoin: .round, miterLimit: 4)
    strokes.addPath(outline, transform: transform)
}

// Gold, lit from the upper left like the audio icon's.
context.saveGState()
context.addPath(strokes)
context.clip(using: .winding)
let gold = CGGradient(
    colorsSpace: space,
    colors: [color(0xF0D58C), color(0xD9A441), color(0xB07E22), color(0xE2B95C)] as CFArray,
    locations: [0, 0.4, 0.75, 1])!
context.drawLinearGradient(gold, start: CGPoint(x: 220, y: 860), end: CGPoint(x: 800, y: 170), options: [])
context.restoreGState()

let image = context.makeImage()!
let bitmap = NSBitmapImageRep(cgImage: image)
try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output))
print("wrote \(output)")

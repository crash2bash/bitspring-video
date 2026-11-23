// Draws the icon shown on video files the player opens by default:
// `swift assets/icon/make-document-icon.swift assets/icon/icon.png assets/icon/document.png`
//
// The system's own blank page with the app icon on it, which is what macOS composes by itself
// for most kinds of file. For anything that counts as a movie it does not: it shows its own
// camcorder page whatever the app says, unless the app names a picture of its own.

import AppKit
import UniformTypeIdentifiers

let arguments = CommandLine.arguments
let source = arguments.count > 1 ? arguments[1] : "icon.png"
let output = arguments.count > 2 ? arguments[2] : "document.png"

let size = 1024
guard let mark = NSImage(contentsOfFile: source) else {
    FileHandle.standardError.write("cannot read \(source)\n".data(using: .utf8)!)
    exit(1)
}
let canvas = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8, samplesPerPixel: 4,
    hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
canvas.size = NSSize(width: size, height: size)

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: canvas)
NSGraphicsContext.current?.imageInterpolation = .high

// The system draws this page a little larger than the pages it puts on files itself; brought
// down to their size so a film does not stand taller than its neighbours in a folder.
let fit = NSAffineTransform()
fit.translateX(by: CGFloat(size) / 2, yBy: CGFloat(size) / 2)
fit.scale(by: 0.965)
fit.translateX(by: -CGFloat(size) / 2, yBy: -CGFloat(size) / 2)
fit.concat()

// A file of no particular kind: a page with a folded corner and nothing on it.
let page = NSWorkspace.shared.icon(for: .data)
page.draw(in: NSRect(x: 0, y: 0, width: size, height: size))

// The page fills the canvas from 7% to 93% of its height and is narrower than it is tall; the
// mark sits in the middle of it, a little below centre so the fold does not crowd it. The
// picture of the app icon carries its own margin and shadow, hence the larger box.
let side = CGFloat(size) * 0.48
mark.draw(in: NSRect(x: (CGFloat(size) - side) / 2, y: CGFloat(size) * 0.44 - side / 2, width: side, height: side))

NSGraphicsContext.restoreGraphicsState()
try canvas.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output))

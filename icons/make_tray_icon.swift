import AppKit

// Aloud menubar template icon v2 — redesign per the owner's spec:
// "A square, with a cross (crosshair) on the upper-left corner —
// representing the area-selection tool — the letter A inside the square,
// and a small volume/speaker icon at the bottom-right corner."
//
// Drawn directly into a 44x44-*pixel* NSBitmapImageRep (not via
// NSImage.lockFocus, which renders at the screen's backing scale factor
// and would silently double the pixel size). 44px is retina resolution
// for a 22pt menubar slot.
//
// Template-image contract: every filled/stroked shape uses pure
// NSColor.black, on a fully transparent background, with LCD font
// smoothing disabled for the letter glyph so no colour fringing sneaks
// into the alpha-antialiased edges. macOS recolours the whole thing for
// light/dark menu bars via `.icon_as_template(true)`.

let S = 44
guard
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: S,
        pixelsHigh: S,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    )
else {
    fatalError("could not allocate bitmap rep")
}
rep.size = NSSize(width: S, height: S)

let ctx = NSGraphicsContext(bitmapImageRep: rep)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = ctx
ctx.shouldAntialias = true
// Grayscale (alpha-only) anti-aliasing everywhere, no LCD subpixel
// smoothing for the text glyph below — subpixel smoothing blends
// against a colour background assumption and can introduce non-grey
// (non-black) RGB into edge pixels, which would break the template-image
// contract.
ctx.cgContext.setAllowsFontSmoothing(false)
ctx.cgContext.setShouldSmoothFonts(false)

NSColor.black.setFill()
NSColor.black.setStroke()

// --- 1. The square: doubles as the spec's literal "square" and as the
// selection-marquee frame the crosshair sits on. Stroked, not filled, so
// the letter A can live inside it without a solid black backdrop.
let squareRect = NSRect(x: 4, y: 4, width: 36, height: 36)
let square = NSBezierPath(rect: squareRect)
square.lineWidth = 3.0
square.stroke()

// --- 2. Crosshair, snapped to the square's upper-left corner —
// represents the area-selection tool's cursor.
func crossAt(_ center: NSPoint, arm: CGFloat, lineWidth: CGFloat) {
    let h = NSBezierPath()
    h.move(to: NSPoint(x: center.x - arm, y: center.y))
    h.line(to: NSPoint(x: center.x + arm, y: center.y))
    h.lineWidth = lineWidth
    h.lineCapStyle = .round
    h.stroke()

    let v = NSBezierPath()
    v.move(to: NSPoint(x: center.x, y: center.y - arm))
    v.line(to: NSPoint(x: center.x, y: center.y + arm))
    v.lineWidth = lineWidth
    v.lineCapStyle = .round
    v.stroke()
}
crossAt(NSPoint(x: 6, y: 38), arm: 6, lineWidth: 2.6)

// --- 3. The letter "A", bold, centered in the square.
let font = NSFont.boldSystemFont(ofSize: 21)
let attrs: [NSAttributedString.Key: Any] = [
    .font: font,
    .foregroundColor: NSColor.black,
]
let letter = NSAttributedString(string: "A", attributes: attrs)
let letterSize = letter.size()
let letterOrigin = NSPoint(
    x: squareRect.midX - letterSize.width / 2,
    y: squareRect.midY - letterSize.height / 2 - 1  // optical centering: caps sit slightly high
)
letter.draw(at: letterOrigin)

// --- 4. Small speaker glyph, straddling the square's bottom-right
// corner (mirroring how the crosshair straddles the top-left corner) —
// box + cone, no wave arcs, kept minimal so it reads as a distinct small
// mark, not mush, at 22pt. Two earlier passes tucked it entirely inside
// the corner, where it fused visually with the frame stroke and vanished
// at native 44px; pulling it out to overlap/cross the corner (like the
// crosshair does) isolates it against transparency instead.
let box = NSBezierPath()
box.move(to: NSPoint(x: 33, y: 2))
box.line(to: NSPoint(x: 37, y: 2))
box.line(to: NSPoint(x: 37, y: 7))
box.line(to: NSPoint(x: 33, y: 7))
box.close()
box.fill()

let cone = NSBezierPath()
cone.move(to: NSPoint(x: 37, y: 7))
cone.line(to: NSPoint(x: 42, y: 10))
cone.line(to: NSPoint(x: 42, y: -1))
cone.line(to: NSPoint(x: 37, y: 2))
cone.close()
cone.fill()

NSGraphicsContext.current?.flushGraphics()
NSGraphicsContext.restoreGraphicsState()

let png = rep.representation(using: .png, properties: [:])!
try! png.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
print("wrote \(CommandLine.arguments[1]) — \(rep.pixelsWide)x\(rep.pixelsHigh)")

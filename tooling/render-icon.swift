import AppKit
import CoreGraphics

// Renders the Icon Composer icon (app/resources/dispatch.icon) as a macOS app icon PNG:
// a 824-point continuous rounded tile on a 1024 canvas, the gradient fill, and the
// artwork at 55% with a neutral shadow. It makes app/resources/dispatch-icon.png (the Dock
// icon of an unbundled build) and docs/assets/icon.png:
//   swift tooling/render-icon.swift app/resources/dispatch.icon/Assets/Vector.png <out.png> <size>
let args = CommandLine.arguments
let art = NSImage(contentsOfFile: args[1])!.cgImage(forProposedRect: nil, context: nil, hints: nil)!
let size = Int(args[3]) ?? 1024
let s = CGFloat(size) / 1024
let cs = CGColorSpace(name: CGColorSpace.displayP3)!
let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
let tile = CGRect(x: 100 * s, y: 100 * s, width: 824 * s, height: 824 * s)
let path = CGPath(roundedRect: tile, cornerWidth: 185 * s, cornerHeight: 185 * s, transform: nil)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -10 * s), blur: 28 * s, color: CGColor(gray: 0, alpha: 0.35))
ctx.addPath(path); ctx.setFillColor(CGColor(gray: 0, alpha: 1)); ctx.fillPath()
ctx.restoreGState()
ctx.saveGState()
ctx.addPath(path); ctx.clip()
let g = CGGradient(colorsSpace: cs, colors: [CGColor(colorSpace: cs, components: [0.1074, 0.1074, 0.1074, 1])!, CGColor(colorSpace: cs, components: [0, 0, 0, 1])!] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(g, start: CGPoint(x: 0, y: tile.maxY), end: CGPoint(x: 0, y: tile.minY), options: [])
let w = tile.width * 0.55
let artRect = CGRect(x: tile.midX - w / 2, y: tile.midY - w / 2, width: w, height: w)
ctx.setShadow(offset: CGSize(width: 0, height: -6 * s), blur: 18 * s, color: CGColor(gray: 0, alpha: 0.5))
ctx.draw(art, in: artRect)
ctx.restoreGState()
let out = NSBitmapImageRep(cgImage: ctx.makeImage()!)
try! out.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))

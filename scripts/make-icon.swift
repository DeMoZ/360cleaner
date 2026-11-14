// Рисует значок 1024×1024: скруглённый квадрат с градиентом (как логотип в окне) и сфера-сетка 360°.
// Запуск: swift scripts/make-icon.swift assets/icon.png
import AppKit

let size: CGFloat = 1024
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon.png"
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                           bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let ctx = NSGraphicsContext.current!.cgContext

// поле как у значков macOS: 824 из 1024
let inset: CGFloat = 100
let rect = CGRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
let shape = CGPath(roundedRect: rect, cornerWidth: 185, cornerHeight: 185, transform: nil)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: NSColor.black.withAlphaComponent(0.35).cgColor)
ctx.addPath(shape)
ctx.setFillColor(NSColor.black.cgColor)
ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(shape)
ctx.clip()
let colors = [NSColor(red: 0.23, green: 0.51, blue: 0.96, alpha: 1).cgColor,
              NSColor(red: 0.55, green: 0.36, blue: 0.96, alpha: 1).cgColor] as CFArray
let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1])!
ctx.drawLinearGradient(gradient, start: CGPoint(x: rect.minX, y: rect.maxY), end: CGPoint(x: rect.maxX, y: rect.minY),
                       options: [])
ctx.restoreGState()

// сфера
let c = CGPoint(x: size / 2, y: size / 2)
let r: CGFloat = 270
ctx.setStrokeColor(NSColor.white.cgColor)
ctx.setLineWidth(34)
ctx.setLineCap(.round)
ctx.strokeEllipse(in: CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r))
ctx.setLineWidth(26)
ctx.strokeEllipse(in: CGRect(x: c.x - r * 0.42, y: c.y - r, width: 2 * r * 0.42, height: 2 * r))
ctx.move(to: CGPoint(x: c.x - r, y: c.y))
ctx.addLine(to: CGPoint(x: c.x + r, y: c.y))
ctx.strokePath()
for k in [-0.55, 0.55] as [CGFloat] {
    let y = c.y + r * k
    let half = r * (1 - k * k).squareRoot()
    ctx.move(to: CGPoint(x: c.x - half, y: y))
    ctx.addLine(to: CGPoint(x: c.x + half, y: y))
    ctx.strokePath()
}
// стрелка вокруг сферы: «оборот 360°»
ctx.setLineWidth(30)
ctx.setStrokeColor(NSColor.white.withAlphaComponent(0.85).cgColor)
let ar = r + 78
ctx.addArc(center: c, radius: ar, startAngle: .pi * 0.15, endAngle: .pi * 0.85, clockwise: true)
ctx.strokePath()
let end: CGFloat = .pi * 0.85
let tip0 = CGPoint(x: c.x + ar * cos(end), y: c.y + ar * sin(end))
// направление движения по часовой стрелке в конце дуги и нормаль к нему
let t = CGPoint(x: sin(end), y: -cos(end))
let n = CGPoint(x: cos(end), y: sin(end))
let tip = CGPoint(x: tip0.x + t.x * 70, y: tip0.y + t.y * 70)
ctx.setFillColor(NSColor.white.withAlphaComponent(0.85).cgColor)
ctx.move(to: tip)
ctx.addLine(to: CGPoint(x: tip0.x + n.x * 56 - t.x * 10, y: tip0.y + n.y * 56 - t.y * 10))
ctx.addLine(to: CGPoint(x: tip0.x - n.x * 56 - t.x * 10, y: tip0.y - n.y * 56 - t.y * 10))
ctx.closePath()
ctx.fillPath()

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))

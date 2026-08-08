import Foundation
import Vision
import AppKit

// aloud-ocr <image-path> [lang...]  -> recognized text on stdout
let args = CommandLine.arguments
guard args.count >= 2, let img = NSImage(contentsOfFile: args[1]),
      let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write("aloud-ocr: cannot read image\n".data(using: .utf8)!)
    exit(2)
}
let req = VNRecognizeTextRequest()
req.recognitionLevel = .accurate       // .fast is BOTH slower and loses umlauts — measured
req.usesLanguageCorrection = true
if args.count > 2 { req.recognitionLanguages = Array(args[2...]) }
do {
    try VNImageRequestHandler(cgImage: cg, options: [:]).perform([req])
    let lines = (req.results ?? []).compactMap { $0.topCandidates(1).first?.string }
    print(lines.joined(separator: "\n"))
} catch {
    FileHandle.standardError.write("aloud-ocr: \(error)\n".data(using: .utf8)!)
    exit(1)
}

import SwiftUI

/// The agent's Markdown. `Text` keeps inline styles but drops block structure, so blocks are laid out here.
struct MarkdownText: View {
  private let blocks: [MarkdownBlock]

  init(_ source: String) {
    blocks = MarkdownBlock.parse(source)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      ForEach(blocks) { block in
        switch block.kind {
        case .paragraph:
          Text(block.content)
        case .heading(let level):
          Text(block.content)
            .font(level <= 2 ? .headline : .subheadline.bold())
        case .listItem(let marker, let depth):
          HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(marker)
              .monospacedDigit()
              .foregroundStyle(Palette.mutedForeground)
            Text(block.content)
          }
          .padding(.leading, CGFloat(depth - 1) * 16)
        case .code:
          Text(block.content)
            .font(.callout.monospaced())
        case .quote:
          Text(block.content)
            .foregroundStyle(Palette.mutedForeground)
            .padding(.leading, 12)
            .overlay(alignment: .leading) {
              Capsule()
                .fill(Palette.border)
                .frame(width: 3)
            }
        }
      }
    }
    // Links keep the text color and are marked by their underline: the accent color means the
    // user's own words, and it is too faint on the reply bubble in dark mode.
    .tint(Palette.foreground)
  }
}

private struct MarkdownBlock: Identifiable {
  enum Kind: Equatable {
    case paragraph
    case heading(level: Int)
    case listItem(marker: String, depth: Int)
    case code
    case quote
  }

  let id: Int
  let kind: Kind
  let content: AttributedString

  static func parse(_ source: String) -> [MarkdownBlock] {
    let options = AttributedString.MarkdownParsingOptions(
      interpretedSyntax: .full,
      failurePolicy: .returnPartiallyParsedIfPossible
    )
    guard
      let document = try? AttributedString(
        markdown: escapingLoneTildes(source),
        options: options
      )
    else {
      return [MarkdownBlock(id: 0, kind: .paragraph, content: AttributedString(source))]
    }
    return document.runs[\.presentationIntent].enumerated().map { index, run in
      MarkdownBlock(
        id: index, kind: kind(of: run.0),
        content: underliningLinks(AttributedString(document[run.1])))
    }
  }

  /// Apple's parser strikes through text between single tildes, but people write ranges such as
  /// `14~21도` with them. Only `~~` strikes through here; code keeps its tildes as written.
  private static func escapingLoneTildes(_ source: String) -> String {
    var lines: [String] = []
    var isInFence = false
    for line in source.split(separator: "\n", omittingEmptySubsequences: false) {
      let trimmed = line.drop { $0 == " " || $0 == "\t" }
      if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
        isInFence.toggle()
        lines.append(String(line))
      } else {
        lines.append(isInFence ? String(line) : escapingLoneTildes(inLine: line))
      }
    }
    return lines.joined(separator: "\n")
  }

  private static func escapingLoneTildes(inLine line: Substring) -> String {
    let characters = Array(line)
    var output = ""
    var isInCode = false
    for (index, character) in characters.enumerated() {
      if character == "`" { isInCode.toggle() }
      let previous = index > 0 ? characters[index - 1] : nil
      let next = index + 1 < characters.count ? characters[index + 1] : nil
      if character == "~" && !isInCode && previous != "~" && previous != "\\" && next != "~" {
        output.append("\\")
      }
      output.append(character)
    }
    return output
  }

  private static func underliningLinks(_ content: AttributedString) -> AttributedString {
    var content = content
    for run in content.runs where run.link != nil {
      content[run.range].underlineStyle = .single
    }
    return content
  }

  /// Components run from the innermost block outward, so the first list item found is this block's own.
  private static func kind(of intent: PresentationIntent?) -> Kind {
    guard let components = intent?.components else { return .paragraph }
    var ordinal: Int?
    var depth = 0
    var isOrdered = false
    for component in components {
      switch component.kind {
      case .header(let level):
        return .heading(level: level)
      case .codeBlock:
        return .code
      case .blockQuote:
        return .quote
      case .listItem(let value):
        if ordinal == nil { ordinal = value }
        depth += 1
      case .orderedList:
        if depth == 1 { isOrdered = true }
      default:
        break
      }
    }
    guard let ordinal else { return .paragraph }
    return .listItem(marker: isOrdered ? "\(ordinal)." : "•", depth: depth)
  }
}

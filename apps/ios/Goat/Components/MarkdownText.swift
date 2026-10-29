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
              .foregroundStyle(.secondary)
            Text(block.content)
          }
          .padding(.leading, CGFloat(depth - 1) * 16)
        case .code:
          Text(block.content)
            .font(.callout.monospaced())
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.fill.tertiary, in: .rect(cornerRadius: 10))
        case .quote:
          Text(block.content)
            .foregroundStyle(.secondary)
            .padding(.leading, 12)
            .overlay(alignment: .leading) {
              Capsule()
                .fill(.separator)
                .frame(width: 3)
            }
        }
      }
    }
  }
}

struct MarkdownBlock: Identifiable {
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
    guard let document = try? AttributedString(markdown: source, options: options) else {
      return [MarkdownBlock(id: 0, kind: .paragraph, content: AttributedString(source))]
    }
    return document.runs[\.presentationIntent].enumerated().map { index, run in
      MarkdownBlock(id: index, kind: kind(of: run.0), content: AttributedString(document[run.1]))
    }
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

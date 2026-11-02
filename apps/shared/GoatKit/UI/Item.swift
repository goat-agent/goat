import SwiftUI

/// web's `ItemGroup`: rows on one filled surface, with a hairline between them.
struct ItemGroup<Content: View>: View {
  @ViewBuilder let content: Content

  var body: some View {
    VStack(spacing: 0) {
      Group(subviews: content) { rows in
        ForEach(rows.indices, id: \.self) { index in
          if index > 0 {
            Palette.border.frame(height: 0.5)
          }
          rows[index]
        }
      }
    }
    .background(Palette.secondary, in: .rect(cornerRadius: 12, style: .continuous))
  }
}

/// web's `Item`: what the row is, an optional line under it, and what can be done on the right.
struct Item<Leading: View, Trailing: View>: View {
  let title: String
  var description: String?
  @ViewBuilder let leading: Leading
  @ViewBuilder let trailing: Trailing

  var body: some View {
    HStack(spacing: 10) {
      leading
      VStack(alignment: .leading, spacing: 1) {
        Text(verbatim: title)
          .font(.body.weight(.medium))
          .foregroundStyle(Palette.foreground)
        if let description {
          Text(verbatim: description)
            .font(.callout)
            .foregroundStyle(Palette.mutedForeground)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      trailing
    }
    .padding(.horizontal, 12)
    .padding(.vertical, 8)
    .frame(minHeight: 44)
  }
}

extension Item where Leading == EmptyView {
  init(title: String, description: String? = nil, @ViewBuilder trailing: () -> Trailing) {
    self.init(title: title, description: description, leading: { EmptyView() }, trailing: trailing)
  }
}

#Preview {
  ItemGroup {
    Item(title: "GitHub", description: "mino-jeon") {
      Image(systemName: "chevron.left.forwardslash.chevron.right")
    } trailing: {
      Button("More", systemImage: "ellipsis") {}.buttonStyle(.icon)
    }
    Item(title: "Email") {
      Text(verbatim: "mino@example.com").foregroundStyle(Palette.mutedForeground)
    }
  }
  .padding(24)
  .frame(width: 420)
  .background(Palette.background)
}

import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// Everything about one agent that people set when making it: its picture, name, and
/// instructions. Making an agent and changing one use this same form, so an agent always looks
/// the same to edit. The face is the photo button.
struct AgentForm<Avatar: View>: View {
  @Binding var name: String
  @Binding var instructions: String
  let hasPhoto: Bool
  let pickPhoto: (Data) -> Void
  let removePhoto: () -> Void
  @ViewBuilder let avatar: Avatar

  @State private var isPicking = false
  @State private var isShowingPhotoMenu = false
  #if !os(macOS)
    @State private var pickedItem: PhotosPickerItem?
  #endif

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      HStack(alignment: .bottom, spacing: 14) {
        photoButton
        VStack(alignment: .leading, spacing: 6) {
          FieldLabel("Name")
          TextField("Name", text: $name, prompt: Text(verbatim: ""))
            .labelsHidden()
            .submitLabel(.done)
            .inputStyle()
        }
      }
      VStack(alignment: .leading, spacing: 6) {
        FieldLabel("Instructions")
        TextField(
          "Instructions", text: $instructions, prompt: Text("How it should work (optional)"),
          axis: .vertical
        )
        .labelsHidden()
        .lineLimit(5...12)
        .inputStyle()
      }
    }
    #if os(macOS)
      // A Mac picks a picture from its files; the Photos sheet is taller than a small window.
      .fileImporter(isPresented: $isPicking, allowedContentTypes: [.image]) { result in
        guard case .success(let url) = result else { return }
        load { try Data(contentsOf: url) }
      }
    #else
      .photosPicker(isPresented: $isPicking, selection: $pickedItem, matching: .images)
      .onChange(of: pickedItem) {
        guard let item = pickedItem else { return }
        pickedItem = nil
        load { try await item.loadTransferable(type: Data.self) }
      }
    #endif
  }

  private var photoButton: some View {
    Button {
      if hasPhoto { isShowingPhotoMenu = true } else { isPicking = true }
    } label: {
      avatar
        .overlay(alignment: .bottomTrailing) {
          Image(systemName: "photo")
            .font(.system(size: 10, weight: .semibold))
            .foregroundStyle(Palette.mutedForeground)
            .frame(width: 22, height: 22)
            .background(Palette.popover, in: .circle)
            .overlay { Circle().strokeBorder(Palette.border, lineWidth: 0.5) }
            .offset(x: 2, y: 2)
        }
    }
    .buttonStyle(.plain)
    .accessibilityLabel(hasPhoto ? "Change Photo" : "Add Photo")
    .dropdownMenu(isPresented: $isShowingPhotoMenu) {
      DropdownMenuItem(title: "Choose Photo", systemImage: "photo") { isPicking = true }
      DropdownMenuItem(
        title: "Remove Photo", systemImage: "trash", role: .destructive, action: removePhoto)
    }
  }

  private func load(_ data: @escaping () async throws -> Data?) {
    Task {
      guard let data = try? await data(), let photo = await AgentPhoto.prepare(data) else { return }
      pickPhoto(photo)
    }
  }
}

import PhotosUI
import SwiftUI

/// Everything about one agent that people set: its picture, name, and instructions. Making an
/// agent and changing one use this same form, so an agent always looks the same to edit.
struct AgentForm<Avatar: View>: View {
  @Binding var name: String
  @Binding var instructions: String
  let hasPhoto: Bool
  let pickPhoto: (Data) -> Void
  let removePhoto: () -> Void
  @ViewBuilder let avatar: Avatar

  @State private var isPicking = false
  @State private var pickedItem: PhotosPickerItem?

  var body: some View {
    // The picture is the name's header, since a form draws no background behind headers on
    // either iPhone or Mac.
    Section {
      TextField("Name", text: $name, prompt: Text("Name"))
        .labelsHidden()
        .submitLabel(.done)
    } header: {
      photoButton
        .frame(maxWidth: .infinity)
        .padding(.bottom, 16)
        .textCase(nil)
    }
    .photosPicker(isPresented: $isPicking, selection: $pickedItem, matching: .images)
    .onChange(of: pickedItem) { loadPickedPhoto() }

    Section("Instructions") {
      TextField(
        "Instructions",
        text: $instructions,
        prompt: Text("Optional"),
        axis: .vertical
      )
      .lineLimit(4...12)
      .labelsHidden()
    }
  }

  private var photoButton: some View {
    Group {
      if hasPhoto {
        Menu {
          Button("Choose Photo", systemImage: "photo") { isPicking = true }
          Button("Remove Photo", systemImage: "trash", role: .destructive, action: removePhoto)
        } label: {
          avatar
        }
        .menuStyle(.button)
        .menuIndicator(.hidden)
      } else {
        Button {
          isPicking = true
        } label: {
          avatar
        }
      }
    }
    .buttonStyle(.plain)
    .accessibilityLabel(hasPhoto ? "Change Photo" : "Add Photo")
  }

  private func loadPickedPhoto() {
    guard let item = pickedItem else { return }
    pickedItem = nil
    Task {
      guard let data = try? await item.loadTransferable(type: Data.self),
        let photo = await AgentPhoto.prepare(data)
      else { return }
      pickPhoto(photo)
    }
  }
}

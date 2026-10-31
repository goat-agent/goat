import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

extension View {
  /// The photo and file pickers behind the composer's + menu; what is picked joins the draft.
  func attachmentPickers(
    for conversation: ConversationModel, isPickingPhotos: Binding<Bool>,
    isPickingFiles: Binding<Bool>
  ) -> some View {
    modifier(
      AttachmentPickers(
        conversation: conversation, isPickingPhotos: isPickingPhotos,
        isPickingFiles: isPickingFiles))
  }
}

private struct AttachmentPickers: ViewModifier {
  let conversation: ConversationModel
  @Binding var isPickingPhotos: Bool
  @Binding var isPickingFiles: Bool

  @State private var pickedPhotos: [PhotosPickerItem] = []

  func body(content: Content) -> some View {
    content
      .photosPicker(
        isPresented: $isPickingPhotos,
        selection: $pickedPhotos,
        maxSelectionCount: max(
          1, ConversationModel.maxAttachments - conversation.attachments.count),
        matching: .images
      )
      .onChange(of: pickedPhotos) { attachPhotos() }
      .fileImporter(
        isPresented: $isPickingFiles,
        allowedContentTypes: [.image, .pdf, .plainText, .commaSeparatedText],
        allowsMultipleSelection: true,
        onCompletion: attachFiles
      )
  }

  private func attachPhotos() {
    let items = pickedPhotos
    guard !items.isEmpty else { return }
    pickedPhotos = []
    for item in items {
      Task {
        guard let data = try? await item.loadTransferable(type: Data.self),
          let photo = await PreparedFile.photo(data)
        else { return }
        await conversation.attach(photo)
      }
    }
  }

  private func attachFiles(_ result: Result<[URL], any Error>) {
    guard case .success(let urls) = result else { return }
    for url in urls {
      Task {
        guard let file = try? await PreparedFile.file(at: url) else { return }
        await conversation.attach(file)
      }
    }
  }
}

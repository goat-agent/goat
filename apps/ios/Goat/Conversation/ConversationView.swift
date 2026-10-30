import AuthenticationServices
import GoatAPI
import PhotosUI
import QuickLook
import SwiftUI
import UniformTypeIdentifiers

struct ConversationView: View {
  @State private var conversation: ConversationModel
  @State private var integrations: IntegrationsModel
  @State private var position = ScrollPosition(edge: .bottom)
  @State private var isAtBottom = true
  @State private var isPickingPhotos = false
  @State private var isPickingFiles = false
  @State private var pickedPhotos: [PhotosPickerItem] = []
  @State private var previewURL: URL?
  @Environment(\.webAuthenticationSession) private var webAuthenticationSession

  init(agent: Components.Schemas.Agent, client: Client) {
    _conversation = State(initialValue: ConversationModel(agent: agent, client: client))
    _integrations = State(initialValue: IntegrationsModel(client: client))
  }

  var body: some View {
    ScrollView {
      LazyVStack(spacing: 6) {
        ForEach(Array(conversation.messages.enumerated()), id: \.element.id) { index, message in
          if let time = gapBefore(index) {
            Text(time, format: .dateTime.month().day().hour().minute())
              .font(.caption)
              .foregroundStyle(.secondary)
              .padding(.top, 12)
          }
          ForEach(message.files ?? [], id: \.id) { file in
            SentFile(file: file, load: conversation.localURL(for:), open: preview)
          }
          if !message.text.isEmpty {
            MessageBubble(text: message.text, isMine: message.role.value1 == .user)
          }
          ForEach(Array((message.connect ?? []).enumerated()), id: \.offset) { _, request in
            ConnectCard(
              name: request.name,
              iconURL: integrations.iconURL(of: request.integrationId),
              isConnected: integrations.isConnected(request.integrationId),
              isConnecting: integrations.connectingID == request.integrationId,
              isEnabled: integrations.connectingID == nil,
              didFail: integrations.failedIntegrationID == request.integrationId
            ) {
              connect(request.integrationId)
            }
          }
        }
        ForEach(conversation.outgoing) { message in
          OutgoingBubble(message: message, load: conversation.localURL(for:), open: preview) {
            Task { await conversation.retry(message.id) }
          } discard: {
            conversation.discard(message.id)
          }
        }
        if conversation.isRunning {
          TypingIndicator()
            .transition(.opacity)
        }
      }
      .animation(.default, value: conversation.isRunning)
      .padding(.horizontal)
      .padding(.vertical, 8)
    }
    .scrollPosition($position)
    .defaultScrollAnchor(.bottom)
    .scrollDismissesKeyboard(.interactively)
    .onScrollGeometryChange(for: Bool.self) { geometry in
      geometry.visibleRect.maxY >= geometry.contentSize.height - 40
    } action: { _, atBottom in
      isAtBottom = atBottom
    }
    .onChange(of: conversation.messages.count) {
      if isAtBottom { scrollToLatest() }
      if conversation.messages.last?.role.value1 == .assistant {
        AccessibilityNotification.Announcement("새 답이 왔어요").post()
      }
    }
    .onChange(of: conversation.outgoing.count) { scrollToLatest() }
    .onChange(of: conversation.isRunning) {
      if isAtBottom { scrollToLatest() }
    }
    .overlay(alignment: .bottomTrailing) {
      if !isAtBottom {
        Button("최신으로", systemImage: "arrow.down", action: scrollToLatest)
          .labelStyle(.iconOnly)
          .buttonStyle(.glass)
          .buttonBorderShape(.circle)
          .padding()
          .transition(.scale.combined(with: .opacity))
      }
    }
    .animation(.default, value: isAtBottom)
    .safeAreaBar(edge: .bottom) {
      Composer(
        draft: $conversation.draft,
        attachments: conversation.attachments,
        canAttach: conversation.canAttach,
        canSend: conversation.canSend,
        pickPhotos: { isPickingPhotos = true },
        pickFiles: { isPickingFiles = true },
        open: { preview($0.localURL) },
        retry: { id in Task { await conversation.retryUpload(id) } },
        remove: conversation.removeAttachment,
        send: send
      )
    }
    .photosPicker(
      isPresented: $isPickingPhotos,
      selection: $pickedPhotos,
      maxSelectionCount: max(1, ConversationModel.maxAttachments - conversation.attachments.count),
      matching: .images
    )
    .onChange(of: pickedPhotos) { attachPhotos() }
    .fileImporter(
      isPresented: $isPickingFiles,
      allowedContentTypes: [.image, .pdf, .plainText, .commaSeparatedText],
      allowsMultipleSelection: true,
      onCompletion: attachFiles
    )
    .quickLookPreview($previewURL)
    .navigationTitle(conversation.agent.name)
    .navigationBarTitleDisplayMode(.inline)
    .task { await integrations.load() }
    .task { await conversation.listen() }
  }

  private func send() {
    Task { await conversation.send() }
  }

  private func preview(_ url: URL) {
    previewURL = url
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

  private func connect(_ integrationID: String) {
    let connector = Connector(client: conversation.client, session: webAuthenticationSession)
    Task {
      await integrations.connect(integrationID, agentID: conversation.agent.id, with: connector)
    }
  }

  private func scrollToLatest() {
    withAnimation { position.scrollTo(edge: .bottom) }
  }

  /// The time to show above a message that starts a new stretch of the conversation.
  private func gapBefore(_ index: Int) -> Date? {
    let message = conversation.messages[index]
    guard index > 0 else { return message.createdAt }
    let previous = conversation.messages[index - 1]
    return message.createdAt.timeIntervalSince(previous.createdAt) > 60 * 60
      ? message.createdAt : nil
  }
}

private struct Composer: View {
  @Binding var draft: String
  let attachments: [ConversationModel.Attachment]
  let canAttach: Bool
  let canSend: Bool
  let pickPhotos: () -> Void
  let pickFiles: () -> Void
  let open: (ConversationModel.Attachment) -> Void
  let retry: (ConversationModel.Attachment.ID) -> Void
  let remove: (ConversationModel.Attachment.ID) -> Void
  let send: () -> Void

  var body: some View {
    GlassEffectContainer {
      VStack(alignment: .leading, spacing: 8) {
        if !attachments.isEmpty {
          ScrollView(.horizontal) {
            HStack(spacing: 10) {
              ForEach(attachments) { attachment in
                AttachmentChip(attachment: attachment) {
                  open(attachment)
                } retry: {
                  retry(attachment.id)
                } remove: {
                  remove(attachment.id)
                }
              }
            }
            .padding(.top, 8)
            .padding(.trailing, 8)
          }
          .scrollIndicators(.hidden)
        }
        HStack(alignment: .bottom, spacing: 8) {
          Menu {
            Button("사진", systemImage: "photo.on.rectangle", action: pickPhotos)
            Button("파일", systemImage: "doc", action: pickFiles)
          } label: {
            Label("첨부", systemImage: "plus")
          }
          .labelStyle(.iconOnly)
          .font(.body.weight(.semibold))
          .buttonStyle(.glass)
          .buttonBorderShape(.circle)
          .controlSize(.large)
          .disabled(!canAttach)
          messageField
        }
      }
    }
    .padding(.horizontal)
    .padding(.bottom, 8)
  }

  private var messageField: some View {
    HStack(alignment: .bottom, spacing: 8) {
      TextField("메시지", text: $draft, axis: .vertical)
        .lineLimit(1...5)
        .padding(.horizontal, 16)
        .padding(.vertical, 11)
        .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 22))
      Button("보내기", systemImage: "arrow.up", action: send)
        .labelStyle(.iconOnly)
        .font(.body.weight(.semibold))
        .buttonStyle(.glassProminent)
        .buttonBorderShape(.circle)
        .controlSize(.large)
        .disabled(!canSend)
    }
  }
}

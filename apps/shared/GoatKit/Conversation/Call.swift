import AVFoundation
import Foundation
import GoatAPI
import Observation
import WebRTC

/// A voice call with an agent. The device sends and hears audio over plain WebRTC; the server
/// picks the voice service, so the app knows nothing about who is on the other end.
@Observable
final class Call: Identifiable {
  enum State: Equatable {
    case connecting
    case connected(since: Date)
    case failed(Problem)
    case ended
  }

  enum Problem: Equatable {
    case microphoneDenied
    case unreachable
  }

  private(set) var state = State.connecting
  private(set) var isMuted = false
  /// Calls start on the speaker, as `configureAudio` sets; headphones take over when connected.
  private(set) var isOnSpeaker = true

  private let agentID: String
  private let client: Client
  @ObservationIgnored private var connection: RTCPeerConnection?
  @ObservationIgnored private var microphone: RTCAudioTrack?
  @ObservationIgnored private var observer: ConnectionObserver?

  private static let factory: RTCPeerConnectionFactory = {
    RTCInitializeSSL()
    return RTCPeerConnectionFactory()
  }()

  init(agentID: String, client: Client) {
    self.agentID = agentID
    self.client = client
  }

  func start() async {
    guard state == .connecting else { return }
    guard await AVAudioApplication.requestRecordPermission() else {
      state = .failed(.microphoneDenied)
      return
    }
    do {
      try configureAudio()
      let connection = try makeConnection()
      let offer = try await connection.offer(for: Self.constraints)
      try await connection.setLocalDescription(offer)
      let output = try await client.startCall(
        path: .init(id: agentID),
        body: .json(.init(sdp: offer.sdp))
      )
      let answer = try output.created.body.json.sdp
      try await connection.setRemoteDescription(RTCSessionDescription(type: .answer, sdp: answer))
    } catch {
      fail()
    }
  }

  func toggleMute() {
    isMuted.toggle()
    microphone?.isEnabled = !isMuted
  }

  #if os(iOS)
    func toggleSpeaker() {
      let session = RTCAudioSession.sharedInstance()
      session.lockForConfiguration()
      defer { session.unlockForConfiguration() }
      do {
        try session.overrideOutputAudioPort(isOnSpeaker ? .none : .speaker)
        isOnSpeaker.toggle()
      } catch {
        // The route stays as it was, and so does the button.
      }
    }
  #endif

  func end() {
    connection?.close()
    connection = nil
    microphone = nil
    observer = nil
    #if os(iOS)
      try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    #endif
    if case .failed = state { return }
    state = .ended
  }

  private static let constraints = RTCMediaConstraints(
    mandatoryConstraints: nil,
    optionalConstraints: nil
  )

  /// The Mac has no audio session; WebRTC uses the system's input and output there.
  private func configureAudio() throws {
    #if os(iOS)
      let configuration = RTCAudioSessionConfiguration.webRTC()
      configuration.category = AVAudioSession.Category.playAndRecord.rawValue
      configuration.mode = AVAudioSession.Mode.voiceChat.rawValue
      configuration.categoryOptions = [.defaultToSpeaker, .allowBluetoothHFP]
      RTCAudioSessionConfiguration.setWebRTC(configuration)
    #endif
  }

  private func makeConnection() throws -> RTCPeerConnection {
    let configuration = RTCConfiguration()
    configuration.sdpSemantics = .unifiedPlan
    let observer = ConnectionObserver { [weak self] state in
      Task { @MainActor in self?.apply(state) }
    }
    guard
      let connection = Self.factory.peerConnection(
        with: configuration,
        constraints: Self.constraints,
        delegate: observer
      )
    else { throw URLError(.cannotConnectToHost) }
    let source = Self.factory.audioSource(with: Self.constraints)
    let microphone = Self.factory.audioTrack(with: source, trackId: "microphone")
    connection.add(microphone, streamIds: ["call"])
    self.connection = connection
    self.microphone = microphone
    self.observer = observer
    return connection
  }

  private func apply(_ connectionState: RTCPeerConnectionState) {
    switch connectionState {
    case .connected:
      if case .connecting = state { state = .connected(since: .now) }
    case .failed:
      fail()
    case .closed, .disconnected:
      if case .connected = state { end() }
    default:
      break
    }
  }

  private func fail() {
    state = .failed(.unreachable)
    end()
  }
}

/// Receives the connection's callbacks on WebRTC's own thread and passes on its state.
nonisolated private final class ConnectionObserver: NSObject, RTCPeerConnectionDelegate {
  let onChange: @Sendable (RTCPeerConnectionState) -> Void

  init(onChange: @escaping @Sendable (RTCPeerConnectionState) -> Void) {
    self.onChange = onChange
  }

  func peerConnection(
    _ peerConnection: RTCPeerConnection,
    didChange newState: RTCPeerConnectionState
  ) {
    onChange(newState)
  }

  func peerConnection(
    _ peerConnection: RTCPeerConnection,
    didChange stateChanged: RTCSignalingState
  ) {}

  func peerConnection(_ peerConnection: RTCPeerConnection, didAdd stream: RTCMediaStream) {}

  func peerConnection(
    _ peerConnection: RTCPeerConnection,
    didRemove stream: RTCMediaStream
  ) {}

  func peerConnectionShouldNegotiate(_ peerConnection: RTCPeerConnection) {}

  func peerConnection(
    _ peerConnection: RTCPeerConnection,
    didChange newState: RTCIceConnectionState
  ) {}

  func peerConnection(
    _ peerConnection: RTCPeerConnection,
    didChange newState: RTCIceGatheringState
  ) {}

  func peerConnection(
    _ peerConnection: RTCPeerConnection,
    didGenerate candidate: RTCIceCandidate
  ) {}

  func peerConnection(
    _ peerConnection: RTCPeerConnection,
    didRemove candidates: [RTCIceCandidate]
  ) {}

  func peerConnection(
    _ peerConnection: RTCPeerConnection,
    didOpen dataChannel: RTCDataChannel
  ) {}
}

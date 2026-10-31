import AVFoundation
import Foundation
import GoatAPI
import Observation

/// Records what the user says and turns it into text for the composer. The server keeps no
/// audio, so the recording stays here until its text arrives and a failure can be tried again.
@Observable
final class Dictation {
  enum State {
    case idle
    case recording
    case transcribing
  }

  enum Problem {
    case microphoneDenied
    case transcriptionFailed
  }

  /// A forgotten recording stops by itself, and what it holds is still transcribed.
  static let maxDuration: TimeInterval = 10 * 60

  private static let levelCount = 48

  private(set) var state = State.idle
  private(set) var problem: Problem?
  /// Recent input levels from 0 to 1, oldest first.
  private(set) var levels = [Float](repeating: 0, count: levelCount)
  /// Counts recordings that produced no text, so the view can play a haptic for each.
  private(set) var missCount = 0

  @ObservationIgnored var onText: (String) -> Void = { _ in }

  private let agentID: String
  private let client: Client
  @ObservationIgnored private var recorder: AVAudioRecorder?
  @ObservationIgnored private var recording: URL?
  @ObservationIgnored private var metering: Task<Void, Never>?

  init(agentID: String, client: Client) {
    self.agentID = agentID
    self.client = client
  }

  func start() async {
    guard state == .idle else { return }
    problem = nil
    guard await AVAudioApplication.requestRecordPermission() else {
      problem = .microphoneDenied
      return
    }
    discardRecording()
    let url = FileManager.default.temporaryDirectory
      .appending(path: "dictation-\(UUID().uuidString).m4a")
    do {
      #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.record, mode: .default)
        try session.setActive(true)
      #endif
      let recorder = try AVAudioRecorder(
        url: url,
        settings: [
          AVFormatIDKey: kAudioFormatMPEG4AAC,
          AVSampleRateKey: 16_000,
          AVNumberOfChannelsKey: 1,
          AVEncoderBitRateKey: 32_000,
        ]
      )
      recorder.isMeteringEnabled = true
      guard recorder.record(forDuration: Self.maxDuration) else {
        throw CocoaError(.fileWriteUnknown)
      }
      self.recorder = recorder
      recording = url
    } catch {
      try? FileManager.default.removeItem(at: url)
      problem = .transcriptionFailed
      return
    }
    state = .recording
    levels = [Float](repeating: 0, count: Self.levelCount)
    metering = Task { await meter() }
  }

  func stop() async {
    guard state == .recording else { return }
    metering?.cancel()
    recorder?.stop()
    recorder = nil
    deactivateAudio()
    await transcribe()
  }

  /// Throws the recording away instead of turning it into text.
  func cancel() {
    guard state == .recording else { return }
    metering?.cancel()
    recorder?.stop()
    recorder = nil
    deactivateAudio()
    discardRecording()
    state = .idle
  }

  func retry() async {
    guard state == .idle, recording != nil else { return }
    await transcribe()
  }

  func dismissProblem() {
    problem = nil
  }

  private func transcribe() async {
    guard let recording else { return }
    state = .transcribing
    problem = nil
    defer { state = .idle }

    let text: String
    do {
      let data = try Data(contentsOf: recording)
      let output = try await client.transcribeAudio(
        path: .init(id: agentID),
        body: .recording(data)
      )
      text = try output.ok.body.json.text.trimmingCharacters(in: .whitespacesAndNewlines)
    } catch {
      problem = .transcriptionFailed
      missCount += 1
      return
    }
    discardRecording()
    if text.isEmpty {
      missCount += 1
    } else {
      onText(text)
    }
  }

  private func meter() async {
    while !Task.isCancelled, let recorder {
      guard recorder.isRecording else {
        await stop()
        return
      }
      recorder.updateMeters()
      // averagePower is in decibels, about -160 for silence and 0 at full scale.
      let level = max(0, min(1, (recorder.averagePower(forChannel: 0) + 50) / 50))
      levels.removeFirst()
      levels.append(level)
      try? await Task.sleep(for: .milliseconds(50))
    }
  }

  private func deactivateAudio() {
    #if os(iOS)
      try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    #endif
  }

  private func discardRecording() {
    if let recording { try? FileManager.default.removeItem(at: recording) }
    recording = nil
  }
}

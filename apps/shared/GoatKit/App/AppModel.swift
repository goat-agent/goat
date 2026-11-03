import Foundation
import GoatAPI
import Observation

@Observable
final class AppModel {
  enum Screen: Equatable {
    case loading
    case signIn
    /// Signed in with no agent yet: the first agent is the last step of signing up.
    case setup
    case home
  }

  private(set) var screen = Screen.loading
  /// The email whose code screen is pushed onto the sign-in flow.
  var signInPath: [String] = []
  /// Newest activity first, as the server orders them.
  private(set) var agents: [Components.Schemas.Agent] = []
  /// Agent ids, so a pushed conversation always shows the agent as it is now.
  var agentPath: [String] = []
  var isCreatingAgent = false
  private(set) var me: Components.Schemas.User?
  /// False while the live agent list is reconnecting.
  private(set) var isOnline = true
  private(set) var isWorking = false
  private(set) var errorMessage: String?
  /// Bumped on every rejected code and every sign-in, to drive haptics.
  private(set) var failedAttempts = 0
  private(set) var signIns = 0

  let client: Client
  let serverURL: URL
  private let tokens: TokenStore
  private let notifications = Notifications()
  /// A tapped notification that arrived before the agent list was loaded.
  private var pendingAgentID: String?

  /// The session token, for connections the generated client does not make, such as the device
  /// socket.
  var sessionToken: String? { tokens.load() }

  init(serverURL: URL = .goatServer, tokens: TokenStore = TokenStore()) {
    self.serverURL = serverURL
    self.tokens = tokens
    client = .goat(serverURL: serverURL, token: { tokens.load() })
    notifications.visibleAgentID = { [weak self] in self?.agentPath.last }
    notifications.open = { [weak self] in self?.openAgent(id: $0) }
  }

  var userName: String { me?.name ?? "" }

  func agent(_ id: String) -> Components.Schemas.Agent? {
    agents.first { $0.id == id }
  }

  func start() async {
    await perform { try await self.route() }
  }

  func sendCode(to email: String) async {
    await perform {
      switch try await self.client.sendCode(body: .json(.init(email: email))) {
      case .noContent:
        if self.signInPath.last != email { self.signInPath = [email] }
      case .badRequest:
        self.errorMessage = String(localized: "Check your email address.")
      case .undocumented(let status, _):
        throw UnexpectedStatus(code: status)
      }
    }
  }

  func signIn(email: String, code: String) async {
    await perform {
      let body = Operations.SignIn.Input.Body.json(
        .init(email: email, code: code, device: CurrentDevice.signingIn)
      )
      switch try await self.client.signIn(body: body) {
      case .ok(let response):
        self.tokens.save(try response.body.json.token)
        self.signIns += 1
        try await self.route()
      case .badRequest, .unauthorized:
        self.failedAttempts += 1
        self.errorMessage = String(localized: "That code is wrong or has expired.")
      case .undocumented(let status, _):
        throw UnexpectedStatus(code: status)
      }
    }
  }

  func rename(to name: String) async {
    guard case .ok(let response) = try? await client.updateMe(body: .json(.init(name: name))),
      let user = try? response.body.json
    else { return }
    me = user
  }

  /// Opens the new agent's conversation, so creating one always lands where it is used.
  func createAgent(name: String, instructions: String, photo: Data?) async {
    await perform {
      let body = Operations.CreateAgent.Input.Body.json(
        .init(name: name, instructions: instructions)
      )
      switch try await self.client.createAgent(body: body) {
      case .created(let response):
        var agent = try response.body.json.agent
        if let photo, let withPhoto = await self.upload(photo, for: agent.id) { agent = withPhoto }
        self.replace(agent)
        self.agentPath = [agent.id]
        self.isCreatingAgent = false
        self.screen = .home
      case .badRequest:
        self.errorMessage = String(localized: "Check what you entered.")
      case .unauthorized:
        self.clearSession()
      case .undocumented(let status, _):
        throw UnexpectedStatus(code: status)
      }
    }
  }

  func update(_ agentID: String, name: String? = nil, instructions: String? = nil) async {
    let body = Operations.UpdateAgent.Input.Body.json(.init(name: name, instructions: instructions))
    guard
      case .ok(let response) = try? await client.updateAgent(path: .init(id: agentID), body: body),
      let agent = try? response.body.json.agent
    else { return }
    replace(agent)
  }

  func setPhoto(_ photo: Data, of agentID: String) async {
    if let agent = await upload(photo, for: agentID) { replace(agent) }
  }

  func removePhoto(of agentID: String) async {
    guard case .ok(let response) = try? await client.removeAgentAvatar(path: .init(id: agentID)),
      let agent = try? response.body.json.agent
    else { return }
    replace(agent)
  }

  func delete(_ agentID: String) async {
    guard case .noContent = try? await client.deleteAgent(path: .init(id: agentID)) else { return }
    agentPath.removeAll { $0 == agentID }
    agents.removeAll { $0.id == agentID }
    if UserDefaults.standard.string(forKey: Self.lastAgentKey) == agentID {
      UserDefaults.standard.removeObject(forKey: Self.lastAgentKey)
    }
  }

  /// Keeps `agents` current until the task is cancelled, reconnecting with backoff.
  func listenToAgents() async {
    var delay = Duration.seconds(1)
    while !Task.isCancelled {
      do {
        let output = try await client.streamAgents()
        for try await event in try output.ok.body.textEventStream.agentListEvents() {
          isOnline = true
          delay = .seconds(1)
          if event._type.value1 == .agents, let list = event.agents { agents = list }
        }
      } catch is CancellationError {
        return
      } catch {
        isOnline = false
        delay = min(delay * 2, .seconds(30))
      }
      try? await Task.sleep(for: delay)
    }
  }

  func open(_ agentID: String) {
    UserDefaults.standard.set(agentID, forKey: Self.lastAgentKey)
  }

  private static let lastAgentKey = "lastAgentID"

  func askForNotifications() async {
    await notifications.requestIfNotDetermined()
  }

  func registerPushToken(_ token: Data) async {
    let hex = token.map { String(format: "%02x", $0) }.joined()
    _ = try? await client.registerPushToken(body: .json(.init(token: hex)))
  }

  private func openAgent(id: String) {
    guard screen == .home else {
      pendingAgentID = id
      return
    }
    if agents.contains(where: { $0.id == id }) { agentPath = [id] }
  }

  func signOut() async {
    _ = try? await client.signOut()
    clearSession()
  }

  private func upload(_ photo: Data, for agentID: String) async -> Components.Schemas.Agent? {
    let output = try? await client.setAgentAvatar(path: .init(id: agentID), body: .jpeg(photo))
    guard case .ok(let response) = output else { return nil }
    return try? response.body.json.agent
  }

  private func replace(_ agent: Components.Schemas.Agent) {
    if let index = agents.firstIndex(where: { $0.id == agent.id }) {
      agents[index] = agent
    } else {
      agents.insert(agent, at: 0)
    }
  }

  private func clearSession() {
    tokens.clear()
    me = nil
    UserDefaults.standard.removeObject(forKey: Self.lastAgentKey)
    pendingAgentID = nil
    signInPath = []
    agents = []
    agentPath = []
    isCreatingAgent = false
    screen = .signIn
  }

  private func route() async throws {
    guard tokens.load() != nil else { return clearSession() }

    switch try await client.getMe() {
    case .ok(let response):
      me = try response.body.json
      if me?.timeZone == nil { await fillTimeZone() }
    case .unauthorized:
      return clearSession()
    case .undocumented(let status, _):
      throw UnexpectedStatus(code: status)
    }

    switch try await client.listAgents() {
    case .ok(let response):
      agents = try response.body.json.agents
      let lastID = pendingAgentID ?? UserDefaults.standard.string(forKey: Self.lastAgentKey)
      pendingAgentID = nil
      if let agent = agents.first(where: { $0.id == lastID }) ?? agents.first {
        agentPath = [agent.id]
        screen = .home
        await notifications.registerIfAuthorized()
      } else {
        screen = .setup
      }
    case .unauthorized:
      clearSession()
    case .undocumented(let status, _):
      throw UnexpectedStatus(code: status)
    }
  }

  private func fillTimeZone() async {
    _ = try? await client.updateMe(body: .json(.init(timeZone: TimeZone.current.identifier)))
  }

  private func perform(_ work: () async throws -> Void) async {
    isWorking = true
    errorMessage = nil
    defer { isWorking = false }
    do {
      try await work()
    } catch {
      errorMessage = String(localized: "Couldn’t reach the server. Try again in a moment.")
    }
  }
}

private struct UnexpectedStatus: Error {
  let code: Int
}

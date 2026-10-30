import Foundation
import GoatAPI
import Observation

@Observable
final class AppModel {
  enum Screen: Equatable {
    case loading
    case signIn
    case setup
    case home
  }

  enum SetupStep: Hashable {
    case agent
  }

  private(set) var screen = Screen.loading
  /// The email whose code screen is pushed onto the sign-in flow.
  var signInPath: [String] = []
  var setupPath: [SetupStep] = []
  private(set) var agents: [Components.Schemas.Agent] = []
  var agentPath: [Components.Schemas.Agent] = []
  var isCreatingAgent = false
  private(set) var userName = ""
  private(set) var needsName = false
  private(set) var isWorking = false
  private(set) var errorMessage: String?
  /// Bumped on every rejected code and every sign-in, to drive haptics.
  private(set) var failedAttempts = 0
  private(set) var signIns = 0

  let client: Client
  private let tokens: TokenStore

  init(serverURL: URL = .goatServer, tokens: TokenStore = TokenStore()) {
    self.tokens = tokens
    client = .goat(serverURL: serverURL, token: { tokens.load() })
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
      switch try await self.client.signIn(body: .json(.init(email: email, code: code))) {
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

  func setName(_ name: String) async {
    await perform {
      switch try await self.client.updateMe(body: .json(.init(name: name))) {
      case .ok:
        self.userName = name
        self.setupPath = [.agent]
      case .badRequest:
        self.errorMessage = String(localized: "Check your name.")
      case .unauthorized:
        self.clearSession()
      case .undocumented(let status, _):
        throw UnexpectedStatus(code: status)
      }
    }
  }

  func createAgent(name: String, instructions: String) async {
    await perform {
      let body = Operations.CreateAgent.Input.Body.json(
        .init(name: name, instructions: instructions)
      )
      switch try await self.client.createAgent(body: body) {
      case .created(let response):
        let agent = try response.body.json.agent
        self.agents.append(agent)
        self.agentPath = [agent]
        self.isCreatingAgent = false
        self.screen = .home
      case .badRequest:
        self.errorMessage = String(localized: "Check what you entered.")
      case .conflict:
        self.needsName = true
        self.setupPath = []
      case .unauthorized:
        self.clearSession()
      case .undocumented(let status, _):
        throw UnexpectedStatus(code: status)
      }
    }
  }

  func open(_ agent: Components.Schemas.Agent) {
    UserDefaults.standard.set(agent.id, forKey: Self.lastAgentKey)
  }

  private static let lastAgentKey = "lastAgentID"

  func signOut() async {
    _ = try? await client.signOut()
    clearSession()
  }

  private func clearSession() {
    tokens.clear()
    userName = ""
    UserDefaults.standard.removeObject(forKey: Self.lastAgentKey)
    signInPath = []
    setupPath = []
    agents = []
    agentPath = []
    isCreatingAgent = false
    screen = .signIn
  }

  private func route() async throws {
    guard tokens.load() != nil else { return clearSession() }

    switch try await client.getMe() {
    case .ok(let response):
      let me = try response.body.json
      userName = me.name
      if me.timeZone == nil { await fillTimeZone() }
      if me.name.isEmpty {
        needsName = true
        setupPath = []
        screen = .setup
        return
      }
    case .unauthorized:
      return clearSession()
    case .undocumented(let status, _):
      throw UnexpectedStatus(code: status)
    }

    switch try await client.listAgents() {
    case .ok(let response):
      agents = try response.body.json.agents
      let lastID = UserDefaults.standard.string(forKey: Self.lastAgentKey)
      if let agent = agents.first(where: { $0.id == lastID }) ?? agents.first {
        agentPath = [agent]
        screen = .home
      } else {
        needsName = false
        setupPath = []
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

extension URL {
  static let goatServer: URL = {
    guard let value = Bundle.main.object(forInfoDictionaryKey: "GoatServerURL") as? String,
      let url = URL(string: value)
    else { fatalError("Set GOAT_SERVER_URL in the build settings.") }
    return url
  }()
}

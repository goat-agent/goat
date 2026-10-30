import Foundation
import GoatAPI
import Observation

@Observable
final class AppModel {
  enum Screen: Equatable {
    case loading
    case signIn
    case setup
    case conversation(Components.Schemas.Agent)
  }

  enum SetupStep: Hashable {
    case agent
  }

  private(set) var screen = Screen.loading
  /// The email whose code screen is pushed onto the sign-in flow.
  var signInPath: [String] = []
  var setupPath: [SetupStep] = []
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
        self.errorMessage = "이메일 주소를 확인해 주세요."
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
        self.errorMessage = "코드가 맞지 않거나 만료됐어요."
      case .undocumented(let status, _):
        throw UnexpectedStatus(code: status)
      }
    }
  }

  func setName(_ name: String) async {
    await perform {
      switch try await self.client.updateMe(body: .json(.init(name: name))) {
      case .ok:
        self.setupPath = [.agent]
      case .badRequest:
        self.errorMessage = "이름을 확인해 주세요."
      case .unauthorized:
        self.signOut()
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
        self.screen = .conversation(try response.body.json.agent)
      case .badRequest:
        self.errorMessage = "입력한 내용을 확인해 주세요."
      case .conflict:
        self.needsName = true
        self.setupPath = []
      case .unauthorized:
        self.signOut()
      case .undocumented(let status, _):
        throw UnexpectedStatus(code: status)
      }
    }
  }

  private func signOut() {
    tokens.clear()
    signInPath = []
    setupPath = []
    screen = .signIn
  }

  private func route() async throws {
    guard tokens.load() != nil else { return signOut() }

    switch try await client.getMe() {
    case .ok(let response):
      let me = try response.body.json
      if me.timeZone == nil { await fillTimeZone() }
      if me.name.isEmpty {
        needsName = true
        setupPath = []
        screen = .setup
        return
      }
    case .unauthorized:
      return signOut()
    case .undocumented(let status, _):
      throw UnexpectedStatus(code: status)
    }

    switch try await client.listAgents() {
    case .ok(let response):
      if let agent = try response.body.json.agents.first {
        screen = .conversation(agent)
      } else {
        needsName = false
        setupPath = []
        screen = .setup
      }
    case .unauthorized:
      signOut()
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
      errorMessage = "서버에 연결하지 못했어요. 잠시 후 다시 시도해 주세요."
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

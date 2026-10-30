import Foundation
import HTTPTypes
import OpenAPIRuntime
import OpenAPIURLSession

/// The server writes dates with `Date.toISOString()`, which includes milliseconds.
private let dateTranscoder: any DateTranscoder = .iso8601WithFractionalSeconds

extension Client {
  /// A client that sends the token returned by `token()` as a bearer token on every request.
  public static func goat(serverURL: URL, token: @escaping @Sendable () -> String?) -> Client {
    Client(
      serverURL: serverURL,
      configuration: Configuration(dateTranscoder: dateTranscoder),
      transport: URLSessionTransport(),
      middlewares: [BearerToken(token: token)]
    )
  }
}

struct BearerToken: ClientMiddleware {
  let token: @Sendable () -> String?

  func intercept(
    _ request: HTTPRequest,
    body: HTTPBody?,
    baseURL: URL,
    operationID: String,
    next: @Sendable (HTTPRequest, HTTPBody?, URL) async throws -> (HTTPResponse, HTTPBody?)
  ) async throws -> (HTTPResponse, HTTPBody?) {
    var request = request
    if let token = token() {
      request.headerFields[.authorization] = "Bearer \(token)"
    }
    return try await next(request, body, baseURL)
  }
}

extension HTTPBody {
  /// The agent events in this stream. Their JSON is decoded outside `Client`, so the date format is set again here.
  public func agentEvents() -> some AsyncSequence<Components.Schemas.AgentEvent, any Error> {
    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .custom { decoder in
      try dateTranscoder.decode(decoder.singleValueContainer().decode(String.self))
    }
    return asDecodedServerSentEventsWithJSONData(
      of: Components.Schemas.AgentEvent.self,
      decoder: decoder
    )
    .compactMap(\.data)
  }
}

extension Operations.UploadFile.Input.Body {
  /// The server tells the type from the bytes and the name, so the name should keep its extension.
  public static func file(_ data: Data, named name: String) -> Self {
    .multipartForm([.file(.init(payload: .init(body: HTTPBody(data)), filename: name))])
  }
}

extension Client {
  /// The server keeps files to 32 MB; the cap only stops a runaway body.
  private static let maxFileBytes = 64 * 1024 * 1024

  public func fileData(agentID: String, fileID: String) async throws -> Data {
    let output = try await downloadFile(path: .init(id: agentID, fileId: fileID))
    return try await Data(collecting: output.ok.body.any, upTo: Self.maxFileBytes)
  }
}

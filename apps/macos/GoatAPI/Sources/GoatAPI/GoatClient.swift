import Foundation
import HTTPTypes
import OpenAPIRuntime
import OpenAPIURLSession

extension Client {
  /// A client that sends the token returned by `token()` as a bearer token on every request.
  public static func goat(serverURL: URL, token: @escaping @Sendable () -> String?) -> Client {
    Client(
      serverURL: serverURL,
      configuration: Configuration(dateTranscoder: .iso8601WithFractionalSeconds),
      transport: URLSessionTransport(),
      middlewares: [BearerToken(token: token)]
    )
  }
}

private struct BearerToken: ClientMiddleware {
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

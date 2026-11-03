import Foundation
import GoatAPI
import UniformTypeIdentifiers
import WebKit

/// Serves `goat://app`, the origin the web app runs on inside Goat. Paths under `/server` go to
/// the goat server with this Mac's session token added; everything else is the web app itself.
/// With one origin for both, the page needs no cookies and the server no CORS, and the page never
/// holds the token.
nonisolated struct AppScheme: URLSchemeHandler {
  static let name = "goat"
  static let serverPrefix = "/server"

  enum Source: Sendable {
    /// The web app built into Goat's resources.
    case bundled(URL)
    /// The web app's dev server, which also rebuilds the page as its code changes.
    case devServer(URL)
  }

  typealias Output = AsyncThrowingStream<URLSchemeTaskResult, any Error>.Continuation

  /// Request headers that stay with the page: its origin and credentials, which the token
  /// replaces, and its cache's validators, since WebKit cannot take a 304 from a scheme handler.
  private static let pageHeaders: Set<String> = [
    "authorization", "content-length", "cookie", "host", "if-modified-since", "if-none-match",
    "origin", "referer",
  ]

  let session: Session
  let serverURL: URL
  let source: Source

  func reply(for request: URLRequest) -> AsyncThrowingStream<URLSchemeTaskResult, any Error> {
    AsyncThrowingStream { output in
      let work = Task {
        do {
          try await respond(to: request, with: output)
          output.finish()
        } catch {
          output.finish(throwing: error)
        }
      }
      output.onTermination = { _ in work.cancel() }
    }
  }

  private func respond(to request: URLRequest, with output: Output) async throws {
    guard let pageURL = request.url else { throw URLError(.badURL) }
    let path = pageURL.path(percentEncoded: true)
    let query = pageURL.query(percentEncoded: true)

    if path == Self.serverPrefix || path.hasPrefix(Self.serverPrefix + "/") {
      let serverPath = String(path.dropFirst(Self.serverPrefix.count))
      let forwarded = proxied(
        request, to: try address(serverURL, path: serverPath, query: query),
        token: await session.token)
      switch (request.httpMethod, serverPath) {
      case ("POST", "/auth/sign-in"): try await signIn(forwarded, pageURL: pageURL, output)
      case ("POST", "/auth/sign-out"): try await signOut(forwarded, pageURL: pageURL, output)
      default: try await relay(forwarded, pageURL: pageURL, output)
      }
      return
    }

    switch source {
    case .bundled(let root):
      try serveFile(Self.file(at: path, in: root), pageURL: pageURL, output)
    case .devServer(let devServerURL):
      let forwarded = proxied(
        request, to: try address(devServerURL, path: path, query: query), token: nil)
      try await relay(forwarded, pageURL: pageURL, output)
    }
  }

  private func address(_ base: URL, path: String, query: String?) throws -> URL {
    guard var components = URLComponents(url: base, resolvingAgainstBaseURL: false) else {
      throw URLError(.badURL)
    }
    let basePath = components.percentEncodedPath
    components.percentEncodedPath =
      (basePath.hasSuffix("/") ? String(basePath.dropLast()) : basePath) + path
    components.percentEncodedQuery = query
    guard let url = components.url else { throw URLError(.badURL) }
    return url
  }

  private func proxied(_ original: URLRequest, to url: URL, token: String?) -> URLRequest {
    var request = URLRequest(url: url)
    request.httpMethod = original.httpMethod
    request.httpBody = original.httpBody
    // The server's sign-in sets a cookie for browsers; this request carries the token instead.
    request.httpShouldHandleCookies = false
    for (name, value) in original.allHTTPHeaderFields ?? [:]
    where !Self.pageHeaders.contains(name.lowercased()) {
      request.setValue(value, forHTTPHeaderField: name)
    }
    if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
    return request
  }

  /// Keeps the token the server answers with, so the device link and later requests use it.
  private func signIn(_ request: URLRequest, pageURL: URL, _ output: Output) async throws {
    let (data, response) = try await URLSession.shared.data(for: request)
    if (response as? HTTPURLResponse)?.statusCode == 200,
      let signedIn = try? JSONDecoder().decode(Components.Schemas.Session.self, from: data)
    {
      await session.signIn(token: signedIn.token)
    }
    output.yield(.response(pageResponse(response, pageURL: pageURL)))
    output.yield(.data(data))
  }

  /// Forgets the token once the server has ended the session, or already had.
  private func signOut(_ request: URLRequest, pageURL: URL, _ output: Output) async throws {
    let (data, response) = try await URLSession.shared.data(for: request)
    if [204, 401].contains((response as? HTTPURLResponse)?.statusCode) {
      await session.signOut()
    }
    output.yield(.response(pageResponse(response, pageURL: pageURL)))
    output.yield(.data(data))
  }

  /// Hands the response to WebKit as it arrives, so event streams flow and large files start at
  /// once.
  private func relay(_ request: URLRequest, pageURL: URL, _ output: Output) async throws {
    let task = URLSession.shared.dataTask(with: request)
    try await withTaskCancellationHandler {
      let _: Void = try await withCheckedThrowingContinuation { finished in
        task.delegate = Relay(pageURL: pageURL, output: output, finished: finished)
        task.resume()
      }
    } onCancel: {
      task.cancel()
    }
  }

  private func serveFile(_ file: URL?, pageURL: URL, _ output: Output) throws {
    guard let file else {
      output.yield(.response(Self.response(pageURL, status: 404, type: "text/plain")))
      return
    }
    let type = UTType(filenameExtension: file.pathExtension)?.preferredMIMEType
    output.yield(
      .response(Self.response(pageURL, status: 200, type: type ?? "application/octet-stream")))
    output.yield(.data(try Data(contentsOf: file)))
  }

  /// The built file at `path`, or the app's page for the router's own addresses, which have no
  /// extension. Nil for a missing file, and never a file outside `root`.
  static func file(at path: String, in root: URL) -> URL? {
    let root = root.standardizedFileURL
    let page = root.appending(path: "index.html")
    let relative = path.removingPercentEncoding ?? path
    let file = root.appending(path: relative).standardizedFileURL
    guard file.path().hasPrefix(root.path()) else { return nil }
    var isDirectory: ObjCBool = false
    if FileManager.default.fileExists(atPath: file.path(), isDirectory: &isDirectory) {
      return isDirectory.boolValue ? page : file
    }
    return file.pathExtension.isEmpty ? page : nil
  }

  private static func response(_ url: URL, status: Int, type: String) -> URLResponse {
    HTTPURLResponse(
      url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": type])
      ?? URLResponse(url: url, mimeType: type, expectedContentLength: -1, textEncodingName: nil)
  }
}

/// Response headers that only make sense between a browser and the server: URLSession has already
/// decoded the body and kept the cookies away from the page.
nonisolated private let serverHeaders: Set<String> = [
  "access-control-allow-credentials", "access-control-allow-origin", "content-encoding",
  "content-length", "set-cookie",
]

/// The server's response as WebKit expects it: for the page's own address, without the headers
/// that were meant for a browser talking to the server directly.
nonisolated private func pageResponse(_ response: URLResponse, pageURL: URL) -> URLResponse {
  guard let http = response as? HTTPURLResponse else { return response }
  var headers: [String: String] = [:]
  for case (let name as String, let value as String) in http.allHeaderFields
  where !serverHeaders.contains(name.lowercased()) {
    headers[name] = value
  }
  return HTTPURLResponse(
    url: pageURL, statusCode: http.statusCode, httpVersion: "HTTP/1.1", headerFields: headers)
    ?? response
}

nonisolated private final class Relay: NSObject, URLSessionDataDelegate, Sendable {
  private let pageURL: URL
  private let output: AppScheme.Output
  private let finished: CheckedContinuation<Void, any Error>

  init(pageURL: URL, output: AppScheme.Output, finished: CheckedContinuation<Void, any Error>) {
    self.pageURL = pageURL
    self.output = output
    self.finished = finished
  }

  func urlSession(
    _ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse
  ) async -> URLSession.ResponseDisposition {
    output.yield(.response(pageResponse(response, pageURL: pageURL)))
    return .allow
  }

  func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
    output.yield(.data(data))
  }

  func urlSession(
    _ session: URLSession, task: URLSessionTask, didCompleteWithError error: (any Error)?
  ) {
    if let error {
      finished.resume(throwing: error)
    } else {
      finished.resume()
    }
  }
}

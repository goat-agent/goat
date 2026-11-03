import Foundation

/// A call from the page. WebKit hands over the message as Foundation objects, so the params stay
/// as JSON until the method says which type they are.
public struct Request: Sendable {
  public let method: Components.Schemas.RequestMethod
  private let params: Data

  /// Nil when the body is not a request the contract knows.
  public init?(body: Any) {
    guard let body = body as? [String: Any], let name = body["method"] as? String,
      let method = Components.Schemas.RequestMethod(rawValue: name),
      let params = try? JSONSerialization.data(withJSONObject: body["params"] ?? [:])
    else { return nil }
    self.method = method
    self.params = params
  }

  public func params<Params: Decodable>(_ type: Params.Type) throws -> Params {
    try JSONDecoder().decode(type, from: params)
  }
}

/// An answer in the form WebKit's reply handler passes to the page.
public func reply(_ result: some Encodable) throws -> Any {
  try JSONSerialization.jsonObject(with: JSONEncoder().encode(result))
}

/// The script that puts the host on the page before any of the page's own scripts run.
public func hostScript(_ host: Components.Schemas.Host) throws -> String {
  "window.goatHost = \(try json(host));"
}

/// The script that hands an event to the page's listeners, as a `goat:<event>` DOM event.
public func eventScript(_ event: Components.Schemas.EventName, _ payload: some Encodable) throws
  -> String
{
  "window.dispatchEvent(new CustomEvent(\"goat:\(event.rawValue)\", { detail: \(try json(payload)) }));"
}

private func json(_ value: some Encodable) throws -> String {
  String(decoding: try JSONEncoder().encode(value), as: UTF8.self)
}

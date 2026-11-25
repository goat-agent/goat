import json
import re
import sys

MODE = sys.argv[1] if len(sys.argv) > 1 else "push"
documents = {}


def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


def receive():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.partition(b":")
        if name.lower() == b"content-length":
            length = int(value)
    return json.loads(sys.stdin.buffer.read(length))


def diagnostics(uri):
    found = []
    for number, line in enumerate(documents[uri].splitlines()):
        if "ERROR" in line:
            severity, message = 1, "bad line " + line.strip()
        elif "WARN" in line:
            severity, message = 2, "odd line"
        else:
            continue
        found.append({
            "range": {"start": {"line": number, "character": 0}, "end": {"line": number, "character": len(line)}},
            "severity": severity,
            "message": message,
        })
    return found


def publish(uri, version):
    send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "check", "value": {"kind": "begin", "title": "check"}}})
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "version": version, "diagnostics": diagnostics(uri)}})
    send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "check", "value": {"kind": "end"}}})


def word_at(uri, position):
    line = documents[uri].splitlines()[position["line"]]
    match = re.compile(r"\w+").search(line, position["character"])
    return match.group(0)


def location(uri, number, line):
    return {"uri": uri, "range": {"start": {"line": number, "character": 0}, "end": {"line": number, "character": len(line)}}}


def answer(method, params):
    if method == "initialize":
        capabilities = {
            "textDocumentSync": 1,
            "definitionProvider": True,
            "referencesProvider": True,
            "hoverProvider": True,
            "documentSymbolProvider": True,
            "renameProvider": True,
            "workspaceSymbolProvider": True,
        }
        if MODE == "pull":
            capabilities["diagnosticProvider"] = {"interFileDependencies": False, "workspaceDiagnostics": False}
        return {"capabilities": capabilities}
    if method == "textDocument/diagnostic":
        return {"kind": "full", "items": diagnostics(params["textDocument"]["uri"])}
    uri = params.get("textDocument", {}).get("uri")
    if method == "textDocument/definition":
        name = word_at(uri, params["position"])
        for number, line in enumerate(documents[uri].splitlines()):
            if line.startswith("def " + name):
                return location(uri, number, line)
        return None
    if method == "textDocument/references":
        name = word_at(uri, params["position"])
        return [location(uri, number, line) for number, line in enumerate(documents[uri].splitlines()) if name in line]
    if method == "textDocument/hover":
        return {"contents": {"kind": "plaintext", "value": "hover of " + word_at(uri, params["position"])}}
    if method == "textDocument/documentSymbol":
        found = []
        for number, line in enumerate(documents[uri].splitlines()):
            if line.startswith("def "):
                place = location(uri, number, line)["range"]
                found.append({"name": line[4:].split("(")[0], "kind": 12, "range": place, "selectionRange": place})
        return found
    if method == "workspace/symbol":
        found = []
        for known, text in documents.items():
            for number, line in enumerate(text.splitlines()):
                if line.startswith("def ") and params["query"] in line:
                    found.append({"name": line[4:].split("(")[0], "kind": 12, "location": location(known, number, line)})
        return found
    if method == "textDocument/rename":
        name = word_at(uri, params["position"])
        edits = []
        for number, line in enumerate(documents[uri].splitlines()):
            for match in re.finditer(r"\b" + re.escape(name) + r"\b", line):
                edits.append({
                    "range": {"start": {"line": number, "character": match.start()}, "end": {"line": number, "character": match.end()}},
                    "newText": params["newName"],
                })
        return {"changes": {uri: edits}}
    if method == "shutdown":
        return None
    raise KeyError(method)


def main():
    while True:
        message = receive()
        if message is None:
            return
        method = message.get("method")
        params = message.get("params") or {}
        if "id" in message and method:
            try:
                send({"jsonrpc": "2.0", "id": message["id"], "result": answer(method, params)})
            except KeyError:
                send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "unknown " + method}})
            continue
        if method == "textDocument/didOpen":
            document = params["textDocument"]
            documents[document["uri"]] = document["text"]
            if MODE == "push":
                publish(document["uri"], document["version"])
        elif method == "textDocument/didChange":
            uri = params["textDocument"]["uri"]
            documents[uri] = params["contentChanges"][-1]["text"]
            if MODE == "push":
                publish(uri, params["textDocument"]["version"])


main()

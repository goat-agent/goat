import json
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "Echo the text back",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string", "description": "What to echo"}},
            "required": ["text"],
        },
    },
    {
        "name": "sum",
        "description": "Add numbers",
        "inputSchema": {
            "type": "object",
            "properties": {"numbers": {"type": "array", "items": {"type": "number"}}},
        },
    },
]


def reply(id, result):
    send({"jsonrpc": "2.0", "id": id, "result": result})


def reject(id):
    send({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}})


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if "id" not in message:
        continue
    if method == "initialize":
        reply(message["id"], {
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fixture", "version": "1"},
            "instructions": "Echo before you sum.",
        })
    elif method == "tools/list":
        reply(message["id"], {"tools": TOOLS})
    elif method == "tools/call":
        name = message["params"]["name"]
        args = message["params"].get("arguments") or {}
        if name == "echo":
            reply(message["id"], {"content": [{"type": "text", "text": args["text"]}]})
        elif name == "sum":
            total = sum(args.get("numbers", []))
            reply(message["id"], {
                "content": [{"type": "text", "text": json.dumps({"total": total})}],
                "structuredContent": {"total": total},
            })
        else:
            reply(message["id"], {"content": [{"type": "text", "text": "no tool " + name}], "isError": True})
    else:
        reject(message["id"])

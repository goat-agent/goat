use std::path::Path;

use goat_wire::{ClientFrame, ServerFrame};
use schemars::schema_for;

fn write_pretty(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    std::fs::write(path, text)
}

fn main() -> std::io::Result<()> {
    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "docs/protocol".to_owned());
    let out = Path::new(&out_dir);
    std::fs::create_dir_all(out)?;

    let client = serde_json::to_value(schema_for!(ClientFrame))?;
    let server = serde_json::to_value(schema_for!(ServerFrame))?;
    let op = serde_json::to_value(schema_for!(goat_protocol::Op))?;
    let event = serde_json::to_value(schema_for!(goat_protocol::Event))?;

    write_pretty(&out.join("client-frame.schema.json"), &client)?;
    write_pretty(&out.join("server-frame.schema.json"), &server)?;
    write_pretty(&out.join("op.schema.json"), &op)?;
    write_pretty(&out.join("event.schema.json"), &event)?;

    let asyncapi = build_asyncapi(&client, &server);
    write_pretty(&out.join("asyncapi.json"), &asyncapi)?;

    eprintln!("wrote protocol schemas to {}", out.display());
    Ok(())
}

fn build_asyncapi(client: &serde_json::Value, server: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "asyncapi": "3.0.0",
        "info": {
            "title": "goat-code daemon protocol",
            "version": goat_wire::PROTOCOL_VERSION.to_string(),
            "description": "Length-delimited JSON frames over the daemon unix socket (and remote mTLS WebSocket). A client sends ClientFrame, the daemon replies with ServerFrame."
        },
        "defaultContentType": "application/json",
        "channels": {
            "clientToDaemon": {
                "address": "/",
                "messages": { "clientFrame": { "$ref": "#/components/messages/ClientFrame" } }
            },
            "daemonToClient": {
                "address": "/",
                "messages": { "serverFrame": { "$ref": "#/components/messages/ServerFrame" } }
            }
        },
        "operations": {
            "sendClientFrame": {
                "action": "send",
                "channel": { "$ref": "#/channels/clientToDaemon" }
            },
            "receiveServerFrame": {
                "action": "receive",
                "channel": { "$ref": "#/channels/daemonToClient" }
            }
        },
        "components": {
            "messages": {
                "ClientFrame": {
                    "name": "ClientFrame",
                    "title": "Client frame",
                    "payload": client
                },
                "ServerFrame": {
                    "name": "ServerFrame",
                    "title": "Server frame",
                    "payload": server
                }
            }
        }
    })
}

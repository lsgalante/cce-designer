use std::net::TcpListener;
use std::io::{BufRead, BufReader, Read, Write};
use cce_ui::mcp::McpTool;
use serde_json::json;
use crate::{CustomEvent, HttpAction};

pub fn start_http_server(server_sender: calloop::channel::Sender<CustomEvent>) {
    std::thread::spawn(move || {
        // CCE_DESIGNER_HTTP_PORT overrides the default so a second instance
        // (tests, debugging) can run alongside one already holding 3000.
        let port: u16 = std::env::var("CCE_DESIGNER_HTTP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3000);
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Failed to bind HTTP server to port {port}: {:?}", e);
                return;
            }
        };
        println!("Embedded HTTP Server listening on http://127.0.0.1:{port}");

        for stream in listener.incoming() {
            let stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };

            let server_sender = server_sender.clone();
            std::thread::spawn(move || {
                let mut write_stream = match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    return;
                }

                if request_line.starts_with("GET /state") {
                    let (tx, rx) = std::sync::mpsc::channel();
                    if server_sender.send(CustomEvent::GetState(tx)).is_ok() {
                        let response_body = rx.recv().unwrap_or_else(|_| "null".to_string());
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            response_body.len(),
                            response_body
                        );
                        let _ = write_stream.write_all(response.as_bytes());
                    }
                } else if request_line.starts_with("POST /action") {
                    let mut content_length = 0;
                    loop {
                        let mut header_line = String::new();
                        if reader.read_line(&mut header_line).is_err() || header_line == "\r\n" || header_line == "\n" || header_line.is_empty() {
                            break;
                        }
                        let lower = header_line.to_lowercase();
                        if lower.starts_with("content-length:") {
                            if let Some(val) = lower.split(':').nth(1) {
                                if let Ok(len) = val.trim().parse::<usize>() {
                                    content_length = len;
                                }
                            }
                        }
                    }

                    let mut body = vec![0; content_length];
                    if reader.read_exact(&mut body).is_ok() {
                        let body_str = String::from_utf8_lossy(&body);
                        if let Ok(action) = serde_json::from_str::<HttpAction>(&body_str) {
                            let (tx, rx) = std::sync::mpsc::channel();
                            if server_sender.send(CustomEvent::PostAction(action, tx)).is_ok() {
                                let res = rx.recv().unwrap_or_else(|_| Err("internal error".to_string()));
                                let response = match res {
                                    Ok(msg) => {
                                        let body = format!("{{\"status\":\"success\",\"message\":\"{}\"}}", msg);
                                        format!(
                                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                            body.len(),
                                            body
                                        )
                                    }
                                    Err(err) => {
                                        let body = format!("{{\"status\":\"error\",\"error\":\"{}\"}}", err);
                                        format!(
                                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                            body.len(),
                                            body
                                        )
                                    }
                                };
                                let _ = write_stream.write_all(response.as_bytes());
                            } else {
                                let body = "{\"status\":\"error\",\"error\":\"failed to send action to event loop\"}";
                                let response = format!(
                                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                );
                                let _ = write_stream.write_all(response.as_bytes());
                            }
                        } else {
                            let body = "{\"status\":\"error\",\"error\":\"failed to parse action JSON\"}";
                            let response = format!(
                                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = write_stream.write_all(response.as_bytes());
                        }
                    } else {
                        let body = "{\"status\":\"error\",\"error\":\"failed to read complete body\"}";
                        let response = format!(
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = write_stream.write_all(response.as_bytes());
                    }
                } else {
                    let body = "{\"error\":\"not found\"}";
                    let response = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = write_stream.write_all(response.as_bytes());
                }
                let _ = write_stream.flush();
            });
        }
    });
}

/// Start the embedded MCP server (cce-ui's tools-only Streamable HTTP
/// implementation): the same automation surface as the HTTP API, spoken as
/// MCP tools so agents can attach with
/// `claude mcp add --transport http cce-designer http://127.0.0.1:3001/mcp`.
pub fn start_mcp_server(server_sender: calloop::channel::Sender<CustomEvent>) {
    // CCE_DESIGNER_MCP_PORT overrides the default, same as the HTTP port.
    let port: u16 = std::env::var("CCE_DESIGNER_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3001);
    cce_ui::mcp::start_mcp_server("cce-designer", port, mcp_tools(), server_sender, CustomEvent::McpCall);
}

/// The designer's MCP tools: `get_state` plus one tool per `HttpAction`
/// variant — the tool name is the variant's serde tag and the arguments are
/// its fields, so dispatch is deserialization (see `apply_mcp_call`).
pub(crate) fn mcp_tools() -> Vec<McpTool> {
    let tool = |name: &str, description: &str, schema: serde_json::Value| McpTool {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema,
    };
    let no_args = || json!({ "type": "object", "properties": {} });
    let slot = |desc: &str| json!({ "type": "integer", "description": desc });
    vec![
        tool(
            "get_state",
            "Get the current project state (node tree with params, cameras, pan, current path, selection) as JSON.",
            no_args(),
        ),
        tool(
            "up",
            "Navigate up one level in the node network (out of the current subnet).",
            no_args(),
        ),
        tool(
            "enter",
            "Enter the subnet/node at the given slot index in the current network level.",
            json!({
                "type": "object",
                "properties": { "slot": slot("Child index in the current network level") },
                "required": ["slot"],
            }),
        ),
        tool(
            "set_param",
            "Set a parameter on the node at the given slot. All values are strings (e.g. \"1.5\", \"0.2,0.4,1\").",
            json!({
                "type": "object",
                "properties": {
                    "slot": slot("Child index in the current network level"),
                    "name": { "type": "string", "description": "Parameter name" },
                    "value": { "type": "string", "description": "New value, as a string" },
                },
                "required": ["slot", "name", "value"],
            }),
        ),
        tool("reset_camera", "Reset the 3D viewport camera rotation and zoom.", no_args()),
        tool(
            "load",
            "Load a project (a project directory containing state.json, or a single state .json file).",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Filesystem path" } },
                "required": ["path"],
            }),
        ),
        tool(
            "save",
            "Save the current project to the given path.",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Filesystem path" } },
                "required": ["path"],
            }),
        ),
        tool(
            "toggle_geometry",
            "Toggle geometry visibility for the node at the given slot (not valid on utility nodes).",
            json!({
                "type": "object",
                "properties": { "slot": slot("Child index in the current network level") },
                "required": ["slot"],
            }),
        ),
        tool(
            "add_node",
            "Add a node from a template (e.g. \"Sphere\") at grid position (x, y) in the current network level.",
            json!({
                "type": "object",
                "properties": {
                    "template_name": { "type": "string", "description": "Template label or type, case-insensitive" },
                    "name": { "type": "string", "description": "Optional node name; auto-numbered when omitted" },
                    "x": { "type": "number", "description": "Grid column" },
                    "y": { "type": "number", "description": "Grid row" },
                },
                "required": ["template_name", "x", "y"],
            }),
        ),
        tool(
            "delete_node",
            "Delete the node at the given slot in the current network level.",
            json!({
                "type": "object",
                "properties": { "slot": slot("Child index in the current network level") },
                "required": ["slot"],
            }),
        ),
        tool(
            "rename_node",
            "Rename the node at the given slot.",
            json!({
                "type": "object",
                "properties": {
                    "slot": slot("Child index in the current network level"),
                    "new_name": { "type": "string" },
                },
                "required": ["slot", "new_name"],
            }),
        ),
        tool(
            "move_node",
            "Move the node at the given slot to grid position (x, y).",
            json!({
                "type": "object",
                "properties": {
                    "slot": slot("Child index in the current network level"),
                    "x": { "type": "number", "description": "Grid column" },
                    "y": { "type": "number", "description": "Grid row" },
                },
                "required": ["slot", "x", "y"],
            }),
        ),
        tool(
            "add_param",
            "Add a parameter to the node at the given slot.",
            json!({
                "type": "object",
                "properties": {
                    "slot": slot("Child index in the current network level"),
                    "name": { "type": "string" },
                    "param_type": { "type": "string", "description": "e.g. float, int, slider, float3, spinbox, choice" },
                    "default": { "type": "string", "description": "Default value, as a string" },
                },
                "required": ["slot", "name", "param_type", "default"],
            }),
        ),
        tool(
            "delete_param",
            "Delete a parameter from the node at the given slot.",
            json!({
                "type": "object",
                "properties": {
                    "slot": slot("Child index in the current network level"),
                    "name": { "type": "string", "description": "Parameter name" },
                },
                "required": ["slot", "name"],
            }),
        ),
        tool("toggle_circular_pane", "Toggle the circular network pane.", no_args()),
        tool(
            "menu_click",
            "Click a menubar item by indices (widget_idx must be a menubar widget slot).",
            json!({
                "type": "object",
                "properties": {
                    "widget_idx": { "type": "integer", "description": "Widget slot of the menubar" },
                    "menu_idx": { "type": "integer", "description": "Menu index within the menubar" },
                    "item_idx": { "type": "integer", "description": "Item index within the menu" },
                },
                "required": ["widget_idx", "menu_idx", "item_idx"],
            }),
        ),
        tool(
            "menu_closed",
            "Notify that a menu cloud was closed (clears the active menu-cloud state).",
            json!({
                "type": "object",
                "properties": {
                    "widget_idx": { "type": "integer" },
                    "menu_idx": { "type": "integer" },
                },
                "required": ["widget_idx", "menu_idx"],
            }),
        ),
        tool(
            "menu_action",
            "Execute a menu action by its label (e.g. \"Show Spreadsheet Pane\", \"Save\") — reaches label-matched menu-pane items that menu_click's index dispatch cannot.",
            json!({
                "type": "object",
                "properties": { "label": { "type": "string", "description": "Menu item label" } },
                "required": ["label"],
            }),
        ),
    ]
}

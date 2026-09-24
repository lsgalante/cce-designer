//! The embedded MCP automation server — the way to drive/inspect the
//! running app (the former bespoke HTTP API was retired in its favor).

use cce_ui::mcp::McpTool;
use serde_json::json;
use crate::CustomEvent;

/// Start the embedded MCP server (cce-ui's tools-only Streamable HTTP
/// implementation). Agents attach with
/// `claude mcp add --transport http cce-designer http://127.0.0.1:3001/mcp`.
pub fn start_mcp_server(server_sender: calloop::channel::Sender<CustomEvent>) {
    // CCE_DESIGNER_MCP_PORT overrides the default so a second instance
    // (tests, debugging) can run alongside one already holding 3001.
    let port: u16 = std::env::var("CCE_DESIGNER_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3001);
    cce_ui::mcp::start_mcp_server("cce-designer", port, mcp_tools(), server_sender, CustomEvent::McpCall);
}

/// The designer's MCP tools: `get_state` plus one tool per `McpAction`
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
            "select",
            "Select the node at the given slot in the current network level (like clicking it); its parameters populate the parameter pane.",
            json!({
                "type": "object",
                "properties": { "slot": slot("Child index in the current network level") },
                "required": ["slot"],
            }),
        ),
        tool(
            "set_param",
            "Set a parameter on the node at the given slot. All values are strings (e.g. \"1.5\", \"0.2,0.4,1\"). A value that reads as an expression — ch(\"../sphere1/Radius\") * 2, $F / 24 — becomes one (Houdini paths: relative to the node, .. its parent, / the root, a bare name its own parameter).",
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
            "Toggle geometry visibility for the node at the given slot.",
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
            "set_pane_collapsed",
            "Collapse a pane to its title stub, or expand it back.",
            json!({
                "type": "object",
                "properties": {
                    "pane": { "type": "string", "description": "network | parameters | spreadsheet | playbar" },
                    "collapsed": { "type": "boolean", "description": "true to collapse, false to expand" },
                },
                "required": ["pane", "collapsed"],
            }),
        ),
        tool(
            "set_pane_detached",
            "Move a pane into its own window, or take it back.",
            json!({
                "type": "object",
                "properties": {
                    "pane": { "type": "string", "description": "network | parameters | spreadsheet | playbar" },
                    "detached": { "type": "boolean", "description": "true to detach, false to reattach" },
                },
                "required": ["pane", "detached"],
            }),
        ),
        tool(
            "set_frame",
            "Move the playhead. Simnets solve up to this frame.",
            json!({
                "type": "object",
                "properties": {
                    "frame": { "type": "number", "description": "Timeline frame" },
                },
                "required": ["frame"],
            }),
        ),
        tool(
            "curve_set_points",
            "Replace a curve node's control points (world-space [x, y, z] triples). The Catmull-Rom strip re-evaluates immediately.",
            json!({
                "type": "object",
                "properties": {
                    "slot": { "type": "integer", "description": "Node index in the current directory; must be a curve node" },
                    "points": {
                        "type": "array",
                        "items": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 },
                        "description": "Control points as [x, y, z] triples; replaces the whole list",
                    },
                },
                "required": ["slot", "points"],
            }),
        ),
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
            "run_command",
            "Run a command by its registry id (e.g. \"toggle_dialog\", \"toggle_grid\", \"save_document\") — every command the dialog lists, including the ones no menu label reaches.",
            json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "Command id, snake_case, as input.kdl binds it" } },
                "required": ["id"],
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

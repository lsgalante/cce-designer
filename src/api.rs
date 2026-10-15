//! The embedded MCP automation server — the way to drive/inspect the
//! running app (the former bespoke HTTP API was retired in its favor).

use cce_ui::mcp::{McpServer, McpTool, ToolClass};
use serde_json::json;
use crate::CustomEvent;

/// What the model is told about the designer when a client connects (the
/// `initialize` reply's `instructions`).
const BRIEF: &str = "\
cce-designer is a node-based procedural design app, like Houdini: a project is a tree of \
networks, each a level of nodes wired together, evaluated into geometry shown in the viewport. \
Call `context` first: it names the open project, the level on screen, the selection and every \
node at that level with its slot. Nodes are addressed by `slot`, their index in the CURRENT \
level; `enter` and `up` change the level, and slots then mean that level's nodes. \
`get_state` returns the whole project with every parameter, which is large: use it to read \
parameter names and values before `set_param`. Edits apply to the open window at once and \
are undoable with `undo` (the same history as Ctrl+Z in the window). `save` writes the project \
file; never edit that file on disk while the app has it open.";

/// Start the embedded MCP server (cce-ui's tools-only Streamable HTTP
/// implementation), filed in the session's MCP registry. Agents configured
/// by hand attach with
/// `claude mcp add --transport http cce-designer http://127.0.0.1:3001/mcp`.
pub fn start_mcp_server(server_sender: calloop::channel::Sender<CustomEvent>) {
    // CCE_DESIGNER_MCP_PORT overrides the default so a second instance
    // (tests, debugging) can run alongside one already holding 3001.
    let port: u16 = std::env::var("CCE_DESIGNER_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3001);
    mcp_server().fixed_port(port).start(server_sender, CustomEvent::McpCall);
}

/// The server: the tools, what each does to the world, and the brief.
pub(crate) fn mcp_server() -> McpServer {
    McpServer::new("cce-designer", mcp_tools())
        .instructions(BRIEF)
        .classify(ToolClass::Read, &["get_state"])
        .classify(
            ToolClass::Edit,
            &[
                "up", "enter", "select", "set_param", "reset_camera", "toggle_bypass",
                "toggle_geometry", "add_node", "delete_node", "rename_node", "move_node",
                "add_param", "delete_param", "set_pane_collapsed", "set_pane_detached",
                "toggle_circular_pane", "set_frame", "curve_set_points", "undo", "redo",
            ],
        )
        // Saving replaces the file on disk and loading replaces the open
        // project, neither undoable here; a command or menu item can be anything.
        .classify(ToolClass::Outward, &["save", "load", "menu_click", "run_command", "menu_action"])
}

/// The designer's MCP tools: `context`, `get_state`, `undo` and `redo`, plus
/// one tool per `McpAction` variant — the tool name is the variant's serde
/// tag and the arguments are its fields, so dispatch is deserialization (see
/// `apply_mcp_call`).
pub(crate) fn mcp_tools() -> Vec<McpTool> {
    let tool = |name: &str, description: &str, schema: serde_json::Value| McpTool {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema,
    };
    let no_args = || json!({ "type": "object", "properties": {} });
    let slot = |desc: &str| json!({ "type": "integer", "description": desc });
    vec![
        cce_ui::mcp::context_tool(),
        tool(
            "undo",
            "Take back the last edit (the window's Ctrl+Z history). Says what was undone, or that there was nothing to undo.",
            no_args(),
        ),
        tool("redo", "Redo the last undone edit (Ctrl+Shift+Z).", no_args()),
        tool(
            "get_state",
            "Get the current project state (node tree with params, cameras, pan, current path, selection) as JSON, plus the playbar, the network grid and the status line's text (`status`).",
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
            "Set a parameter on the node at the given slot. All values are strings (e.g. \"1.5\", \"0.2,0.4,1\"). A value that reads as an expression — ch(\"../sphere1/radius\") * 2, $F / 24 — becomes one (Houdini paths: relative to the node, .. its parent, / the root, a bare name its own parameter). A row of a family that the node does not have yet — operand_3 on an Attribute node with one operand — is added, with any between.",
            json!({
                "type": "object",
                "properties": {
                    "slot": slot("Child index in the current network level"),
                    "name": { "type": "string", "description": "Parameter name (radius, base_resolution); its label (Base Resolution) is accepted too" },
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
            "toggle_bypass",
            "Bypass the node at the given slot, or stop bypassing it. A bypassed node stays in the graph and does nothing: what reads it gets the node's own Input.",
            json!({
                "type": "object",
                "properties": { "slot": slot("Child index in the current network level") },
                "required": ["slot"],
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
            "Add a node from a template (e.g. \"Sphere\") at grid position (x, y) in the current network level. The root holds Geometry nodes, cameras and pages; every operator goes inside a Geometry node, and a template that does not belong at the level is refused.",
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
                    "name": { "type": "string", "description": "The parameter's name, what a ch() path spells: lowercase letters, digits and underscores (base_resolution)" },
                    "label": { "type": "string", "description": "What the params pane shows for it (Base Resolution); the name when absent" },
                    "param_type": { "type": "string", "description": "One of text, float, slider, spinbox, sliderspinbox, float3, choice, toggle, button, code, node, attribute, group — optionally with detail after a colon (slider:-2:2, choice:A,B,C)" },
                    "default": { "type": "string", "description": "Default value, as a string" },
                },
                "required": ["slot", "name", "param_type", "default"],
            }),
        ),
        tool(
            "delete_param",
            "Delete a parameter from the node at the given slot. A member of a family of rows (operand_2) is taken away as its row menu takes it: the members after it move up a number, and the last is kept.",
            json!({
                "type": "object",
                "properties": {
                    "slot": slot("Child index in the current network level"),
                    "name": { "type": "string", "description": "Parameter name (radius, base_resolution); its label (Base Resolution) is accepted too" },
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
            "Click a menubar item by indices. Only the viewport menubar's Camera menu (menu 0) and the parameters menubar's Preset and Reset menus (0, 1) are dispatched this way; everything else is a command, see run_command.",
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

//! `State::sheet`: the spreadsheet's own state beside its widget — what
//! its rows were last read from (the key `sync_selection_readouts` skips a
//! refill on), where the rows' points stood, and the height its plate asks
//! for.

pub struct SheetPane {
    /// The node the rows were last read from — by ID, whatever the name
    /// says: names repeat across levels — and its parameters' texts then.
    /// None with nothing selected.
    pub last_spreadsheet_node_name: Option<String>,
    pub last_spreadsheet_node_params: Option<Vec<(String, String)>>,
    /// The frame and geometry version the spreadsheet's rows were read at.
    pub last_spreadsheet_read_at: (i32, u64),
    /// Where the rows' points were when the table was last filled — a row
    /// is a point, by index — kept so a selection of rows stages its markers
    /// in the scene without an evaluation (`rebuild_row_markers`, into
    /// `State::markers`' `row_markers`).
    pub spreadsheet_points: Vec<[f32; 3]>,
    /// The plate's height as asked for (`floating_spreadsheet_rect` fits
    /// it to the window); its top edge drags it.
    pub floating_spreadsheet_height: f32,
}

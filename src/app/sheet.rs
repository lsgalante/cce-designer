//! `State::sheet`: the spreadsheet's own state beside its widget — what
//! its rows were last read from (the key `sync_selection_readouts` skips a
//! refill on), where the rows' points stood, and the height its plate asks
//! for. The table's columns are built here too
//! (`State::geometry_to_spreadsheet_columns`).

use super::*;

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

impl State {
    /// The spreadsheet's table for `geom`, as headers and typed columns:
    /// a row per point — its number, its point groups (`g:<name>`, 1 or
    /// 0), its position and colour, each other point attribute a column per
    /// component, then the detail attributes (`d:`) repeated down theirs.
    pub(crate) fn geometry_to_spreadsheet_columns(geom: &Detail) -> (Vec<String>, Vec<cce_ui::widget::SheetColumn>) {
        // One row per POINT, not per triangle corner. The soup listed the same
        // place once for every face touching it — a sphere came to 2304 rows for
        // 362 places — and the row number meant nothing a user could point at.
        // It is now the point index, which is also what the Point Numbers overlay
        // draws.
        // The groups stand FIRST after the point's number, a column each,
        // `g:<name>`, 1 for a member and 0 for the rest. Until 2026-09-29
        // they were `g:` columns after every attribute — the thirteenth column
        // of a sphere's table, off the right of any pane — and blank for a
        // point not in the group, so a group of one point among five hundred
        // was a column that looked empty. Sorting the column, descending,
        // brings the members to the top. The header was `group:<name>` from
        // then until 2026-10-07, when the columns came to fit their content and
        // the header was the widest thing in a column of 0s and 1s; `g:` is the
        // detail columns' `d:` again.
        let groups = geom.points().group_names();
        let mut headers = vec!["Point".to_string()];
        headers.extend(groups.iter().map(|g| format!("g:{}", g)));
        headers.extend([
            "Pos.x".to_string(),
            "Pos.y".to_string(),
            "Pos.z".to_string(),
            "Col.r".to_string(),
            "Col.g".to_string(),
            "Col.b".to_string(),
        ]);

        // Columnar storage means the columns are known up front, from the store
        // rather than from a scan of every element's map. `names()` is sorted, so
        // they hold still between frames.
        let attribs: Vec<(String, crate::detail::AttribType)> = geom
            .points()
            .names()
            .into_iter()
            .filter(|n| *n != crate::detail::CD)
            .filter_map(|n| geom.points().get(n).map(|a| (n.to_string(), a.ty())))
            .collect();

        // A Derivative attribute wears a trailing `~`: it resets at every step
        // boundary, and telling that apart from a value that persists is the first
        // question you ask when a solve misbehaves.
        let mark = |geom: &Detail, class: crate::detail::Class, name: &str| -> String {
            match geom.store(class).kind(name) {
                crate::detail::AttribKind::Derivative => format!("{}~", name),
                crate::detail::AttribKind::Live => name.to_string(),
            }
        };

        for (name, ty) in &attribs {
            let label = mark(geom, crate::detail::Class::Point, name);
            match ty.components() {
                1 => headers.push(label),
                n => {
                    for c in ["x", "y", "z", "w"].iter().take(n) {
                        headers.push(format!("{}.{}", label, c));
                    }
                }
            }
        }

        // Detail attributes ride along as `d:` columns, constant down the table —
        // which is what a detail attribute IS. Without this the Analysis node
        // would write its answers somewhere nothing could show them.
        let detail: Vec<(String, crate::detail::AttribType)> = geom
            .detail()
            .names()
            .into_iter()
            .filter_map(|n| geom.detail().get(n).map(|a| (n.to_string(), a.ty())))
            .collect();
        for (name, ty) in &detail {
            let label = mark(geom, crate::detail::Class::Detail, name);
            match ty.components() {
                1 => headers.push(format!("d:{}", label)),
                n => {
                    for c in ["x", "y", "z", "w"].iter().take(n) {
                        headers.push(format!("d:{}.{}", label, c));
                    }
                }
            }
        }

        // The table as COLUMNS of values (cce-ui's `SheetColumn`): the widget
        // writes the cells it paints, so a refill is a copy of the values. A
        // playing simulation refills the table every frame, and until
        // 2026-10-07 every cell of every row was formatted into a String here
        // for a pane that shows thirty rows — at ten thousand points most of
        // what the table cost a frame.
        use cce_ui::widget::SheetColumn;
        let n = geom.num_points();
        let float = |values: Vec<f32>| SheetColumn::Float { values, decimals: 4 };
        // A value's components, a column each, `rows` long.
        let value_columns = |value: crate::detail::AttribValue, rows: usize| -> Vec<SheetColumn> {
            use crate::detail::AttribValue as V;
            match value {
                V::Float(f) => vec![float(vec![f; rows])],
                V::Int(i) => vec![SheetColumn::Int(vec![i as i64; rows])],
                V::Float2(a) => a.iter().map(|&v| float(vec![v; rows])).collect(),
                V::Float3(a) => a.iter().map(|&v| float(vec![v; rows])).collect(),
                V::Float4(a) => a.iter().map(|&v| float(vec![v; rows])).collect(),
            }
        };
        let mut columns = Vec::with_capacity(headers.len());
        columns.push(SheetColumn::Int((0..n as i64).collect()));
        for g in &groups {
            // The membership found once: `in_group` at every point looked the
            // group up by name each time, most of what the table cost a frame.
            let members = geom.points().group(g).unwrap_or(&[]);
            columns.push(SheetColumn::Int((0..n).map(|p| members.get(p).copied().unwrap_or(false) as i64).collect()));
        }
        for k in 0..3 {
            columns.push(float(geom.positions().iter().map(|p| p[k]).collect()));
        }
        let colors = geom.point_colors();
        for k in 0..3 {
            columns.push(float(colors.iter().map(|c| c[k]).collect()));
        }
        // A column covers its whole class, so there is no "this element does not
        // have it" case left to render as a dash.
        for (name, _) in &attribs {
            use crate::detail::AttribData as D;
            match geom.points().get(name) {
                Some(D::Float(v)) => columns.push(float(v.clone())),
                Some(D::Int(v)) => columns.push(SheetColumn::Int(v.iter().map(|&i| i as i64).collect())),
                Some(D::Float2(v)) => columns.extend((0..2).map(|k| float(v.iter().map(|c| c[k]).collect()))),
                Some(D::Float3(v)) => columns.extend((0..3).map(|k| float(v.iter().map(|c| c[k]).collect()))),
                Some(D::Float4(v)) => columns.extend((0..4).map(|k| float(v.iter().map(|c| c[k]).collect()))),
                None => {}
            }
        }
        for (name, ty) in &detail {
            match geom.detail().value(name, 0) {
                Some(value) => columns.extend(value_columns(value, n)),
                None => columns.extend((0..ty.components()).map(|_| SheetColumn::Text(vec!["-".to_string(); n]))),
            }
        }

        (headers, columns)
    }
}

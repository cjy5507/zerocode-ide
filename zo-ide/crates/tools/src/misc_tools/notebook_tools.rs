use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// --- Input/Output structs ---

#[derive(Debug, Deserialize)]
pub(crate) struct NotebookEditInput {
    pub notebook_path: String,
    pub cell_id: Option<String>,
    pub new_source: Option<String>,
    pub cell_type: Option<NotebookCellType>,
    pub edit_mode: Option<NotebookEditMode>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NotebookCellType {
    Code,
    Markdown,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NotebookEditMode {
    Replace,
    Insert,
    Delete,
}

#[derive(Debug, Serialize)]
pub(crate) struct NotebookEditOutput {
    pub(crate) new_source: String,
    pub(crate) cell_id: Option<String>,
    pub(crate) cell_type: Option<NotebookCellType>,
    pub(crate) language: String,
    pub(crate) edit_mode: String,
    pub(crate) error: Option<String>,
    pub(crate) notebook_path: String,
    pub(crate) original_file: String,
    pub(crate) updated_file: String,
}

// --- Execution ---

pub(crate) fn execute_notebook_edit(
    input: NotebookEditInput,
) -> Result<NotebookEditOutput, String> {
    let path = std::path::PathBuf::from(&input.notebook_path);
    if path.extension().and_then(|ext| ext.to_str()) != Some("ipynb") {
        return Err(String::from(
            "File must be a Jupyter notebook (.ipynb file).",
        ));
    }

    let original_file = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let mut notebook: serde_json::Value =
        serde_json::from_str(&original_file).map_err(|error| error.to_string())?;
    let language = notebook
        .get("metadata")
        .and_then(|metadata| metadata.get("kernelspec"))
        .and_then(|kernelspec| kernelspec.get("language"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("python")
        .to_string();
    let cells = notebook
        .get_mut("cells")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| String::from("Notebook cells array not found"))?;

    let edit_mode = input.edit_mode.unwrap_or(NotebookEditMode::Replace);
    let target_index = match input.cell_id.as_deref() {
        Some(cell_id) => Some(resolve_cell_index(cells, Some(cell_id), edit_mode)?),
        None if matches!(
            edit_mode,
            NotebookEditMode::Replace | NotebookEditMode::Delete
        ) =>
        {
            Some(resolve_cell_index(cells, None, edit_mode)?)
        }
        None => None,
    };
    let resolved_cell_type = match edit_mode {
        NotebookEditMode::Delete => None,
        NotebookEditMode::Insert => Some(input.cell_type.unwrap_or(NotebookCellType::Code)),
        NotebookEditMode::Replace => Some(input.cell_type.unwrap_or_else(|| {
            target_index
                .and_then(|index| cells.get(index))
                .and_then(cell_kind)
                .unwrap_or(NotebookCellType::Code)
        })),
    };
    let new_source = require_notebook_source(input.new_source, edit_mode)?;

    let cell_id = apply_notebook_edit(
        cells,
        edit_mode,
        target_index,
        resolved_cell_type,
        &new_source,
    )?;

    let updated_file =
        serde_json::to_string_pretty(&notebook).map_err(|error| error.to_string())?;
    std::fs::write(&path, &updated_file).map_err(|error| error.to_string())?;

    Ok(NotebookEditOutput {
        new_source,
        cell_id,
        cell_type: resolved_cell_type,
        language,
        edit_mode: format_notebook_edit_mode(edit_mode),
        error: None,
        notebook_path: path.display().to_string(),
        original_file,
        updated_file,
    })
}

/// Apply the requested edit to the notebook `cells`, returning the id of the
/// affected cell. Splitting the three-armed edit dispatch out of
/// [`execute_notebook_edit`] lets that function read as parse → resolve →
/// apply → persist.
fn apply_notebook_edit(
    cells: &mut Vec<serde_json::Value>,
    edit_mode: NotebookEditMode,
    target_index: Option<usize>,
    resolved_cell_type: Option<NotebookCellType>,
    new_source: &str,
) -> Result<Option<String>, String> {
    Ok(match edit_mode {
        NotebookEditMode::Insert => {
            let resolved_cell_type = resolved_cell_type
                .ok_or_else(|| String::from("insert mode requires a cell type"))?;
            let new_id = make_unique_cell_id(cells);
            let new_cell = build_notebook_cell(&new_id, resolved_cell_type, new_source);
            let insert_at = target_index.map_or(cells.len(), |index| index + 1);
            cells.insert(insert_at, new_cell);
            cells
                .get(insert_at)
                .and_then(|cell| cell.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        }
        NotebookEditMode::Delete => {
            let idx = target_index
                .ok_or_else(|| String::from("delete mode requires a target cell index"))?;
            let removed = cells.remove(idx);
            removed
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        }
        NotebookEditMode::Replace => {
            let resolved_cell_type = resolved_cell_type
                .ok_or_else(|| String::from("replace mode requires a cell type"))?;
            let idx = target_index
                .ok_or_else(|| String::from("replace mode requires a target cell index"))?;
            let cell = cells
                .get_mut(idx)
                .ok_or_else(|| String::from("Cell index out of range"))?;
            cell["source"] = serde_json::Value::Array(source_lines(new_source));
            cell["cell_type"] = serde_json::Value::String(match resolved_cell_type {
                NotebookCellType::Code => String::from("code"),
                NotebookCellType::Markdown => String::from("markdown"),
            });
            match resolved_cell_type {
                NotebookCellType::Code => {
                    if !cell.get("outputs").is_some_and(serde_json::Value::is_array) {
                        cell["outputs"] = json!([]);
                    }
                    if cell.get("execution_count").is_none() {
                        cell["execution_count"] = serde_json::Value::Null;
                    }
                }
                NotebookCellType::Markdown => {
                    if let Some(object) = cell.as_object_mut() {
                        object.remove("outputs");
                        object.remove("execution_count");
                    }
                }
            }
            cell.get("id")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        }
    })
}

fn require_notebook_source(
    source: Option<String>,
    edit_mode: NotebookEditMode,
) -> Result<String, String> {
    match edit_mode {
        NotebookEditMode::Delete => Ok(source.unwrap_or_default()),
        NotebookEditMode::Insert | NotebookEditMode::Replace => source
            .ok_or_else(|| String::from("new_source is required for insert and replace edits")),
    }
}

fn build_notebook_cell(cell_id: &str, cell_type: NotebookCellType, source: &str) -> Value {
    let mut cell = json!({
        "cell_type": match cell_type {
            NotebookCellType::Code => "code",
            NotebookCellType::Markdown => "markdown",
        },
        "id": cell_id,
        "metadata": {},
        "source": source_lines(source),
    });
    if let Some(object) = cell.as_object_mut() {
        match cell_type {
            NotebookCellType::Code => {
                object.insert(String::from("outputs"), json!([]));
                object.insert(String::from("execution_count"), Value::Null);
            }
            NotebookCellType::Markdown => {}
        }
    }
    cell
}

fn cell_kind(cell: &serde_json::Value) -> Option<NotebookCellType> {
    cell.get("cell_type")
        .and_then(serde_json::Value::as_str)
        .map(|kind| {
            if kind == "markdown" {
                NotebookCellType::Markdown
            } else {
                NotebookCellType::Code
            }
        })
}

fn resolve_cell_index(
    cells: &[serde_json::Value],
    cell_id: Option<&str>,
    edit_mode: NotebookEditMode,
) -> Result<usize, String> {
    if cells.is_empty()
        && matches!(
            edit_mode,
            NotebookEditMode::Replace | NotebookEditMode::Delete
        )
    {
        return Err(String::from("Notebook has no cells to edit"));
    }
    if let Some(cell_id) = cell_id {
        cells
            .iter()
            .position(|cell| cell.get("id").and_then(serde_json::Value::as_str) == Some(cell_id))
            .ok_or_else(|| format!("Cell id not found: {cell_id}"))
    } else {
        Ok(cells.len().saturating_sub(1))
    }
}

fn source_lines(source: &str) -> Vec<serde_json::Value> {
    if source.is_empty() {
        return vec![serde_json::Value::String(String::new())];
    }
    source
        .split_inclusive('\n')
        .map(|line| serde_json::Value::String(line.to_string()))
        .collect()
}

fn format_notebook_edit_mode(mode: NotebookEditMode) -> String {
    match mode {
        NotebookEditMode::Replace => String::from("replace"),
        NotebookEditMode::Insert => String::from("insert"),
        NotebookEditMode::Delete => String::from("delete"),
    }
}

fn make_cell_id(index: usize) -> String {
    format!("cell-{}", index + 1)
}

/// Generate a `cell-N` id that does not collide with any existing cell id.
///
/// Deriving the index from `cells.len()` is unsafe once a delete has shrunk the
/// array: `cell-{len+1}` can re-mint an id that an earlier cell already holds,
/// after which a later `cell_id` Replace/Delete resolves to the wrong cell.
/// Probe upward from the current length until we find an unused id.
fn make_unique_cell_id(cells: &[serde_json::Value]) -> String {
    let mut index = cells.len();
    loop {
        let candidate = make_cell_id(index);
        let collides = cells.iter().any(|cell| {
            cell.get("id").and_then(serde_json::Value::as_str) == Some(candidate.as_str())
        });
        if !collides {
            return candidate;
        }
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(id: &str) -> serde_json::Value {
        json!({
            "cell_type": "code",
            "id": id,
            "metadata": {},
            "source": [],
            "outputs": [],
            "execution_count": serde_json::Value::Null,
        })
    }

    fn cell_ids(cells: &[serde_json::Value]) -> Vec<String> {
        cells
            .iter()
            .filter_map(|cell| {
                cell.get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string)
            })
            .collect()
    }

    /// After a delete shrinks the array, an insert must not re-mint an id that a
    /// surviving cell already holds — otherwise a later `cell_id` edit hits the
    /// wrong cell.
    #[test]
    fn insert_after_delete_never_collides_with_existing_id() {
        // Notebook with sequential ids: cell-1, cell-2, cell-3.
        let mut cells = vec![cell("cell-1"), cell("cell-2"), cell("cell-3")];

        // Delete the *first* cell: cell-1 is removed, leaving cell-2, cell-3.
        let removed = apply_notebook_edit(
            &mut cells,
            NotebookEditMode::Delete,
            Some(0),
            None,
            "",
        )
        .expect("delete should succeed");
        assert_eq!(removed.as_deref(), Some("cell-1"));

        // Insert a new cell. With the buggy `make_cell_id(cells.len())` this
        // mints `cell-3` again (len == 2 -> cell-3), colliding with the
        // surviving `cell-3`; a later cell_id Replace/Delete would then resolve
        // to the wrong cell. The unique generator must avoid any existing id.
        let new_id = apply_notebook_edit(
            &mut cells,
            NotebookEditMode::Insert,
            Some(0),
            Some(NotebookCellType::Code),
            "x = 1\n",
        )
        .expect("insert should succeed")
        .expect("insert returns the new cell id");

        let ids = cell_ids(&cells);
        // The freshly minted id must be unique across the whole notebook.
        assert_eq!(
            ids.iter().filter(|id| **id == new_id).count(),
            1,
            "new cell id {new_id} collided with an existing id: {ids:?}",
        );
        // Every cell id in the notebook must remain distinct.
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "duplicate cell ids present: {ids:?}");
    }

    #[test]
    fn unique_id_skips_a_taken_sequential_slot() {
        // len == 2 -> first candidate is `cell-3`, but it is already taken,
        // so the generator must probe upward to `cell-4`.
        let cells = vec![cell("cell-1"), cell("cell-3")];
        assert_eq!(make_unique_cell_id(&cells), "cell-4");
    }
}

"""A report as a Markdown document.

    # Title

    ## Section name

    | sku  | qty | price |
    |:-----|----:|------:|
    | bolt |  12 |  0.35 |
    | nut  |     |  1.10 |
    | **total** | 12 | 1.45 |

Between blocks there is one blank line and the document ends with one newline.

  * Each column is as wide as its longest cell in that section, counting its name and its numeric
    total but not the `**total**` label, and at least 3. Text columns are padded on the right and numeric columns on the left. The
    divider row has the same width, with its colon on the side the column aligns to: `:---` for
    text and `---:` for numbers.
  * A pipe in a cell or a column name is written `\\|`, a newline is written `<br>`, and these
    count at their written width. A backslash stays as it is.
  * A cell that is not known is empty, and the width is padded as usual.
  * A decimal cell is written with exactly its column's places, rounded half up in its digits:
    with places=1, "1.25" is `1.3` and "-1.25" is `-1.3`. An int cell as it is.
  * The total row closes every section that has at least one row. The first column holds
    `**total**` when that column is text, and otherwise the row has its total in that column too
    and no label. Every numeric column holds its sum, of the cells that are known, with the same
    places rule, and any other column is empty. The total row is not padded: each cell is written
    between single spaces, as in the example.
  * A section with no rows is written as its heading and then the line `_No rows._`. A report
    with no sections is only its title.
"""


def export(report):
    """REPORT as Markdown text."""
    lines = [f"# {report.title}", ""]
    for section in report.sections:
        lines.append(f"## {section.name}")
        lines.append("")
        lines.append("| " + " | ".join(column.name for column in report.columns) + " |")
        for row in section.rows:
            lines.append("| " + " | ".join(str(cell) for cell in row.cells) + " |")
        lines.append("")
    return "\n".join(lines).rstrip("\n") + "\n"

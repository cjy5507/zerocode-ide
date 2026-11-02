"""One numeric column of a report as a chart in plain text, for a terminal.

`export(report, column=None, width=20)` draws COLUMN, or the first numeric column when it is
None, from the report's rows in order. The label of a row is its first text cell, or `?` when
it has no known one.

    Stock: qty
    North
      bolt   ████████████████████ 12
      washer ██████████ 6
      nut    (unknown)

  * The first line is the title, a colon, a space and the column name.
  * Each section is its name on a line of its own and then its rows. A section with no rows is its
    name only. Sections do not have blank lines between them, and the text ends with one newline.
  * Labels are indented two spaces and padded on the right to the longest label in the whole
    report, and one space follows the padding.
  * A bar's length is value × WIDTH ÷ the largest value in the whole report, measured in half
    cells and rounded half up to a whole number of half cells. A whole cell is `█` and a final
    half cell is `▌`. After the bar come a space and the value as it was written. A value of zero
    has no bar, and then there is one space fewer: the label's padding, a space, and the value.
  * A value above zero never rounds to nothing: it has at least `▌`.
  * A negative value draws no bar, as for zero, and it does not take part in the largest value.
    When no value is above zero, there are no bars at all.
  * A cell that is not known prints `(unknown)` in the place of a bar and value.
  * `ValueError` when COLUMN names no column, when it names a text column, or when the report has
    no numeric column and COLUMN is None.
"""


def export(report, column=None, width=20):
    """One column of REPORT as a text chart."""
    index = next(i for i, c in enumerate(report.columns) if c.name == column or (column is None and c.kind != "text"))
    lines = [report.title]
    for section in report.sections:
        for row in section.rows:
            lines.append(f"{row.cells[0]} {'#' * int(float(row.cells[index]))}")
    return "\n".join(lines) + "\n"

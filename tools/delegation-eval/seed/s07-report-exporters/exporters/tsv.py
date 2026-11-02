"""A report as tab-separated text for loading into another system.

    #title	Stock
    section	sku	qty	price
    North	bolt	12	0.35
    North	nut	\\N	1.10

  * The first line is `#title`, a tab and the title. The second names the columns, after a first
    column called `section`. Every other line is one row, in report order, carrying its section's
    name in front. The text ends with one newline, and a section with no rows adds no line.
  * In the title, section names, column names and cells, these are escaped: a backslash as `\\\\`,
    a tab as `\\t`, a newline as `\\n`, and a carriage return as `\\r`.
  * A cell that is not known is `\\N`, written as those two characters without further escaping.
    A text cell that really holds a backslash and an N is written `\\\\N`, so the two never meet.
  * An int cell is written as it is. A decimal cell is written with exactly its column's places,
    padded with zeros on the right when it has too few, and rounded half to even when it has too
    many: with places=2, "1.5" is `1.50`, "1.125" is `1.12`, "1.135" is `1.14`, and "-0.005" is
    `-0.00`, which keeps its sign because the text it came from had one.
  * `load(text)` reads this format back into (title, columns, rows): the column names without
    `section`, and each row as a list starting with the section name. Unknown cells come back as
    None, and every other cell as the unescaped text. `ValueError` for a text whose first line
    is not the title line, for a line with the wrong number of fields, or for a backslash
    followed by anything but the characters above.
"""


def export(report):
    """REPORT as tab-separated text."""
    lines = [f"#title\t{report.title}", "\t".join(["section"] + [column.name for column in report.columns])]
    for section in report.sections:
        for row in section.rows:
            cells = ["\\N" if cell is None else str(cell) for cell in row.cells]
            lines.append("\t".join([section.name] + cells))
    return "\n".join(lines) + "\n"


def load(text):
    """The (title, columns, rows) an exported TEXT holds."""
    lines = text.split("\n")
    title = lines[0].split("\t")[1]
    columns = lines[1].split("\t")[1:]
    return title, columns, [line.split("\t") for line in lines[2:] if line]

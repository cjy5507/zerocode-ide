"""The report every exporter writes out. Exporters read it and do not change it.

A report has a title, columns and rows, grouped into sections:

    Report(
        title="Stock",
        columns=[Column("sku", "text"), Column("qty", "int"), Column("price", "decimal", places=2)],
        sections=[Section("North", [Row(["bolt", 12, "0.35"]), Row(["nut", None, "1.10"])])],
    )

A cell is a str for a text column, an int for an int column, a decimal string such as "0.35"
for a decimal column, and None for a value that is not known, in any column. Rows hold one cell
per column, in the order of the columns. A section may hold no rows and a report may hold no
sections.
"""


class Column:
    def __init__(self, name, kind, places=0):
        self.name = name
        self.kind = kind
        self.places = places


class Row:
    def __init__(self, cells):
        self.cells = cells


class Section:
    def __init__(self, name, rows):
        self.name = name
        self.rows = rows


class Report:
    def __init__(self, title, columns, sections):
        self.title = title
        self.columns = columns
        self.sections = sections

"""A report as JSON Lines, one object per line, for a pipeline that reads a stream.

    {"kind":"report","columns":["sku","qty","price"],"sections":1,"title":"Stock"}
    {"kind":"row","price":"0.35","qty":12,"section":"North","sku":"bolt"}
    {"kind":"end","rows":1,"sum":{"price":"0.35","qty":12},"check":"476e7cdb"}

  * `kind` comes first in every object, and `check` comes last in the end object. Every other key
    is sorted. There are no spaces between tokens, and text that is not ASCII is written as it is,
    not as an escape. The text ends with one newline.
  * The first line is the report line: `columns` lists the names in report order, and `sections`
    counts the sections, the empty ones too.
  * Each row is one line, in report order, holding `section` and each column by name. A row whose
    column is itself called `section` or `kind` holds that value under `column.section` or
    `column.kind`, and those keys sort like any other.
  * An int cell is a JSON number and a decimal cell is a JSON string holding exactly its column's
    places, padded with zeros or rounded half up in its digits: `"0.35"`. A text cell is a string
    and a cell that is not known is null.
  * The last line is the end line. `rows` counts the row lines. `sum` holds each numeric column by
    name, as the sum of its known cells written as its own kind would be; a numeric column with no
    known cell sums to null. `check` is the CRC-32, from zlib, of every line before it including
    their newlines, encoded as UTF-8, written as eight lowercase hexadecimal digits.
"""

import json


def export(report):
    """REPORT as JSON Lines text."""
    names = [column.name for column in report.columns]
    lines = [json.dumps({"kind": "report", "title": report.title, "columns": names})]
    for section in report.sections:
        for row in section.rows:
            lines.append(json.dumps({"kind": "row", "section": section.name, **dict(zip(names, row.cells))}))
    return "\n".join(lines) + "\n"

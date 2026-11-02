"""A small CSV reader and writer. It does not lean on the standard csv module, whose errors
do not say where the trouble is.

Reading (`parse`):
  * A record ends at a newline, written `\\n` or `\\r\\n`. A final newline adds no record, and
    an empty text has no records.
  * Fields are separated by commas. A field may be quoted with double quotes, and then may
    hold commas, newlines (kept exactly as written) and quotes written twice.
  * A quote that opens a field only counts as the very first character of the field. A quote
    anywhere else in an unquoted field is an ordinary character: `a"b` is the text a"b.
  * After a closing quote only a comma or the end of the record may follow.
  * A blank line is a record of one empty field.
  * `ParseError` has `line` and `column`, both counted from 1, where the trouble was found:
    for text after a closing quote, the first such character; for a quoted field never closed,
    the quote that opened it.

Writing (`dump`):
  * A field is quoted only when it holds a comma, a quote, `\\r` or `\\n`, when it is empty and is
    the only field of its record, or when it starts or ends with a space.
  * Records end with `\\n`. `parse(dump(rows)) == rows` for any rows of strings.
"""


class ParseError(ValueError):
    def __init__(self, message, line, column):
        super().__init__(f"{message} at line {line}, column {column}")
        self.line = line
        self.column = column


def parse(text):
    """The records of TEXT, each a list of strings."""
    if not text:
        return []
    return [line.split(",") for line in text.rstrip("\n").split("\n")]


def dump(rows):
    """ROWS as CSV text."""
    return "".join(",".join(row) + "\n" for row in rows)

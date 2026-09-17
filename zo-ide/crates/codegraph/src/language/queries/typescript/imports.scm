(import_statement source: (string) @import.path)

(call_expression
  function: (identifier) @_require
  arguments: (arguments (string) @import.path)
  (#eq? @_require "require"))

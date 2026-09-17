(class_definition name: (identifier) @definition.class)
(function_definition name: (identifier) @definition.fn)

(class_definition
  body: (block
    (function_definition name: (identifier) @definition.method)))

(module
  (expression_statement
    (assignment left: (identifier) @definition.const)))

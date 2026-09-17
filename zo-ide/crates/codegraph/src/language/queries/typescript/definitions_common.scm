(class_declaration name: (_) @definition.class)
(function_declaration name: (identifier) @definition.fn)
(generator_function_declaration name: (identifier) @definition.fn)
(method_definition name: (property_identifier) @definition.method)

(lexical_declaration
  "const"
  (variable_declarator name: (identifier) @definition.const))

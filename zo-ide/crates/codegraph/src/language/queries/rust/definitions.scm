(function_item name: (identifier) @definition.fn)
(struct_item name: (type_identifier) @definition.struct)
(trait_item name: (type_identifier) @definition.trait)
(enum_item name: (type_identifier) @definition.enum)
(union_item name: (type_identifier) @definition.union)
(type_item name: (type_identifier) @definition.type)
(mod_item name: (identifier) @definition.mod)
(const_item name: (identifier) @definition.const)
(static_item name: (identifier) @definition.static)
(macro_definition name: (identifier) @definition.macro)

(impl_item
  body: (declaration_list
    (function_item name: (identifier) @definition.method)))

(trait_item
  body: (declaration_list
    (function_signature_item name: (identifier) @definition.method)))

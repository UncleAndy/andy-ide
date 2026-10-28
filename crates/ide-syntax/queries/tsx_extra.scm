; JSX nodes, appended to typescript.scm for the TSX grammar only:
; these node types do not exist in the plain TypeScript grammar.

(jsx_opening_element name: (identifier) @tag)
(jsx_closing_element name: (identifier) @tag)
(jsx_self_closing_element name: (identifier) @tag)
(jsx_attribute (property_identifier) @attribute)
(jsx_expression) @string.special

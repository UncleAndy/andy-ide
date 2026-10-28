; TypeScript highlighting.
;
; The query shipped inside tree-sitter-typescript covers only the TypeScript
; specific syntax and is meant to be used after the JavaScript query ("inherits:
; javascript" in the nvim-treesitter layout). The JavaScript query does not
; compile against the TypeScript grammar, because it mentions JSX nodes. This
; file therefore covers the shared syntax here, and tsx_extra.scm adds the JSX
; nodes on top for the TSX grammar.

(comment) @comment

(string) @string
(template_string) @string
(regex) @string

(number) @number

[ "const"
  "let"
  "var"
  "function"
  "class"
  "extends"
  "implements"
  "if"
  "else"
  "for"
  "of"
  "in"
  "while"
  "do"
  "return"
  "break"
  "continue"
  "switch"
  "case"
  "default"
  "try"
  "catch"
  "finally"
  "throw"
  "new"
  "delete"
  "typeof"
  "instanceof"
  "yield"
  "await"
  "async"
  "import"
  "export"
  "from"
  "as"
  "static"
  "abstract"
  "declare"
  "enum"
  "interface"
  "namespace"
  "public"
  "private"
  "protected"
  "readonly"
  "override"
  "satisfies"
  "keyof"
] @keyword

; `null`, `true`, `false` and `undefined` are named nodes in this grammar,
; not anonymous tokens, so they are matched by node name.
[
  (null)
  (true)
  (false)
  (undefined)
] @constant.builtin

(this) @variable.builtin
(super) @variable.builtin

(type_identifier) @type
(predefined_type) @type.builtin

; General property names come first: tree-sitter lets a later pattern
; override an earlier one, so the specific rules below win.
(property_identifier) @property

(function_declaration name: (identifier) @function)
(generator_function_declaration name: (identifier) @function)
(method_definition name: (property_identifier) @function.method)
(method_signature name: (property_identifier) @function.method)
(call_expression function: (identifier) @function.call)
(call_expression function: (member_expression property: (property_identifier) @function.method.call))
(new_expression constructor: (identifier) @type)

(variable_declarator name: (identifier) @variable)
(required_parameter pattern: (identifier) @variable.parameter)
(optional_parameter pattern: (identifier) @variable.parameter)
(shorthand_property_identifier_pattern) @variable

(decorator) @attribute

[ "="
  "=="
  "==="
  "!="
  "!=="
  "+"
  "-"
  "*"
  "/"
  "%"
  "&&"
  "||"
  "!"
  "=>"
  "<="
  ">="
  "?"
  "??"
] @operator

[ "("
  ")"
  "{"
  "}"
  "["
  "]"
  ","
  ";"
  ":"
  "."
] @punctuation

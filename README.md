# pbkit

`pbkit` is a Rust CLI for day-to-day protobuf inspection work:

- sort `.proto` text by `message`, `enum`, and field order
- decode unknown protobuf wire data from stdin or a file
- query protobuf binaries with a small JSONPath-like selector
- use either raw unknown wire fields, a descriptor set, or `.proto` source files

## Install

```sh
cargo install --path .
```

```sh
cargo install pbkit
```

## Usage

Sort a proto file in place:

```sh
pbkit sort api.proto --write
pbkit sort api.proto --fields name
```

Format and lint proto files:

```sh
pbkit fmt api.proto
pbkit fmt --write api.proto
pbkit fmt --write protos/
pbkit fmt --recursive --write protos/
pbkit fmt --check api.proto
pbkit fmt --check --diff protos/
pbkit fmt --config pbkit.toml --write protos/
pbkit fmt --without-sort api.proto

pbkit lint api.proto
pbkit lint protos/
pbkit lint --recursive protos/
pbkit lint --config pbkit.toml protos/
pbkit lint --without-sort api.proto
```

`fmt` validates protobuf syntax before it rewrites anything and renders from the
tree-sitter protobuf syntax tree. `lint` uses the same syntax tree and runs
default checks for file headers, naming, proto3 `required`, and whether
`pbkit fmt` would change the file. `lint --without-sort` keeps those default
rules and layout checks, but compares against layout-only formatting so import,
declaration, field, and enum value order is ignored.

Default lint checks:

- parse errors from the protobuf grammar
- missing `syntax` or `edition`
- missing `package`
- `message`, `enum`, `service`, and `rpc` names should be PascalCase
- field names should be lower_snake_case
- enum values should be UPPER_SNAKE_CASE
- `required` fields are rejected under proto3
- canonical `pbkit fmt` layout/order

`pbkit fmt` v1 uses a canonical style:

- two-space indentation, with tabs removed from indentation
- one trailing newline
- one blank line between top-level layout groups
- trailing comments move with the declaration or field they belong to and align to a stable column
- detached comments stay detached when declarations are sorted
- multiline option literals and rpc bodies are expanded consistently
- multiline field options are expanded consistently
- `reserved` and `extensions` ranges are spaced consistently
- imports sorted by default
- fields and enum values sorted by number by default
- declarations sorted by default
- `--without-sort` keeps declaration, import, and field order while still normalizing layout

`fmt` and `lint` accept either files or directories. Directory inputs are searched
recursively and only `.proto` files are processed. `--recursive` is accepted for
explicit CI scripts; recursion is already the default for directory inputs.
Use `pbkit fmt --check --diff` in CI to fail when formatting would change and
print a unified diff for the affected files.

`fmt` and `lint` read `pbkit.toml` from the current directory or an ancestor by
default. Use `--config path/to/pbkit.toml` to choose a specific file:

```toml
[fmt]
sort = true
field_sort = "number"      # "number" or "name"
declaration_sort = true
import_sort = true

[lint]
sort = true
```

CLI flags override config values. `--without-sort` disables import,
declaration, field, and enum value sorting for that run. Per-rule lint disable
lists are intentionally not implemented yet; lint currently uses the default
rules.

Decode unknown protobuf wire data:

```sh
cat pb.bin | pbkit decode
```

Query unknown wire data by field number. Length-delimited fields expose `bytes_base64`,
`utf8`, and a best-effort nested `message` view:

```sh
cat pb.bin | pbkit query '$.2[0].message.1[0]'
cat pb.bin | pbkit query '$.2[*].message.1[*]'
cat pb.bin | pbkit query '$.2[0]' --format raw > extracted.bin
```

Query with a descriptor set:

```sh
protoc -I . --include_imports --descriptor_set_out schema.pb schema.proto
cat pb.bin | pbkit query '$.user.name' --descriptor-set schema.pb --message my.pkg.Envelope
```

Query directly with `.proto` files. `pbkit` compiles them at runtime with `protox`, so
there is no `protoc` dependency for this path:

```sh
cat pb.bin | pbkit query '$.user.id' --proto schema.proto -I . --message my.pkg.Envelope
cat pb.bin | pbkit query '$.name' --proto single-message.proto -I .
cat pb.bin | pbkit query '$.payload' --proto schema.proto -I . --format raw > payload.bin
```

When a descriptor set or proto source resolves to exactly one non-map message,
`pbkit query` and `pbkit complete fields/query-path` infer `--message`
automatically.
For descriptor-based bytes fields, `--format raw`, `--format hex`, and
`--format base64` decode protobuf JSON base64 strings back to bytes.

Generate shell completions:

```sh
mkdir -p ~/.zfunc
pbkit completions zsh > _pbkit
pbkit completions bash > pbkit.bash
pbkit completions fish > pbkit.fish
```

Install bash completion for the current shell:

```sh
pbkit completions bash > pbkit.bash
source pbkit.bash
```

For zsh, install the generated completion once:

```sh
mkdir -p ~/.zfunc
pbkit completions zsh > ~/.zfunc/_pbkit
```

Then ensure `~/.zshrc` contains:

```sh
fpath=(~/.zfunc $fpath)
autoload -Uz compinit
compinit
```

For fish:

```sh
mkdir -p ~/.config/fish/completions
pbkit completions fish > ~/.config/fish/completions/pbkit.fish
```

The bash, zsh, and fish completions call `pbkit complete ...` on Tab, so they can
suggest proto files from `-I`, message names, field names, and query paths.
Query path completion understands repeated fields and maps, so it can suggest
forms such as `$.items[*]` and `$.labels["<key>"]`. `pbkit` also exposes those
proto-aware candidates directly for editor integrations:

Proto file completion searches `-I` roots and the current directory recursively.

```sh
pbkit complete proto-files -I protos --prefix user
pbkit complete messages --proto schema.proto -I . --prefix my.pkg.
pbkit complete fields --proto schema.proto -I . --message my.pkg.Envelope --prefix user
pbkit complete query-path --proto schema.proto -I . --message my.pkg.Envelope --prefix '$.user.n'
pbkit complete query-path --proto single-message.proto -I . --prefix '$.items['
```

## Path Syntax

The selector intentionally starts small:

- `$` is the root and can be omitted
- `.field` selects an object key
- `['field']` selects an object key
- `[0]` selects an array item
- `[*]` selects every array item
- `.*` selects every object value

When a key is applied to an array, `pbkit` applies it to every array item, so
`$.items.id` is equivalent to `$.items[*].id`. A query with multiple matches is
returned as a JSON array.

Unknown wire decoding uses numeric field names and arrays for repeated occurrences.
Descriptor-based decoding uses protobuf JSON field names.

## License

MIT OR Apache-2.0

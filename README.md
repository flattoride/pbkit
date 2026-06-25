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

After the crate is published:

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
pbkit fmt --check api.proto
pbkit fmt --without-sort api.proto
pbkit fmt --sort-declarations api.proto

pbkit lint api.proto
pbkit lint --without-sort api.proto
```

`fmt` validates protobuf syntax before it rewrites anything and renders from the
tree-sitter protobuf syntax tree. `lint` defaults to checking whether `pbkit fmt`
would change the file; `--without-sort` skips the sort/order check and only
reports syntax-level failures.

`pbkit fmt` v1 uses a canonical style:

- two-space indentation, with tabs removed from indentation
- one trailing newline
- one blank line between top-level layout groups
- imports sorted by default
- fields and enum values sorted by number by default
- declarations keep source order by default; use `--sort-declarations` to sort them
- `--without-sort` keeps declaration, import, and field order while still normalizing layout

Decode unknown protobuf wire data:

```sh
cat pb.bin | pbkit decode
```

Query unknown wire data by field number. Length-delimited fields expose `bytes_base64`,
`utf8`, and a best-effort nested `message` view:

```sh
cat pb.bin | pbkit query '$.2[0].message.1[0]'
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
```

Generate shell completions:

```sh
mkdir -p ~/.zfunc
pbkit completions zsh > _pbkit
pbkit completions bash > pbkit.bash
pbkit completions fish > pbkit.fish
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

The zsh completion calls `pbkit complete ...` on Tab, so it can suggest proto
files from `-I`, message names, field names, and query paths. `pbkit` also
exposes those proto-aware candidates directly for editor integrations:

```sh
pbkit complete proto-files -I protos --prefix user
pbkit complete messages --proto schema.proto -I . --prefix my.pkg.
pbkit complete fields --proto schema.proto -I . --message my.pkg.Envelope --prefix user
pbkit complete query-path --proto schema.proto -I . --message my.pkg.Envelope --prefix '$.user.n'
```

## Path Syntax

The selector intentionally starts small:

- `$` is the root and can be omitted
- `.field` selects an object key
- `['field']` selects an object key
- `[0]` selects an array item

Unknown wire decoding uses numeric field names and arrays for repeated occurrences.
Descriptor-based decoding uses protobuf JSON field names.

## License

MIT OR Apache-2.0

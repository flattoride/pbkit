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

## Usage

Sort a proto file in place:

```sh
pbkit sort api.proto --write
pbkit sort api.proto --fields name
```

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

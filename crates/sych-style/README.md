# sych-style

A style check that rustfmt and clippy cannot do. It reads the syntax tree (`syn`), so it does not
confuse an `if` expression with an `if` statement.

## The `blank-line-after-if` rule

An `if` statement (together with its `else` chain) that is followed by another statement in the same block is separated from
it by a blank line:

```rust
if missing {
    return Err(err);
}

let value = read()?;
```

It is not required before a closing `}`, inside an `else` chain, or for an `if` expression (`let x = if … else …`).

## Running

```sh
just lint          # part of lint
just style-fix     # insert blank lines and format
cargo run -p sych-style -- crates [--fix]
```

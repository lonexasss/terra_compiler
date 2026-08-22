# terra

A tiny transpiled scripting language. One-line programs compile to native Rust
binaries: terra reads a `.tr` script, emits equivalent Rust, and hands it to
`cargo`.

No runtime, no VM — every script becomes real compiled code.

## the language

Every line is `<verb>.<operand>`. Comments start with `#`.
Variables are `i64`, dynamically typed-free by design.

| line | meaning |
|---|---|
| `x.10` | assign an integer (declares on first use) |
| `x.y` | copy another variable |
| `x.+5` `x.-5` `x.*5` `x./5` | modify in place |
| `log."text"` | print text |
| `log.x` | print value |
| `log."text".x` | print text followed by value |
| `in.x` | read an integer from stdin |
| `w.500` | sleep 500 ms |
| `q.` | exit |

Reserved words: `log`, `in`, `w`, `q`. Negative literals: assign `0`, then subtract.

## example

```text
# demo.tr
log."enter a number:"
in.x
log."you typed ".x
x.+15
y.x
y.*2
log."doubled = ".y
```

```console
$ terra_compiler demo.tr --emit-rust
fn main() {
    println!("enter a number:");
    let mut x: i64 = { /* stdin read + parse */ };
    println!("you typed {}", x);
    x = x + 15;
    let mut y: i64 = x;
    y = y * 2;
    println!("doubled = {}", y);
}

$ terra_compiler demo.tr
enter a number:
7
you typed 7
doubled = 44
```

Errors are caught before anything is built:

```console
$ terra_compiler bad.tr
terra: error: line 1: missing operand for 'z'
terra: error: line 2: unknown variable 'zz'
terra: 2 error(s), nothing built
```

## usage

```console
$ cargo run -- script.tr              # compile & run
$ cargo run -- script.tr --emit-rust  # show generated rust only
```

## how it works

1. each line is validated against the grammar; undeclared variables are
   rejected at this stage with line numbers
2. accepted lines are translated to rust statements (`let mut` on first
   assignment, plain assignment afterwards)
3. generated code is written to `terra_build/` and executed via `cargo run`

## tests

```console
$ cargo test
```

21 unit tests cover translation of every command, escaping, mutability,
and diagnostics.

<div align="center">

<img src="https://capsule-render.vercel.app/api?type=waving&height=170&color=0:2b1810,50:8a4b2f,100:e07a5f&text=TERRA&fontSize=72&fontColor=f5ede6&fontAlignY=42&desc=.tr%20%E2%86%92%20rust%20%E2%86%92%20native%20binary&descAlignY=64&descSize=15&descColor=e0c4ae&animation=twinkling" width="100%" />

<img src="https://readme-typing-svg.demolab.com?font=Noto+Sans+Mono&weight=600&size=18&duration=2800&pause=900&color=E07A5F&center=true&vCenter=true&width=480&lines=write+.tr;get+rust;run+native" alt="typing" />

<p>
<img src="https://img.shields.io/badge/rust-stable-dea584?style=flat-square" alt="rust" />
<img src="https://github.com/lonexasss/terra_compiler/actions/workflows/ci.yml/badge.svg?style=flat-square" alt="ci status" />
<img src="https://img.shields.io/badge/license-MIT-e07a5f?style=flat-square" alt="mit" />
</p>

<img src="https://images.unsplash.com/photo-1515879218367-8466d910aaa4?q=80&w=900&auto=format&fit=crop&sat=-100" width="620" alt="" />

</div>

A tiny transpiled scripting language. One-line programs compile to native Rust
binaries: terra reads a `.tr` script, emits equivalent Rust, and hands it to
`cargo`. No VM, no interpreter — the output is plain rust.

## the grammar

Every line is `<verb>.<operand>`. Comments start with `#`.
Values are integers (i64).

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

## what you write / what you get

| demo.tr | the rust cargo runs |
|---|---|
| ```text```<br>```log."enter a number:"```<br>```in.x```<br>```log."you typed ".x```<br>```x.+15```<br>```y.x```<br>```y.*2```<br>```log."doubled = ".y``` | ```rust```<br>```fn main() {```<br>&nbsp;&nbsp;&nbsp;&nbsp;```println!("enter a number:");```<br>&nbsp;&nbsp;&nbsp;&nbsp;```let mut x: i64 = /* stdin */ ;```<br>&nbsp;&nbsp;&nbsp;&nbsp;```println!("you typed {}", x);```<br>&nbsp;&nbsp;&nbsp;&nbsp;```x += 15;```<br>&nbsp;&nbsp;&nbsp;&nbsp;```let mut y: i64 = x;```<br>&nbsp;&nbsp;&nbsp;&nbsp;```y *= 2;```<br>&nbsp;&nbsp;&nbsp;&nbsp;```println!("doubled = {}", y);```<br>```}``` |

```console
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

<div align="center">

<img src="https://capsule-render.vercel.app/api?type=waving&height=80&color=100:2b1810,50:8a4b2f,0:e07a5f&section=footer&animation=twinkling" width="100%" />

</div>

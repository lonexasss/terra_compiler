use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::process::{Command, exit};

const GENERATED_CARGO_TOML: &str =
    "[package]\nname = \"terra_output\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";

const VERSION: &str = env!("CARGO_PKG_VERSION");

const RESERVED: [&str; 26] = [
    "log", "in", "w", "q", "j", "jeq", "jne", "jlt", "jgt", "jle", "jge", "ask", "rnd", "cls",
    "at", "key", "len", "up", "low", "abs", "min", "max", "now", "bell", "chr", "ord",
];

const COND_OPS: [(&str, &str); 6] = [
    ("jeq", "=="),
    ("jne", "!="),
    ("jlt", "<"),
    ("jgt", ">"),
    ("jle", "<="),
    ("jge", ">="),
];

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_uint(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// integer literal (incl. negative) usable as a hoisted initializer
fn const_init(rhs: &str) -> Option<String> {
    let digits = rhs.strip_prefix('-').unwrap_or(rhs);
    if is_uint(digits) {
        Some(rhs.to_string())
    } else {
        None
    }
}

/// escape braces/quotes so println! doesn't eat them
fn escape_text(text: &str) -> String {
    // \" means a real quote character; protect it from the generic pass
    const ESC_Q: char = '\u{1}';
    text.replace("\\\"", &ESC_Q.to_string())
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(ESC_Q, "\\\"")
        .replace('{', "{{")
        .replace('}', "}}")
}

/// translation state: which variables exist (and of which type), whether
/// we generate the flat body (linear mode) or hoist for the state machine
#[derive(Clone)]
struct Ctx {
    ints: HashSet<String>,
    strs: HashSet<String>,
    machine: bool,
    hoisted: Vec<String>,
}

impl Ctx {
    fn new(machine: bool) -> Self {
        Ctx {
            ints: HashSet::new(),
            strs: HashSet::new(),
            machine,
            hoisted: Vec::new(),
        }
    }

    /// first mention declares (hoisted in machine mode), rest just assign
    fn bind(&mut self, name: &str, rhs: &str) -> String {
        if self.ints.insert(name.to_string()) {
            if self.machine {
                // with jumps, textual order no longer equals execution order,
                // so every variable is created up front with a safe value;
                // constant initializers ride along in the hoisted `let`,
                // everything else gets assigned at the actual site
                match const_init(rhs) {
                    Some(v) => {
                        self.hoisted.push(format!("    #[allow(unused_variables)]\n    let mut {name}: i64 = {v};"));
                        if v == rhs {
                            String::new()
                        } else {
                            format!("    {name} = {rhs};")
                        }
                    }
                    None => {
                        self.hoisted.push(
                            format!("    #[allow(unused_variables)]\n    let mut {name}: i64 = 0;"),
                        );
                        format!("    {name} = {rhs};")
                    }
                }
            } else {
                format!("    #[allow(unused_variables)]\n    let mut {name}: i64 = {rhs};")
            }
        } else {
            format!("    {name} = {rhs};")
        }
    }

    /// same contract as bind, but for string variables; `expr` is any rust
    /// expression evaluating to String
    fn bind_str(&mut self, name: &str, expr: &str) -> String {
        if self.strs.insert(name.to_string()) {
            if self.machine {
                self.hoisted.push(format!(
                    "    #[allow(unused_variables)]\n    let mut {name}: String = String::new();"
                ));
            } else {
                return format!(
                    "    #[allow(unused_variables)]\n    let mut {name}: String = {expr};"
                );
            }
        }
        format!("    {name} = {expr};")
    }

    /// convenience wrapper: assign a quoted literal
    fn bind_str_lit(&mut self, name: &str, quoted: &str) -> String {
        self.bind_str(name, &format!("{quoted}.to_string()"))
    }

    /// declare `name` as a string if needed, then APPEND `val`
    /// (a rust expression yielding &str/String) to it
    fn append_str(&mut self, name: &str, val: &str) -> String {
        if self.strs.insert(name.to_string()) {
            if self.machine {
                self.hoisted.push(format!(
                    "    #[allow(unused_variables)]\n    let mut {name}: String = String::new();"
                ));
            } else {
                return format!(
                    "    #[allow(unused_variables)]\n    let mut {name}: String = String::new();\n    {name} += {val};"
                );
            }
        }
        format!("    {name} += {val};")
    }

    fn require_str(&self, name: &str) -> Result<(), String> {
        if self.strs.contains(name) {
            Ok(())
        } else if self.ints.contains(name) {
            Err(format!("'{name}' is an integer variable"))
        } else {
            Err(format!("unknown variable '{name}'"))
        }
    }

    fn require_int(&self, name: &str) -> Result<(), String> {
        if self.ints.contains(name) {
            Ok(())
        } else if self.strs.contains(name) {
            Err(format!("'{name}' is a string variable, a number was expected"))
        } else {
            Err(format!("unknown variable '{name}'"))
        }
    }

    /// log can print either kind
    fn require_any(&self, name: &str) -> Result<(), String> {
        if self.ints.contains(name) || self.strs.contains(name) {
            Ok(())
        } else {
            Err(format!("unknown variable '{name}'"))
        }
    }
}

/// rust expression: read one raw line from stdin (no trailing newline);
/// clean exit on EOF
const READ_LINE_EXPR: &str = r#"{
        use std::io::BufRead;
        let mut __buf = String::new();
        let __n = std::io::stdin().lock().read_line(&mut __buf).unwrap_or(0);
        if __n == 0 {
            eprintln!("terra: unexpected end of input");
            std::process::exit(1);
        }
        while __buf.ends_with('\n') || __buf.ends_with('\r') {
            __buf.pop();
        }
        __buf
    }"#;

fn translate_in(name: &str, ctx: &mut Ctx) -> Result<String, String> {
    if !is_ident(name) {
        return Err(format!("in expects a variable name, got '{name}'"));
    }
    if ctx.strs.contains(name) {
        return Ok(ctx.bind_str(name, READ_LINE_EXPR));
    }
    let reader = read_int_expr();
    Ok(ctx.bind(name, reader))
}

fn read_int_expr() -> &'static str {
    // keeps asking until it gets a whole number; a closed stdin ends
    // the program cleanly instead of looping forever
    r#"{
        use std::io::BufRead;
        loop {
            let mut __buf = String::new();
            let __read = std::io::stdin().lock().read_line(&mut __buf).unwrap_or(0);
            if __read == 0 {
                eprintln!("terra: unexpected end of input");
                std::process::exit(1);
            }
            match __buf.trim().parse::<i64>() {
                Ok(__v) => break __v,
                Err(_) => {
                    use std::io::Write;
                    print!("(a whole number, please) ");
                    std::io::stdout().flush().ok();
                }
            }
        }
    }"#
}

/// ask."prompt".x — print the prompt without a newline, then read an
/// integer into x. the python-style cousin of `in`.
fn translate_ask(operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    let rest = operand
        .strip_prefix('"')
        .ok_or_else(|| "ask expects a quoted prompt, got '{operand}'".to_string())?;
    let close = rest
        .find('"')
        .ok_or_else(|| "unterminated prompt string".to_string())?;
    let text = escape_text(&rest[..close]);
    let var = rest[close + 1..]
        .trim()
        .strip_prefix('.')
        .ok_or_else(|| "ask expects '.<variable>' after the prompt".to_string())?;
    if !is_ident(var) {
        return Err(format!("'{var}' is not a variable name"));
    }
    if ctx.strs.contains(var) {
        let bind = ctx.bind_str(var, READ_LINE_EXPR);
        return Ok(format!("    print!(\"{text}\");\n    {{\n        use std::io::Write;\n        std::io::stdout().flush().ok();\n    }}\n{bind}"));
    }
    Ok(format!(
        "    print!(\"{text}\");\n    {{\n        use std::io::Write;\n        std::io::stdout().flush().ok();\n    }}\n{}",
        ctx.bind(var, read_int_expr())
    ))
}

fn translate_log(operand: &str, ctx: &Ctx) -> Result<String, String> {
    if let Some(rest) = operand.strip_prefix('"') {
        // log."text" / log."text".x
        let close = rest
            .find('"')
            .ok_or_else(|| "unterminated string".to_string())?;
        let text = escape_text(&rest[..close]);
        let after = rest[close + 1..].trim();
        if after.is_empty() {
            return Ok(format!(r#"    println!("{text}");"#));
        }
        let var = after
            .strip_prefix('.')
            .ok_or_else(|| "expected '.<variable>' after the string".to_string())?;
        if !is_ident(var) {
            return Err(format!("'{var}' is not a variable name"));
        }
        ctx.require_any(var)?;
        Ok(format!(r#"    println!("{text}{{}}", {var});"#))
    } else if is_ident(operand) {
        ctx.require_any(operand)?;
        Ok(format!(r#"    println!("{{}}", {operand});"#))
    } else {
        Err(format!(
            "log expects a quoted string or a variable, got '{operand}'"
        ))
    }
}

fn is_jump_verb(verb: &str) -> bool {
    verb == "j" || COND_OPS.iter().any(|(v, _)| *v == verb)
}

/// key.k. -> wait for one raw keypress, k gets its byte value.
/// the terminal is restored right after the read.
fn translate_key(operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    let var = operand
        .strip_suffix('.')
        .ok_or_else(|| "key expects '<var>.' (trailing dot), got '{operand}'".to_string())?;
    if !is_ident(var) {
        return Err(format!("'{var}' is not a variable name"));
    }
    let reader = r#"{
        use std::io::{Read, Write};
        std::process::Command::new("stty")
            .args(["-F", "/dev/tty", "-icanon", "-echo"])
            .status()
            .ok();
        let mut __b = [0u8; 1];
        let __n = std::io::stdin().lock().read(&mut __b).unwrap_or(0);
        std::process::Command::new("stty")
            .args(["-F", "/dev/tty", "sane"])
            .status()
            .ok();
        if __n == 0 {
            eprintln!("terra: unexpected end of input");
            std::process::exit(1);
        }
        std::io::stdout().flush().ok();
        i64::from(__b[0])
    }"#;
    Ok(ctx.bind(var, reader))
}

/// at.3.7."text" -> move the cursor to row 3, column 7 and print text.
/// text may contain dots; rows/columns may be integer variables.
fn translate_at(operand: &str, ctx: &Ctx) -> Result<String, String> {
    let bad = || format!("at expects '<row>.<col>.\"text\"', got '{operand}'");
    let (row, rest) = operand.split_once('.').ok_or_else(bad)?;
    let (col, quoted) = rest.split_once('.').ok_or_else(bad)?;
    for (what, part) in [("row", row), ("column", col)] {
        if !is_uint(part) && !is_ident(part) {
            return Err(format!("bad {what} '{part}'"));
        }
        if is_ident(part) && !is_uint(part) {
            ctx.require_int(part).map_err(|e| match e {
                msg if msg.starts_with("unknown") => format!("unknown {what} '{part}'"),
                msg => msg,
            })?;
        }
    }
    let text = quoted
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .ok_or_else(bad)?;
    let text = escape_text(text);
    const FLUSH: &str = "\n    {\n        use std::io::Write;\n        std::io::stdout().flush().ok();\n    }";
    if is_uint(row) && is_uint(col) {
        Ok(format!(r#"    print!("\x1b[{row};{col}H{text}");{FLUSH}"#))
    } else {
        // variable rows/columns must be interpolated at run time
        Ok(format!(
            r#"    print!("\x1b[{{}};{{}}H{text}", {row}, {col});{FLUSH}"#
        ))
    }
}

/// parse a fully-quoted string operand into its escaped rust literal
fn quoted_literal(operand: &str, what: &str) -> Result<String, String> {
    let inner = operand
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .ok_or_else(|| format!("unterminated {what} string"))?;
    Ok(format!("\"{}\"", escape_text(inner)))
}

/// rnd.x.50 -> x becomes a random whole number in 0..50.
/// seeded from the clock nanos with an xor-shift; plenty for games.
fn translate_rnd(operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    let parts: Vec<&str> = operand.split('.').collect();
    if parts.len() != 2 {
        return Err(format!("rnd expects '<var>.<limit>', got '{operand}'"));
    }
    let (var, limit) = (parts[0], parts[1]);
    if !is_ident(var) {
        return Err(format!("'{var}' is not a variable name"));
    }
    if ctx.strs.contains(var) {
        return Err(format!(
            "'{var}' is a string variable, a number was expected"
        ));
    }
    if !is_uint(limit) {
        return Err(format!("bad limit '{limit}' (whole number expected)"));
    }
    let expr = format!(
        r#"{{
            use std::time::{{SystemTime, UNIX_EPOCH}};
            let __n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0);
            ((__n ^ (__n << 13) ^ (__n >> 7)) % {limit}.max(1)) as i64
        }}"#
    );
    Ok(ctx.bind(var, &expr))
}

/// j.label / jeq.a.b.label family -> state machine moves
fn translate_jump(
    verb: &str,
    operand: &str,
    ctx: &Ctx,
    labels: &HashSet<String>,
) -> Result<String, String> {
    if verb == "j" {
        if !labels.contains(operand) {
            return Err(format!("unknown label '{operand}'"));
        }
        return Ok(format!(
            "    __pc = \"{operand}\";\n    continue '__run;"
        ));
    }
    let op = COND_OPS
        .iter()
        .find(|(v, _)| *v == verb)
        .map(|(_, o)| *o)
        .unwrap();
    let (a, rest) = operand
        .split_once('.')
        .ok_or_else(|| format!("{verb} expects '<var>.<value>.<label>', got '{operand}'"))?;
    if !is_ident(a) {
        return Err(format!("'{a}' is not a variable name"));
    }
    // the compared value may be a quoted string whose text contains dots,
    // e.g. jeq.msg."yes sir".over — so locate the label after the closing quote
    let (b_expr, target) = if rest.starts_with('"') {
        let close = quoted_end(rest).ok_or_else(|| {
            format!("{verb}: unclosed string in '{operand}'")
        })?;
        let inner = &rest[1..close];
        let label = rest[close + 1..]
            .strip_prefix('.')
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                format!("{verb} expects '<var>.\"text\".<label>', got '{operand}'")
            })?;
        ctx.require_str(a)?;
        (
            format!("\"{}\"", escape_text(inner)),
            label,
        )
    } else {
        let parts: Vec<&str> = rest.split('.').collect();
        if parts.len() != 2 {
            return Err(format!(
                "{verb} expects '<var>.<value>.<label>', got '{operand}'"
            ));
        }
        let (b, target) = (parts[0], parts[1]);
        let b_expr = if is_uint(b) {
            ctx.require_int(a)?;
            b.to_string()
        } else if is_ident(b) {
            if ctx.strs.contains(b) {
                ctx.require_str(a)?;
                b.to_string()
            } else {
                ctx.require_int(a)?;
                ctx.require_int(b)?;
                b.to_string()
            }
        } else {
            return Err(format!(
                "bad comparison operand '{b}' (number, variable or \"string\" expected)"
            ));
        };
        (b_expr, target)
    };
    if !labels.contains(target) {
        return Err(format!("unknown label '{target}'"));
    }
    Ok(format!(
        "    if {a} {op} {b_expr} {{\n        __pc = \"{target}\";\n        continue '__run;\n    }}"
    ))
}

/// index of the closing '"' for a string starting at `s[0]`,
/// honoring \" escapes; None if unterminated
fn quoted_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// rust expression: seconds since the unix epoch
const NOW_EXPR: &str =
    "{ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }";

fn args2<'a>(operand: &'a str, verb: &str) -> Result<(&'a str, &'a str), String> {
    operand
        .split_once('.')
        .ok_or_else(|| format!("{verb} expects '<var>.<var>', got '{operand}'"))
}

fn args3<'a>(operand: &'a str, verb: &str) -> Result<(&'a str, &'a str, &'a str), String> {
    let parts: Vec<&str> = operand.split('.').collect();
    if parts.len() != 3 {
        return Err(format!("{verb} expects three dot-separated parts, got '{operand}'"));
    }
    Ok((parts[0], parts[1], parts[2]))
}

fn require_idents(verb: &str, vs: &[&str], operand: &str) -> Result<(), String> {
    if vs.iter().all(|v| is_ident(v)) {
        Ok(())
    } else {
        Err(format!("{verb}: bad operands '{operand}'"))
    }
}

fn translate_len(operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    let (n, s) = args2(operand, "len")?;
    require_idents("len", &[n, s], operand)?;
    ctx.require_str(s)?;
    // chars, not bytes — cyrillic must count as one letter
    Ok(ctx.bind(n, &format!("{s}.chars().count() as i64")))
}

fn translate_case(up: bool, operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    let verb = if up { "up" } else { "low" };
    let (d, s) = args2(operand, verb)?;
    require_idents(verb, &[d, s], operand)?;
    ctx.require_str(s)?;
    if ctx.ints.contains(d) {
        return Err(format!("'{d}' is an integer variable"));
    }
    let m = if up { "to_uppercase" } else { "to_lowercase" };
    Ok(ctx.bind_str(d, &format!("{s}.{m}()")))
}

fn translate_abs(operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    let (d, x) = args2(operand, "abs")?;
    require_idents("abs", &[d, x], operand)?;
    ctx.require_int(x)?;
    Ok(ctx.bind(d, &format!("{x}.abs()")))
}

fn translate_minmax(op: &str, operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    let (m, a, b) = args3(operand, op)?;
    require_idents(op, &[m, a, b], operand)?;
    ctx.require_int(a)?;
    ctx.require_int(b)?;
    Ok(ctx.bind(m, &format!("{a}.{op}({b})")))
}

fn translate_now(operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    if !is_ident(operand) {
        return Err(format!("now expects a variable name, got '{operand}'"));
    }
    Ok(ctx.bind(operand, NOW_EXPR))
}

fn translate_bell() -> Result<String, String> {
    Ok(
        "    print!(\"\\u{7}\");\n    {\n        use std::io::Write;\n        std::io::stdout().flush().ok();\n    }"
            .to_string(),
    )
}

fn translate_chr(operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    let (s, n) = args2(operand, "chr")?;
    if !is_ident(s) || (!is_uint(n) && !is_ident(n)) {
        return Err(format!("chr expects '<string-var>.<code>', got '{operand}'"));
    }
    if ctx.ints.contains(s) {
        return Err(format!("'{s}' is an integer variable"));
    }
    let v = if is_uint(n) {
        n.to_string()
    } else {
        ctx.require_int(n)?;
        n.to_string()
    };
    Ok(ctx.bind_str(
        s,
        &format!(
            "{{ (char::from_u32({v} as u32).unwrap_or('\\0')).to_string() }}"
        ),
    ))
}

fn translate_ord(operand: &str, ctx: &mut Ctx) -> Result<String, String> {
    let (n, s) = args2(operand, "ord")?;
    require_idents("ord", &[n, s], operand)?;
    ctx.require_str(s)?;
    Ok(ctx.bind(
        n,
        &format!("{s}.chars().next().map(|c| c as i64).unwrap_or(0)"),
    ))
}

/// cut a trailing '# ...' comment, but only outside string literals
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..i],
            _ => {}
        }
    }
    line
}

fn translate(
    raw: &str,
    ctx: &mut Ctx,
    labels: &HashSet<String>,
) -> Result<String, String> {
    let line = strip_comment(raw.trim()).trim_end();
    if line.is_empty() || line.starts_with('#') {
        return Ok(String::new());
    }

    let (verb, operand) = line
        .split_once('.')
        .ok_or_else(|| format!("expected '<verb>.<operand>', got '{line}'"))?;

    match verb {
        "log" => translate_log(operand, ctx),
        "in" => translate_in(operand, ctx),
        "ask" => translate_ask(operand, ctx),
        "rnd" => translate_rnd(operand, ctx),
        "at" => translate_at(operand, ctx),
        "key" => translate_key(operand, ctx),
        "len" => translate_len(operand, ctx),
        "up" => translate_case(true, operand, ctx),
        "low" => translate_case(false, operand, ctx),
        "abs" => translate_abs(operand, ctx),
        "min" | "max" => translate_minmax(verb, operand, ctx),
        "now" => translate_now(operand, ctx),
        "bell" => translate_bell(),
        "chr" => translate_chr(operand, ctx),
        "ord" => translate_ord(operand, ctx),
        "cls" => {
            if !operand.is_empty() {
                return Err("cls takes no operand".to_string());
            }
            Ok(
                r#"    print!("\x1b[2J\x1b[H");
    {
        use std::io::Write;
        std::io::stdout().flush().ok();
    }"#
                .to_string(),
            )
        }
        "w" => {
            if !is_uint(operand) {
                return Err(format!("w expects milliseconds, got '{operand}'"));
            }
            Ok(format!(
                "    std::thread::sleep(std::time::Duration::from_millis({operand}));"
            ))
        }
        "q" => {
            if operand.is_empty() {
                if ctx.machine {
                    Ok("    break '__run;".to_string())
                } else {
                    Ok("    return;".to_string())
                }
            } else if is_uint(operand) && operand.len() <= 3 {
                Ok(format!(
                    "    {{\n        use std::io::Write;\n        std::io::stdout().flush().ok();\n    }}\n    std::process::exit({operand});"
                ))
            } else {
                Err(format!(
                    "q expects no operand or an exit code 0-255, got '{operand}'"
                ))
            }
        }
        v if is_jump_verb(v) => translate_jump(v, operand, ctx, labels),
        target => {
            if !is_ident(target) {
                return Err(format!("unknown command '{target}'"));
            }
            if RESERVED.contains(&target) {
                return Err(format!("'{target}' is a reserved word"));
            }
            if operand.is_empty() {
                return Err(format!("missing operand for '{target}'"));
            }
            if is_uint(operand) {
                return Ok(ctx.bind(target, operand));
            }
            if let Some(rest) = operand.strip_prefix(['+', '-', '*', '/', '%']) {
                // string concatenation first: s.+"more" or s.+other_str
                let op = &operand[..1];
                if rest.starts_with('"') || (is_ident(rest) && ctx.strs.contains(rest)) {
                    if op != "+" {
                        return Err("strings support only .+ (concatenation)".to_string());
                    }
                    if ctx.ints.contains(target) {
                        return Err(format!("'{target}' is an integer variable"));
                    }
                    return if rest.starts_with('"') {
                        let lit = quoted_literal(rest, "string")?;
                        Ok(ctx.append_str(target, &lit))
                    } else {
                        Ok(ctx.append_str(target, &format!("&{rest}")))
                    };
                }
                // x.+5, x.-2, x.+score, ... modify in place; the part after
                // the sign may be a literal or another variable.
                // x.-N on a fresh variable declares a negative literal instead
                if !is_uint(rest) && !is_ident(rest) {
                    return Err(format!(
                        "bad arithmetic operand '{operand}' (expected +N/-N/*N//N/%N or +var)"
                    ));
                }
                let op = &operand[..1];
                if (op == "/" || op == "%") && rest == "0" {
                    return Err(format!("division by zero ({op}0)"));
                }
                if op == "-" && is_uint(rest) && !ctx.ints.contains(target) {
                    return Ok(ctx.bind(target, &format!("-{rest}")));
                }
                let rhs = if is_uint(rest) {
                    rest.to_string()
                } else {
                    ctx.require_int(rest)?;
                    rest.to_string()
                };
                ctx.require_int(target)?;
                return Ok(format!("    {target} = {target} {op} {rhs};"));
            }
            // msg."hello" -> string variable assignment
            if operand.starts_with('"') {
                if ctx.ints.contains(target) {
                    return Err(format!("'{target}' is an integer variable"));
                }
                let lit = quoted_literal(operand, "string")?;
                return Ok(ctx.bind_str_lit(target, &lit));
            }
            if ctx.strs.contains(target) {
                return Err(format!(
                    "'{target}' is a string variable, \"text\" expected"
                ));
            }
            if is_ident(operand) {
                if ctx.strs.contains(operand) {
                    if ctx.ints.contains(target) {
                        return Err(format!("'{target}' is an integer variable"));
                    }
                    return Ok(ctx.bind_str(target, &format!("{operand}.clone()")));
                }
                ctx.require_int(operand)?;
                return Ok(ctx.bind(target, operand));
            }
            Err(format!(
                "unknown operand '{operand}' (integer, +N/-N/*N//N/-N or variable expected)"
            ))
        }
    }
}

/// shift a statement block deeper so it fits inside a match arm
fn indent_block(code: &str, spaces: usize) -> String {
    code.lines()
        .map(|l| {
            if l.is_empty() {
                l.to_string()
            } else {
                format!("{}{}", " ".repeat(spaces), l)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// whole program -> rust body or "line N: ..." errors.
///
/// linear mode (no labels/jumps): statements in order, plain body.
///
/// machine mode (any label or jump present): variables hoisted above a
/// labeled loop, each label becomes a match arm, jumps move __pc.
/// backward jumps work because nothing depends on textual position.
fn compile(source: &str) -> Result<String, Vec<String>> {
    let statements = split_statements(source);

    // pass one: labels, duplicates, flow detection
    let mut labels: HashSet<String> = HashSet::new();
    let mut label_order: Vec<String> = Vec::new();
    let mut has_flow = false;
    let mut errors: Vec<String> = Vec::new();

    for (no, text) in &statements {
        let line = text.trim();
        if let Some(name) = line.strip_prefix(':') {
            if !is_ident(name) {
                errors.push(format!("line {no}: bad label '{line}'"));
            } else if !labels.insert(name.to_string()) {
                errors.push(format!("line {no}: duplicate label '{name}'"));
            } else {
                label_order.push(name.to_string());
            }
            continue;
        }
        if let Some((verb, _)) = line.split_once('.') {
            if is_jump_verb(verb) {
                has_flow = true;
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let machine = has_flow || !label_order.is_empty();

    // segment per label, plus the implicit prologue before the first one
    let mut seg_index: HashMap<String, usize> = HashMap::new();
    let mut seg_ids: Vec<String> = vec!["@begin".to_string()];
    for name in &label_order {
        seg_index.insert(name.clone(), seg_ids.len());
        seg_ids.push(name.clone());
    }
    let mut segments: Vec<Vec<String>> = vec![Vec::new(); seg_ids.len()];
    let mut current = 0usize;

    let mut ctx = Ctx::new(machine);

    for (no, text) in &statements {
        let trimmed = text.trim();
        if let Some(name) = trimmed.strip_prefix(':') {
            if is_ident(name) {
                current = seg_index[name];
            }
            continue;
        }
        match translate(text, &mut ctx, &labels) {
            Ok(code) if !code.is_empty() => segments[current].push(code),
            Ok(_) => {}
            Err(msg) => errors.push(format!("line {no}: {msg}")),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    if !machine {
        let mut body = String::new();
        for seg in &segments {
            for stmt in seg {
                body.push_str(stmt);
                body.push('\n');
            }
        }
        return Ok(body);
    }

    // state machine assembly
    let mut body = String::new();
    for h in &ctx.hoisted {
        body.push_str(h);
        body.push('\n');
    }
    body.push_str("    let mut __pc = \"@begin\";\n");
    body.push_str("    '__run: loop {\n");
    body.push_str("        match __pc {\n");
    for (idx, id) in seg_ids.iter().enumerate() {
        body.push_str(&format!("            \"{id}\" => {{\n"));
        for stmt in &segments[idx] {
            body.push_str(&indent_block(stmt, 12));
            body.push('\n');
        }
        // if the segment already exits (q., q.N or trailing jump), no fallthrough
        let exits = segments[idx]
            .last()
            .map(|s| {
                s.trim_end().ends_with("continue '__run;")
                    || s.contains("break '__run;")
                    || s.contains("std::process::exit(")
            })
            .unwrap_or(false);
        if !exits {
            if idx + 1 < seg_ids.len() {
                body.push_str(&format!(
                    "                __pc = \"{}\";\n                continue '__run;\n",
                    seg_ids[idx + 1]
                ));
            } else {
                body.push_str("                break '__run;\n");
            }
        }
        body.push_str("            }\n");
    }
    body.push_str("            _ => {}\n");
    body.push_str("        }\n");
    body.push_str("    }\n");
    Ok(body)
}

/// statements are separated by newlines OR by ';'.
/// returns (source_line_number, statement) so diagnostics stay honest;
/// ';' inside string literals is content, not a separator.
fn split_statements(source: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (idx, raw) in source.lines().enumerate() {
        let mut current = String::new();
        let mut in_string = false;
        for c in raw.chars() {
            match c {
                '"' => {
                    in_string = !in_string;
                    current.push(c);
                }
                // rest of the physical line is a comment
                '#' if !in_string => break,
                ';' if !in_string => out.push((idx + 1, std::mem::take(&mut current))),
                _ => current.push(c),
            }
        }
        out.push((idx + 1, current));
    }
    out
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("terra {VERSION}");
        return;
    }
    if args.len() < 2 || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("terra compiler {VERSION}");
        eprintln!("usage: terra_compiler <script.tr> [--emit-rust]");
        eprintln!("       terra_compiler -e 'x.6; log.\"six = \".x; q.'");
        eprintln!("  --emit-rust   print generated rust instead of running it");
        eprintln!("  -e <program>  run a one-liner right on the command line");
        exit(if args.len() < 2 { 1 } else { 0 });
    }

    let emit_only = args.iter().any(|a| a == "--emit-rust");

    // -e takes the next argument as the whole program
    let content = if let Some(pos) = args.iter().position(|a| a == "-e") {
        match args.get(pos + 1) {
            Some(program) => program.clone(),
            None => {
                eprintln!("terra: -e expects a program string");
                exit(1);
            }
        }
    } else {
        let path = &args[1];
        fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("terra: cannot read '{path}': {e}");
            eprintln!("       (tip: for one-liners use  terra_compiler -e 'x.10; log.x; q.')");
            exit(1);
        })
    };

    let body = match compile(&content) {
        Ok(b) => b,
        Err(errors) => {
            for e in &errors {
                eprintln!("terra: error: {e}");
            }
            eprintln!("terra: {} error(s), nothing built", errors.len());
            exit(1);
        }
    };

    let rust_code =
        format!("#[allow(unused_assignments, unused_mut)]\nfn main() {{\n{body}}}\n");

    if emit_only {
        print!("{rust_code}");
        return;
    }

    fs::create_dir_all("terra_build/src").unwrap();
    fs::write("terra_build/Cargo.toml", GENERATED_CARGO_TOML).unwrap();
    fs::write("terra_build/src/main.rs", &rust_code).unwrap();

    let status = Command::new("cargo")
        .args(["run", "--quiet", "--manifest-path", "terra_build/Cargo.toml"])
        .status()
        .unwrap_or_else(|e| {
            eprintln!("terra: failed to start cargo: {e}");
            exit(1);
        });
    if !status.success() {
        exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(line: &str) -> Result<String, String> {
        translate(line, &mut Ctx::new(false), &HashSet::new())
    }

    fn t_machine(line: &str) -> String {
        translate(line, &mut Ctx::new(true), &HashSet::new()).unwrap()
    }

    fn compile_str(lines: &[&str]) -> Result<String, Vec<String>> {
        compile(&lines.join("\n"))
    }

    #[test]
    fn assignment_then_reassignment() {
        let mut ctx = Ctx::new(false);
        assert_eq!(translate("x.10", &mut ctx, &HashSet::new()).unwrap(), "    #[allow(unused_variables)]\n    let mut x: i64 = 10;");
        assert_eq!(translate("x.20", &mut ctx, &HashSet::new()).unwrap(), "    x = 20;");
    }

    #[test]
    fn arithmetic_ops() {
        let mut ctx = Ctx::new(false);
        translate("x.10", &mut ctx, &HashSet::new()).unwrap();
        assert_eq!(translate("x.+5", &mut ctx, &HashSet::new()).unwrap(), "    x = x + 5;");
        assert_eq!(translate("x.-2", &mut ctx, &HashSet::new()).unwrap(), "    x = x - 2;");
        assert_eq!(translate("x.*3", &mut ctx, &HashSet::new()).unwrap(), "    x = x * 3;");
        assert_eq!(translate("x./4", &mut ctx, &HashSet::new()).unwrap(), "    x = x / 4;");
    }

    #[test]
    fn arithmetic_on_undeclared_is_error() {
        assert!(t("y.+5").unwrap_err().contains("unknown variable 'y'"));
    }

    #[test]
    fn negative_literal_declares_when_new() {
        assert_eq!(t("x.-5").unwrap(), "    #[allow(unused_variables)]\n    let mut x: i64 = -5;");
    }

    #[test]
    fn negative_literal_still_subtracts_when_declared() {
        let mut ctx = Ctx::new(false);
        translate("x.10", &mut ctx, &HashSet::new()).unwrap();
        assert_eq!(translate("x.-5", &mut ctx, &HashSet::new()).unwrap(), "    x = x - 5;");
    }

    #[test]
    fn bad_arithmetic_operand() {
        let mut ctx = Ctx::new(false);
        translate("x.1", &mut ctx, &HashSet::new()).unwrap();
        // 'abc' would be a valid variable name; this is genuinely malformed
        assert!(t("x.+ab!c").unwrap_err().contains("bad arithmetic operand"));
    }

    #[test]
    fn division_by_zero_is_compile_error() {
        let mut ctx = Ctx::new(false);
        translate("x.1", &mut ctx, &HashSet::new()).unwrap();
        assert!(t("x./0").unwrap_err().contains("division by zero"));
        assert!(t("y./0").unwrap_err().contains("division by zero"));
    }

    // ---------- variable arithmetic ----------

    #[test]
    fn in_place_ops_accept_variables() {
        let mut ctx = declared_ctx();
        for (line, expected) in [
            ("i.+k", "    i = i + k;"),
            ("i.-k", "    i = i - k;"),
            ("i.*k", "    i = i * k;"),
            ("i./k", "    i = i / k;"),
            ("i.%k", "    i = i % k;"),
        ] {
            let out = translate(line, &mut ctx.clone(), &HashSet::new()).unwrap();
            assert_eq!(out, expected, "{line}");
        }
    }

    #[test]
    fn in_place_var_rhs_must_be_declared() {
        let mut ctx = Ctx::new(false);
        translate("i.1", &mut ctx, &HashSet::new()).unwrap();
        assert!(t("i.+nope").unwrap_err().contains("unknown variable 'nope'"));
    }

    #[test]
    fn modulo_by_zero_is_compile_error() {
        let mut ctx = Ctx::new(false);
        translate("x.1", &mut ctx, &HashSet::new()).unwrap();
        assert!(t("x.%0").unwrap_err().contains("division by zero"));
    }

    // ---------- rnd and cls ----------

    #[test]
    fn rnd_declares_and_uses_modulo_range() {
        let out = t("rnd.x.50").unwrap();
        assert!(out.contains("let mut x: i64"), "{out}");
        assert!(out.contains("% 50.max(1)"), "{out}");
    }

    #[test]
    fn rnd_reuses_existing_variable() {
        let mut ctx = Ctx::new(false);
        translate("rnd.x.10", &mut ctx, &HashSet::new()).unwrap();
        let out = translate("rnd.x.20", &mut ctx, &HashSet::new()).unwrap();
        assert!(out.starts_with("    x = "), "{out}");
        assert!(out.contains("% 20.max(1)"));
    }

    #[test]
    fn rnd_bad_shapes_are_errors() {
        assert!(t("rnd.x").unwrap_err().contains("'<var>.<limit>'"));
        assert!(t("rnd.x.a").unwrap_err().contains("bad limit"));
        assert!(t("rnd.5.10").unwrap_err().contains("not a variable name"));
    }

    #[test]
    fn cls_clears_screen_with_ansi() {
        let out = t("cls.").unwrap();
        assert!(out.contains("\\x1b[2J"));
        assert!(out.contains("flush"));
        assert!(t("cls.5").unwrap_err().contains("takes no operand"));
    }

    // ---------- strings ----------

    #[test]
    fn quoted_operand_declares_string_variable() {
        let out = t("msg.\"hello\"").unwrap();
        assert_eq!(out, "    #[allow(unused_variables)]\n    let mut msg: String = \"hello\".to_string();");
        let mut ctx = Ctx::new(false);
        translate("msg.\"a\"", &mut ctx, &HashSet::new()).unwrap();
        let out = translate("msg.\"b\"", &mut ctx, &HashSet::new()).unwrap();
        assert_eq!(out, "    msg = \"b\".to_string();");
    }

    #[test]
    fn string_escapes_survive_assignment() {
        let out = t("m.\"say \\\"hi\\\"\"").unwrap();
        assert!(out.contains("\\\"hi\\\""), "{out}");
    }

    #[test]
    fn log_accepts_strings_and_ints() {
        let mut ctx = Ctx::new(false);
        translate("msg.\"hi\"", &mut ctx, &HashSet::new()).unwrap();
        translate("n.1", &mut ctx, &HashSet::new()).unwrap();
        for line in ["log.msg", "log.n", "log.\"t\".msg", "log.\"t\".n"] {
            translate(line, &mut ctx, &HashSet::new()).unwrap();
        }
    }

    #[test]
    fn type_mismatches_are_errors() {
        let mut ctx = Ctx::new(false);
        translate("msg.\"hi\"", &mut ctx, &HashSet::new()).unwrap();
        translate("n.1", &mut ctx, &HashSet::new()).unwrap();
        assert!(translate("n.msg", &mut ctx, &HashSet::new())
            .unwrap_err()
            .contains("'n' is an integer variable"));
        assert!(translate("n.+msg", &mut ctx, &HashSet::new())
            .unwrap_err()
            .contains("'n' is an integer variable"));
        assert!(translate("in.msg", &mut ctx, &HashSet::new())
            .unwrap()
            .contains("read_line"));
        assert!(translate("rnd.msg.5", &mut ctx, &HashSet::new())
            .unwrap_err()
            .contains("'msg' is a string variable"));
        assert!(translate("msg.n", &mut ctx, &HashSet::new())
            .unwrap_err()
            .contains("'msg' is a string variable"));
        // comparing an int variable against a quoted string is rejected
        translate("nq.7", &mut ctx, &HashSet::new()).unwrap();
        assert!(translate("jeq.nq.\"7\".l", &mut ctx, &HashSet::new())
            .unwrap_err()
            .contains("'nq' is an integer variable"));
    }

    #[test]
    fn string_hoists_empty_in_machine_mode() {
        let out = t_machine("msg.\"hi\"");
        assert_eq!(out, "    msg = \"hi\".to_string();");
        // and the hoisted declaration exists when jumps force machine mode
        let prog = compile_str(&["msg.\"hi\"", "j.end", ":end", "q."]).unwrap();
        assert!(prog.contains("let mut msg: String = String::new();"), "{prog}");
    }

    // ---------- at ----------

    #[test]
    fn at_positions_the_cursor() {
        let out = t("at.3.7.\"X\"").unwrap();
        assert_eq!(
            out,
            "    print!(\"\\x1b[3;7HX\");\n    {\n        use std::io::Write;\n        std::io::stdout().flush().ok();\n    }"
        );

        let mut ctx = Ctx::new(false);
        translate("r.1", &mut ctx, &HashSet::new()).unwrap();
        translate("c.2", &mut ctx, &HashSet::new()).unwrap();
        let out = translate("at.r.c.\"@\"", &mut ctx, &HashSet::new()).unwrap();
        assert_eq!(
            out,
            "    print!(\"\\x1b[{};{}H@\", r, c);\n    {\n        use std::io::Write;\n        std::io::stdout().flush().ok();\n    }"
        );
        let out = translate("at.r.5.\"@\"", &mut ctx, &HashSet::new()).unwrap();
        assert!(out.contains("\"\\x1b[{};{}H@\", r, 5)"), "{out}");
        assert!(out.contains("stdout().flush()"));
    }

    #[test]
    fn at_text_may_contain_dots() {
        let out = t("at.1.1.\"a.b.c\"").unwrap();
        assert_eq!(
            out,
            "    print!(\"\\x1b[1;1Ha.b.c\");\n    {\n        use std::io::Write;\n        std::io::stdout().flush().ok();\n    }"
        );
    }

    #[test]
    fn at_bad_shapes_are_errors() {
        assert!(t("at.1.1").is_err());
        assert!(t("at.x.1.\"Y\"").unwrap_err().contains("unknown row"));
        assert!(t("at.1.x.\"Y\"").unwrap_err().contains("unknown column"));
        assert!(t("at.! .1.\"Y\"").unwrap_err().contains("bad row"));
        assert!(t("at.1.1.unterminated").unwrap_err().contains("at expects"));
        let mut ctx = Ctx::new(false);
        translate("s.\"str\"", &mut ctx, &HashSet::new()).unwrap();
        assert!(translate("at.s.1.\"Y\"", &mut ctx, &HashSet::new())
            .unwrap_err()
            .contains("string variable"));
    }

    // ---------- key ----------

    #[test]
    fn key_reads_a_single_byte_into_an_int_var() {
        let out = t("key.k.").unwrap();
        assert!(out.contains("let mut k: i64 = {"), "{out}");
        assert!(out.contains("-icanon"), "{out}");
        assert!(out.contains("i64::from(__b[0])"));
        assert!(out.contains("\"sane\""));
    }

    #[test]
    fn key_reuses_existing_variable() {
        let mut ctx = Ctx::new(false);
        translate("k.0", &mut ctx, &HashSet::new()).unwrap();
        let out = translate("key.k.", &mut ctx, &HashSet::new()).unwrap();
        assert!(out.starts_with("    k = "), "{out}");
    }

    #[test]
    fn key_requires_trailing_dot() {
        assert!(t("key.k").unwrap_err().contains("trailing dot"));
        assert!(t("key.5.").unwrap_err().contains("not a variable name"));
    }

    // ---------- prompts flush immediately ----------

    #[test]
    fn ask_and_at_flush_stdout() {
        let out = t("ask.\"n: \".x").unwrap();
        assert!(out.contains("stdout().flush()"), "{out}");
        let out = t("at.1.1.\"X\"").unwrap();
        assert!(out.contains("stdout().flush()"), "{out}");
    }

    #[test]
    fn reader_reprompts_instead_of_panic() {
        assert!(read_int_expr().contains("a whole number, please"));
        assert!(!read_int_expr().contains("expect("));
    }

    #[test]
    fn copy_variable() {
        let mut ctx = Ctx::new(false);
        translate("a.7", &mut ctx, &HashSet::new()).unwrap();
        assert_eq!(translate("b.a", &mut ctx, &HashSet::new()).unwrap(), "    #[allow(unused_variables)]\n    let mut b: i64 = a;");
    }

    #[test]
    fn copy_from_undeclared_is_error() {
        assert!(t("b.zz").unwrap_err().contains("unknown variable 'zz'"));
    }

    #[test]
    fn log_text_and_escaping() {
        assert_eq!(
            t(r#"log."hi {there}""#).unwrap(),
            r#"    println!("hi {{there}}");"#
        );
        assert!(t(r#"log."say "hi""#).is_err()); // inner quote ends the string early
    }

    #[test]
    fn log_variable() {
        let mut ctx = Ctx::new(false);
        translate("n.1", &mut ctx, &HashSet::new()).unwrap();
        assert_eq!(translate("log.n", &mut ctx, &HashSet::new()).unwrap(), "    println!(\"{}\", n);");
    }

    #[test]
    fn log_unknown_variable_is_error() {
        assert!(t("log.nope").unwrap_err().contains("unknown variable 'nope'"));
    }

    #[test]
    fn log_text_plus_variable() {
        let mut ctx = Ctx::new(false);
        translate("n.1", &mut ctx, &HashSet::new()).unwrap();
        assert_eq!(
            translate(r#"log."value = ".n"#, &mut ctx, &HashSet::new()).unwrap(),
            r#"    println!("value = {}", n);"#
        );
    }

    #[test]
    fn unterminated_string_is_error() {
        assert!(t(r#"log."oops"#).unwrap_err().contains("unterminated"));
    }

    #[test]
    fn wait_command() {
        assert!(t("w.500")
            .unwrap()
            .contains("Duration::from_millis(500)"));
        assert!(t("w.abc").unwrap_err().contains("expects milliseconds"));
    }

    #[test]
    fn quit_command() {
        assert_eq!(t("q.").unwrap(), "    return;");
        assert!(t("q.42").unwrap().contains("exit(42)"));
        assert!(t("q.abc").unwrap_err().contains("exit code"));
    }

    #[test]
    fn string_input_reads_line() {
        let mut ctx = Ctx::new(false);
        translate("msg.\"\"", &mut ctx, &HashSet::new()).unwrap();
        let out = translate("in.msg", &mut ctx, &HashSet::new()).unwrap();
        assert!(out.contains("read_line"));
        assert!(out.contains("__buf"));
        // fresh names keep numeric input
        assert!(!translate("in.age", &mut ctx, &HashSet::new())
            .unwrap()
            .contains("ends_with"));
    }

    #[test]
    fn ask_prompt_for_string_var() {
        let mut ctx = Ctx::new(false);
        translate("name.\"anon\"", &mut ctx, &HashSet::new()).unwrap();
        let out = translate("ask.\"who? \".name", &mut ctx, &HashSet::new()).unwrap();
        assert!(out.contains("who? "));
        assert!(out.contains("read_line"));
    }

    #[test]
    fn conditions_compare_strings() {
        let labels = HashSet::from(["over".to_string()]);
        let mut ctx = Ctx::new(true);
        translate("msg.\"yes\"", &mut ctx, &labels).unwrap();
        assert!(
            translate("jeq.msg.\"yes\".over", &mut ctx, &labels)
                .unwrap()
                .contains("if msg == \"yes\"")
        );
        assert!(
            translate("jne.msg.\"no\".over", &mut ctx, &labels)
                .unwrap()
                .contains("if msg != \"no\"")
        );
        // dots inside the literal don't break label parsing
        assert!(
            translate("jeq.msg.\"a.b\".over", &mut ctx, &labels)
                .unwrap()
                .contains("__pc = \"over\"")
        );
        assert!(translate("jeq.msg.\"oops.over", &mut ctx, &labels)
            .unwrap_err()
            .contains("unclosed"));
        // string vs string variable
        translate("want.\"yes\"", &mut ctx, &labels).unwrap();
        assert!(
            translate("jeq.msg.want.over", &mut ctx, &labels)
                .unwrap()
                .contains("if msg == want")
        );
        // mixed types are rejected
        translate("n5.5", &mut ctx, &labels).unwrap();
        assert!(
            translate("jeq.n5.\"5\".over", &mut ctx, &labels)
                .unwrap_err()
                .contains("'n5' is an integer variable")
        );
        assert!(
            translate("jeq.msg.n5.over", &mut ctx, &labels)
                .unwrap_err()
                .contains("'msg' is a string variable")
        );
    }

    #[test]
    fn string_concat() {
        let mut ctx = Ctx::new(false);
        translate("s.\"a\"", &mut ctx, &HashSet::new()).unwrap();
        assert_eq!(
            translate("s.+\"b\"", &mut ctx, &HashSet::new()).unwrap(),
            "    s += \"b\";"
        );
        translate("t2.\"x\"", &mut ctx, &HashSet::new()).unwrap();
        assert_eq!(
            translate("s.+t2", &mut ctx, &HashSet::new()).unwrap(),
            "    s += &t2;"
        );
        // fresh target becomes a new string variable
        let out = translate("u2.+s", &mut ctx, &HashSet::new()).unwrap();
        assert!(out.contains("let mut u2: String = String::new()"));
        assert!(out.contains("u2 += &s"));
        // only + is defined for strings
        assert!(translate("s.-\"z\"", &mut ctx, &HashSet::new())
            .unwrap_err()
            .contains("only .+"));
    }

    #[test]
    fn copy_string_variable() {
        let mut ctx = Ctx::new(false);
        translate("greet.\"hi\"", &mut ctx, &HashSet::new()).unwrap();
        assert!(
            translate("b2.greet", &mut ctx, &HashSet::new())
                .unwrap()
                .contains("let mut b2: String = greet.clone()")
        );
        translate("num7.7", &mut ctx, &HashSet::new()).unwrap();
        assert!(translate("num7.greet", &mut ctx, &HashSet::new())
            .unwrap_err()
            .contains("'num7' is an integer variable"));
    }

    #[test]
    fn string_helper_verbs() {
        let mut ctx = Ctx::new(false);
        translate("s.\"abc\"", &mut ctx, &HashSet::new()).unwrap();
        let out = translate("len.n2.s", &mut ctx, &HashSet::new()).unwrap();
        assert!(out.contains("s.chars().count() as i64"));
        assert!(
            translate("up.upd.s", &mut ctx, &HashSet::new())
                .unwrap()
                .contains("s.to_uppercase()")
        );
        assert!(
            translate("low.lod.s", &mut ctx, &HashSet::new())
                .unwrap()
                .contains("s.to_lowercase()")
        );
        assert!(
            translate("chr.ch.65", &mut ctx, &HashSet::new())
                .unwrap()
                .contains("from_u32(65 as u32)")
        );
        assert!(
            translate("ord.od.s", &mut ctx, &HashSet::new())
                .unwrap()
                .contains("s.chars().next()")
        );
        // chr refuses an integer target
        translate("i9.9", &mut ctx, &HashSet::new()).unwrap();
        assert!(translate("chr.i9.65", &mut ctx, &HashSet::new())
            .unwrap_err()
            .contains("'i9' is an integer variable"));
    }

    #[test]
    fn int_helper_verbs() {
        let mut ctx = Ctx::new(false);
        translate("x9.-3", &mut ctx, &HashSet::new()).unwrap();
        assert!(
            translate("abs.ad.x9", &mut ctx, &HashSet::new())
                .unwrap()
                .contains("x9.abs()")
        );
        translate("a9.1", &mut ctx, &HashSet::new()).unwrap();
        translate("b9.2", &mut ctx, &HashSet::new()).unwrap();
        assert!(
            translate("min.mn.a9.b9", &mut ctx, &HashSet::new())
                .unwrap()
                .contains("a9.min(b9)")
        );
        assert!(
            translate("max.mx.a9.b9", &mut ctx, &HashSet::new())
                .unwrap()
                .contains("a9.max(b9)")
        );
        assert!(
            t_machine("now.ts").contains("UNIX_EPOCH")
        );
        assert!(t_machine("bell.").contains("\\u{7}"));
        // reserved words are parsed as commands first
        assert!(t("len.5").unwrap_err().contains("len expects"));
        assert!(t("now.5")
            .unwrap_err()
            .contains("now expects a variable name"));
    }

    #[test]
    fn input_command_declares_once() {
        let mut ctx = Ctx::new(false);
        let out = translate("in.x", &mut ctx, &HashSet::new()).unwrap();
        assert!(out.contains("let mut x: i64"));
        let out = translate("in.x", &mut ctx, &HashSet::new()).unwrap();
        assert!(out.starts_with("    x = "));
    }

    #[test]
    fn ask_prompts_without_newline_then_reads() {
        let mut ctx = Ctx::new(false);
        let out = translate(r##"ask."num: ".x"##, &mut ctx, &HashSet::new()).unwrap();
        assert!(out.starts_with(r#"    print!("num: ");"#));
        assert!(out.contains("let mut x: i64"));
        assert!(out.contains("read_line"));
    }

    #[test]
    fn ask_rejects_missing_variable_or_bad_string() {
        assert!(t(r##"ask."no var""##).is_err());
        assert!(t(r##"ask."oops"##).is_err());
        assert!(t("ask.5").is_err());
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        assert_eq!(t("# note").unwrap(), "");
        assert_eq!(t("").unwrap(), "");
        assert_eq!(t("   ").unwrap(), "");
    }

    #[test]
    fn inline_comments_are_stripped_but_not_inside_strings() {
        let mut ctx = Ctx::new(false);
        assert_eq!(
            translate("x.10 # ten", &mut ctx, &HashSet::new()).unwrap(),
            "    #[allow(unused_variables)]\n    let mut x: i64 = 10;"
        );
        // '#' inside quotes is content, not a comment
        assert_eq!(
            translate(r##"log."#1 winner""##, &mut ctx, &HashSet::new()).unwrap(),
            r##"    println!("#1 winner");"##
        );
    }

    #[test]
    fn lines_without_dot_are_errors() {
        assert!(t("hello world").unwrap_err().contains("expected '<verb>.<operand>'"));
    }

    #[test]
    fn unknown_command_is_reported() {
        assert!(t("5.x").unwrap_err().contains("unknown command '5'"));
        assert!(t("?.x").unwrap_err().contains("unknown command '?'"));
    }

    #[test]
    fn reserved_words_rejected_as_variables() {
        assert!(t("log.10").unwrap_err().contains("log expects"));
        assert!(t("w.x").unwrap_err().contains("expects milliseconds"));
        assert!(t("j.x").unwrap_err().contains("unknown label 'x'"));
        assert!(t("jeq.x").is_err());
    }

    #[test]
    fn missing_operand_is_error() {
        assert!(t("x.").unwrap_err().contains("missing operand"));
    }

    // ---------- conditional jumps ----------

    fn tj(line: &str) -> Result<String, String> {
        let mut ctx = Ctx::new(false);
        let labels: HashSet<String> = ["top".to_string(), "end".to_string()].into();
        translate(line, &mut ctx, &labels)
    }

    fn declared_ctx() -> Ctx {
        let mut ctx = Ctx::new(false);
        translate("i.1", &mut ctx, &HashSet::new()).unwrap();
        translate("k.2", &mut ctx, &HashSet::new()).unwrap();
        ctx
    }

    #[test]
    fn conditional_ops_map_to_rust_comparisons() {
        for (verb, op) in COND_OPS {
            let ctx = declared_ctx();
            let line = format!("{verb}.i.k.top");
            let out = translate(&line, &mut { ctx }, &["top".to_string()].into()).unwrap();
            assert!(out.contains(&format!("if i {op} k")), "{out}");
            assert!(out.contains("__pc = \"top\""), "{out}");
        }
    }

    #[test]
    fn conditional_accepts_literal_rhs() {
        let mut ctx = declared_ctx();
        let out = translate("jlt.i.10.end", &mut ctx, &["end".to_string()].into()).unwrap();
        assert!(out.contains("if i < 10"));
        assert!(out.contains("__pc = \"end\";"));
    }

    #[test]
    fn conditional_requires_declared_lhs() {
        assert!(tj("jeq.nope.1.top").unwrap_err().contains("unknown variable 'nope'"));
    }

    #[test]
    fn conditional_rejects_bad_shape() {
        assert!(tj("jeq.i.1").unwrap_err().contains("expects '<var>.<value>.<label>'"));
        let mut ctx = declared_ctx();
        assert!(
            translate("jeq.i.x y.top", &mut ctx, &["top".to_string()].into())
                .unwrap_err()
                .contains("bad comparison operand")
        );
    }

    #[test]
    fn jump_unknown_label_is_error() {
        assert!(tj("j.nowhere").unwrap_err().contains("unknown label 'nowhere'"));
    }

    // ---------- compile(): modes, labels, machine ----------

    #[test]
    fn compile_collects_diagnostics_with_line_numbers() {
        let src = "x.10\nz.\nlog.zz\n";
        let errs = compile(src).unwrap_err();
        assert_eq!(errs.len(), 2);
        assert!(errs[0].starts_with("line 2:"));
        assert!(errs[1].starts_with("line 3:"));
    }

    #[test]
    fn statements_split_on_semis_comments_and_newlines() {
        let stmts = split_statements("x.10; log.\"a;b\".x # trailing\ny.2");
        let texts: Vec<(usize, &str)> =
            stmts.iter().map(|(n, s)| (*n, s.as_str())).collect();
        // ';' inside the string survives, comment cuts the rest of its line
        assert_eq!(texts, [(1, "x.10"), (1, " log.\"a;b\".x "), (2, "y.2")]);
    }

    #[test]
    fn whole_program_on_one_line_compiles_to_machine() {
        let body =
            compile("i.3; :top; log.i; i.-1; jgt.i.0.top; q.").unwrap();
        assert!(body.contains("let mut __pc = \"@begin\";"));
        assert!(body.contains("\"top\" => {"));
    }

    #[test]
    fn semicolon_errors_keep_real_line_numbers() {
        let errs = compile("x.1\nz.; log.zz\n").unwrap_err();
        assert_eq!(errs.len(), 2);
        assert!(errs[0].starts_with("line 2:"));
        assert!(errs[1].starts_with("line 2:"));
    }

    #[test]
    fn compile_happy_path_generates_body() {
        let body = compile("x.3\nlog.\"n = \".x\n").unwrap();
        assert!(body.contains("let mut x: i64 = 3;"));
        assert!(body.contains("println!(\"n = {}\", x);"));
        assert!(!body.contains("__pc")); // linear mode stays flat
    }

    #[test]
    fn duplicate_labels_are_errors() {
        let errs = compile(":top\n:top\n").unwrap_err();
        assert!(errs[0].contains("duplicate label 'top'"));
    }

    #[test]
    fn bad_label_names_are_errors() {
        let errs = compile(":9lives\n").unwrap_err();
        assert!(errs[0].contains("bad label"));
    }

    #[test]
    fn machine_mode_wraps_body_in_state_loop() {
        let body = compile("i.3\n:top\nj.top\n").unwrap();
        assert!(body.contains("let mut __pc = \"@begin\";"));
        assert!(body.contains("'__run: loop {"));
        assert!(body.contains("match __pc {"));
        assert!(body.contains("\"top\" => {"));
    }

    #[test]
    fn machine_mode_hoists_declarations() {
        let src = "a.5\nb.a\nj.done\n:done\nlog.b\n";
        let body = compile(src).unwrap();
        assert_eq!(body.matches("let mut a: i64").count(), 1);
        assert!(body.contains("let mut b: i64 = 0;")); // hoisted with safe init
        assert!(body.contains("    b = a;") || body.contains("\n    b = a;"));
    }

    #[test]
    fn machine_mode_last_segment_exits_loop() {
        let body = compile(":only\nq.\n").unwrap();
        assert!(body.contains("break '__run;"));
    }

    #[test]
    fn machine_mode_q_breaks_instead_of_return() {
        let body = compile(":top\nq.\nj.top\n").unwrap();
        assert!(body.contains("break '__run;"));
        assert!(!body.contains("    return;"));
    }

    #[test]
    fn countdown_example_shapes_correctly() {
        let src = "i.3\n:top\nlog.\"t minus \".i\ni.-1\njgt.i.0.top\nlog.\"liftoff\"\nq.\n";
        let body = compile(src).unwrap();
        assert!(body.contains("if i > 0 {\n            __pc = \"top\";") || body.contains("if i > 0 {"));
        assert!(body.contains("\"top\" => {"));
        // i hoisted once, then decremented in place
        assert_eq!(body.matches("let mut i: i64").count(), 1);
        assert!(body.contains("i = i - 1;"));
    }

    #[test]
    fn jump_before_declaration_still_compiles() {
        // textual order lies in machine mode; hoisting makes this valid
        let body = compile("j.skip\nx.10\n:skip\nlog.x\n").unwrap();
        assert!(body.contains("let mut x: i64 = 10;"));
        assert!(body.contains("__pc = \"skip\";"));
    }
}

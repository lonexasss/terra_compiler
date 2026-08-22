use std::collections::HashSet;
use std::env;
use std::fs;
use std::process::{Command, exit};

const GENERATED_CARGO_TOML: &str =
    "[package]\nname = \"terra_output\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";

const RESERVED: [&str; 4] = ["log", "in", "w", "q"];

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

/// escape braces/quotes so println! doesn't eat them
fn escape_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('{', "{{")
        .replace('}', "}}")
}

// first mention -> let mut, later ones just assign
fn bind(name: &str, rhs: &str, declared: &mut HashSet<String>) -> String {
    if declared.insert(name.to_string()) {
        format!("    let mut {name}: i64 = {rhs};")
    } else {
        format!("    {name} = {rhs};")
    }
}

fn require_declared(name: &str, declared: &HashSet<String>) -> Result<(), String> {
    if declared.contains(name) {
        Ok(())
    } else {
        Err(format!("unknown variable '{name}'"))
    }
}

fn translate_in(name: &str, declared: &mut HashSet<String>) -> Result<String, String> {
    if !is_ident(name) {
        return Err(format!("in expects a variable name, got '{name}'"));
    }
    let reader = r#"{
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).expect("input error");
        buf.trim().parse().expect("invalid number")
    }"#;
    if declared.insert(name.to_string()) {
        Ok(format!("    let mut {name}: i64 = {reader};"))
    } else {
        Ok(format!("    {name} = {reader};"))
    }
}

fn translate_log(operand: &str, declared: &HashSet<String>) -> Result<String, String> {
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
        require_declared(var, declared)?;
        Ok(format!(r#"    println!("{text}{{}}", {var});"#))
    } else if is_ident(operand) {
        require_declared(operand, declared)?;
        Ok(format!(r#"    println!("{{}}", {operand});"#))
    } else {
        Err(format!(
            "log expects a quoted string or a variable, got '{operand}'"
        ))
    }
}

fn translate(
    raw: &str,
    declared: &mut HashSet<String>,
) -> Result<String, String> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(String::new());
    }

    let (verb, operand) = line
        .split_once('.')
        .ok_or_else(|| format!("expected '<verb>.<operand>', got '{line}'"))?;

    match verb {
        "log" => translate_log(operand, declared),
        "in" => translate_in(operand, declared),
        "w" => {
            if !is_uint(operand) {
                return Err(format!("w expects milliseconds, got '{operand}'"));
            }
            Ok(format!(
                "    std::thread::sleep(std::time::Duration::from_millis({operand}));"
            ))
        }
        "q" => {
            if !operand.is_empty() {
                return Err("q takes no operand".to_string());
            }
            Ok("    return;".to_string())
        }
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
                return Ok(bind(target, operand, declared));
            }
            if let Some(rest) = operand.strip_prefix(['+', '-', '*', '/']) {
                // x.+5, x.-2, ...
                if !is_uint(rest) {
                    return Err(format!(
                        "bad arithmetic operand '{operand}' (expected +N, -N, *N or /N)"
                    ));
                }
                require_declared(target, declared)?;
                let op = &operand[..1];
                return Ok(format!("    {target} = {target} {op} {rest};"));
            }
            if is_ident(operand) {
                require_declared(operand, declared)?;
                return Ok(bind(target, operand, declared));
            }
            Err(format!(
                "unknown operand '{operand}' (integer, +N/-N/*N//N or variable expected)"
            ))
        }
    }
}

// whole program -> rust body or "line N: ..." errors
fn compile(source: &str) -> Result<String, Vec<String>> {
    let mut declared: HashSet<String> = HashSet::new();
    let mut body = String::new();
    let mut errors = Vec::new();

    for (idx, raw) in source.lines().enumerate() {
        match translate(raw, &mut declared) {
            Ok(code) if !code.is_empty() => {
                body.push_str(&code);
                body.push('\n');
            }
            Ok(_) => {}
            Err(msg) => errors.push(format!("line {}: {msg}", idx + 1)),
        }
    }

    if errors.is_empty() {
        Ok(body)
    } else {
        Err(errors)
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("terra compiler");
        eprintln!("usage: terra_compiler <script.tr> [--emit-rust]");
        eprintln!("  --emit-rust   print generated rust instead of running it");
        exit(if args.len() < 2 { 1 } else { 0 });
    }

    let path = &args[1];
    let emit_only = args.iter().any(|a| a == "--emit-rust");

    let content = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("terra: cannot read '{path}': {e}");
        exit(1);
    });

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

    let rust_code = format!("fn main() {{\n{body}}}\n");

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
        translate(line, &mut HashSet::new())
    }

    #[test]
    fn assignment_then_reassignment() {
        let mut d = HashSet::new();
        assert_eq!(translate("x.10", &mut d).unwrap(), "    let mut x: i64 = 10;");
        assert_eq!(translate("x.20", &mut d).unwrap(), "    x = 20;");
    }

    #[test]
    fn arithmetic_ops() {
        let mut d = HashSet::new();
        translate("x.10", &mut d).unwrap();
        assert_eq!(translate("x.+5", &mut d).unwrap(), "    x = x + 5;");
        assert_eq!(translate("x.-2", &mut d).unwrap(), "    x = x - 2;");
        assert_eq!(translate("x.*3", &mut d).unwrap(), "    x = x * 3;");
        assert_eq!(translate("x./4", &mut d).unwrap(), "    x = x / 4;");
    }

    #[test]
    fn arithmetic_on_undeclared_is_error() {
        assert!(t("y.+5").unwrap_err().contains("unknown variable 'y'"));
    }

    #[test]
    fn bad_arithmetic_operand() {
        let mut d = HashSet::new();
        translate("x.1", &mut d).unwrap();
        assert!(t("x.+abc").unwrap_err().contains("bad arithmetic operand"));
    }

    #[test]
    fn copy_variable() {
        let mut d = HashSet::new();
        translate("a.7", &mut d).unwrap();
        assert_eq!(translate("b.a", &mut d).unwrap(), "    let mut b: i64 = a;");
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
        let mut d = HashSet::new();
        translate("n.1", &mut d).unwrap();
        assert_eq!(translate("log.n", &mut d).unwrap(), "    println!(\"{}\", n);");
    }

    #[test]
    fn log_unknown_variable_is_error() {
        assert!(t("log.nope").unwrap_err().contains("unknown variable 'nope'"));
    }

    #[test]
    fn log_text_plus_variable() {
        let mut d = HashSet::new();
        translate("n.1", &mut d).unwrap();
        assert_eq!(
            translate(r#"log."value = ".n"#, &mut d).unwrap(),
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
        assert!(t("q.42").unwrap_err().contains("takes no operand"));
    }

    #[test]
    fn input_command_declares_once() {
        let mut d = HashSet::new();
        let out = translate("in.x", &mut d).unwrap();
        assert!(out.contains("let mut x: i64"));
        let out = translate("in.x", &mut d).unwrap();
        assert!(out.starts_with("    x = "));
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        assert_eq!(t("# note").unwrap(), "");
        assert_eq!(t("").unwrap(), "");
        assert_eq!(t("   ").unwrap(), "");
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
    }

    #[test]
    fn missing_operand_is_error() {
        assert!(t("x.").unwrap_err().contains("missing operand"));
    }

    #[test]
    fn compile_collects_diagnostics_with_line_numbers() {
        let src = "x.10\nz.\nlog.zz\n";
        let errs = compile(src).unwrap_err();
        assert_eq!(errs.len(), 2);
        assert!(errs[0].starts_with("line 2:"));
        assert!(errs[1].starts_with("line 3:"));
    }

    #[test]
    fn compile_happy_path_generates_body() {
        let body = compile("x.3\nlog.\"n = \".x\n").unwrap();
        assert!(body.contains("let mut x: i64 = 3;"));
        assert!(body.contains("println!(\"n = {}\", x);"));
    }
}

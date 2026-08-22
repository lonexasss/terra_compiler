use std::env;
use std::fs;
use std::process::Command;

fn translate_line(line: &str) -> String {
    let line = line.trim();

    // Перевод log."текст" -> println!("текст");
    if line.starts_with("log.\"") && line.ends_with('"') {
        let text = &line[5..line.len() - 1];
        return format!("    println!(\"{}\");", text);
    }

    // Перевод log.x -> println!("{}", x);
    if line.starts_with("log.") {
        let var_name = &line[4..];
        return format!("    println!(\"{{}}\", {});", var_name);
    }

    // Перевод x.10 -> let x = 10;
    if line.contains('.') {
        let parts: Vec<&str> = line.split('.').collect();
        if parts.len() == 2 && parts[1].chars().all(|c| c.is_ascii_digit()) {
            return format!("    let {} = {};", parts[0], parts[1]);
        }
    }

    String::new()
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("Использование: terra_compiler <файл.terra>");
        return;
    }

    let file_path = &args[1];
    let content = fs::read_to_string(file_path).expect("Не удалось прочитать файл TERRA");

    let mut rust_code = String::from("fn main() {\n");
    for line in content.lines() {
        let translated = translate_line(line);
        if !translated.is_empty() {
            rust_code.push_str(&translated);
            rust_code.push('\n');
        }
    }
    rust_code.push_str("}\n");

    let _ = fs::create_dir_all("terra_build/src");
    fs::write("terra_build/Cargo.toml", "[package]\nname = \"terra_output\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n").unwrap();
    fs::write("terra_build/src/main.rs", rust_code).unwrap();

    Command::new("cargo")
        .args(["run", "--quiet", "--manifest-path", "terra_build/Cargo.toml"])
        .status()
        .expect("Ошибка запуска cargo");
}
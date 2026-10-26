use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut fix = false;
    let mut roots = Vec::new();
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--fix" => fix = true,
            _ => roots.push(PathBuf::from(arg)),
        }
    }

    if roots.is_empty() {
        roots.push(PathBuf::from("crates"));
    }

    match sych_style::run(&roots, fix) {
        Ok(violations) if violations.is_empty() => ExitCode::SUCCESS,
        Ok(violations) => {
            for v in &violations {
                eprintln!(
                    "{}:{}: blank-line-after-if: add an empty line after this `if` block",
                    v.path.display(),
                    v.line
                );
            }
            eprintln!(
                "sych-style: {} violations; `just style-fix` fixes them",
                violations.len()
            );
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("sych-style: {err}");
            ExitCode::from(2)
        }
    }
}

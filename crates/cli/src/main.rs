//! `brigadier`: see the library's docs.

use std::process::ExitCode;

fn main() -> ExitCode {
    if brigadier_sandbox::build_slots::is_wrapper() {
        brigadier_sandbox::build_slots::main();
    }
    let words: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    let code = brigadier_cli::run(
        words,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    ExitCode::from(code)
}

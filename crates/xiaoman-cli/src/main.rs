use std::process::ExitCode;

fn main() -> ExitCode {
    let code = xiaoman_cli::cli::main_with_args(std::env::args_os().skip(1));
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

//! Internal single-use script sandbox. Not an operator command.
fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let option = args.next();
    if option.as_deref() == Some("--help") {
        println!(
            "Internal Melodee script worker: --memory-limit-bytes <bytes>; bounded JSON on stdin."
        );
        return std::process::ExitCode::SUCCESS;
    }
    let limit = args.next().and_then(|value| value.parse::<u64>().ok());
    if option.as_deref() != Some("--memory-limit-bytes") || args.next().is_some() {
        return std::process::ExitCode::FAILURE;
    }
    match limit.and_then(|limit| melodee_application::scripting::run_worker(limit).ok()) {
        Some(()) => std::process::ExitCode::SUCCESS,
        None => std::process::ExitCode::FAILURE,
    }
}

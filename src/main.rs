fn main() {
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let code = buffetcar::run_with_io(std::env::args_os(), &mut stdout, &mut stderr);
    std::process::exit(code);
}

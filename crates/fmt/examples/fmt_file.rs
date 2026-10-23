fn main() {
    let path = std::env::args().nth(1).expect("path");
    let src = std::fs::read_to_string(&path).unwrap();
    match zapc_fmt::format_source(&src) {
        Ok(out) => print!("{out}"),
        Err(e) => eprintln!("{e}"),
    }
}

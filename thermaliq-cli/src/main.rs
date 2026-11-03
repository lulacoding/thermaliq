//! `tq` — command-line client for the daemon. Built in module M14 (PORTING.md).

fn main() {
    eprintln!(
        "tq {} — v2 skeleton, no commands yet (see PORTING.md)",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(1);
}

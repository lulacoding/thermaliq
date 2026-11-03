//! Daemon wiring: paths, config, history, API, service, agent. See SPEC.md §5.

fn main() {
    eprintln!(
        "thermaliq-daemon {} — v2 skeleton, does not control fans yet (see PORTING.md)",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(1);
}

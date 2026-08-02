// Sets cfg(fork_starship) when the resolved starship crate is the personal
// git fork (which adds the cache API to starship::print) rather than the
// crates.io release. The fork's extras are cache-only, so a plain crates.io
// build must compile with them out - no --no-default-features required.
fn main() {
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rustc-check-cfg=cfg(fork_starship)");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let lock = std::fs::read_to_string(std::path::Path::new(&manifest).join("Cargo.lock"));
    let lock = match lock {
        Ok(l) => l,
        Err(_) => return,
    };
    if is_fork_starship(&lock) {
        println!("cargo:rustc-cfg=fork_starship");
    }
}

fn is_fork_starship(lock: &str) -> bool {
    let mut in_starship = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_starship = false;
        } else if line.starts_with("name = ") && line.contains("\"starship\"") {
            in_starship = true;
        } else if in_starship && line.starts_with("source = ") {
            return line.contains("git+https://github.com/zdonghe/starship");
        }
    }
    false
}

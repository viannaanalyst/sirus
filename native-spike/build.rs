//! Links Ghostty's libghostty-vt when the `ghostty` feature is on (ADR-199). The static library is
//! built by `scripts/build-libghostty-vt.sh` at the revision in vendor/libghostty-vt/VERSION.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_GHOSTTY");
    if std::env::var_os("CARGO_FEATURE_GHOSTTY").is_none() {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/libghostty-vt/lib");
    let lib = dir.join("libghostty-vt.a");
    println!("cargo:rerun-if-changed={}", lib.display());
    if !lib.exists() {
        panic!(
            "the `ghostty` feature needs {}; run scripts/build-libghostty-vt.sh first",
            lib.display()
        );
    }
    println!("cargo:rustc-link-search=native={}", dir.display());
}

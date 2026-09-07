//! Forwards the rustc target triple into the crate as `AIFS_TARGET_TRIPLE`.

fn main() {
    println!("cargo:rerun-if-env-changed=TARGET");
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=AIFS_TARGET_TRIPLE={target}");
    }
}

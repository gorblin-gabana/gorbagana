fn main() {
    // Tell cargo about our custom cfg flags
    println!("cargo:rustc-check-cfg=cfg(target_os, values(\"solana\"))");
} 
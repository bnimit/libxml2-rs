fn main() {
    // Re-run if any source file changes
    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    // Generate C headers from pub extern "C" items
    // Uncomment once there are symbols to export:
    //
    // let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // cbindgen::Builder::new()
    //     .with_crate(&crate_dir)
    //     .with_config(cbindgen::Config::from_file("cbindgen.toml").unwrap())
    //     .generate()
    //     .expect("cbindgen failed")
    //     .write_to_file("include/libxml/parser.h");
}

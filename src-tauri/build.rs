fn main() {
    println!("cargo:rerun-if-changed=stub/kryoto-setup.exe");
    if !std::path::Path::new("stub/kryoto-setup.exe").exists() {
        panic!("The setup program is missing. Run `pnpm stub` first.");
    }
    tauri_build::build()
}

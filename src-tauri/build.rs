fn main() {
    println!("cargo:rerun-if-changed=../templates");
    #[cfg(feature = "app")]
    tauri_build::build()
}

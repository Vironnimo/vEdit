fn main() {
    println!("cargo:rerun-if-changed=assets/vedit.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/vedit.ico")
            .set("ProductName", "vEdit")
            .set("FileDescription", "vEdit text and code editor")
            .set("InternalName", "vEdit")
            .compile()
            .expect("compile vEdit Windows icon and version resources");
    }
}

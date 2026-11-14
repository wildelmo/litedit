//! Embeds the app icon and version information into litedit.exe.
//! Only runs when building on Windows for Windows (needs rc.exe).

fn main() {
    println!("cargo:rerun-if-changed=assets/litedit.ico");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/litedit.ico");
            res.set(
                "FileDescription",
                "litedit - ultralight markdown-first text editor",
            );
            res.set("ProductName", "litedit");
            res.set("LegalCopyright", "MIT License");
            if let Err(e) = res.compile() {
                panic!("could not embed the icon and version info: {e}");
            }
        }
    }
}

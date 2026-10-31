//! Embeds the application icon and version info into the Windows executables.
//!
//! Best-effort: if no resource compiler is available (`rc.exe` from the
//! Windows SDK for MSVC, `windres` for MinGW) the build continues without an
//! icon instead of failing.

fn main() {
    println!("cargo:rerun-if-changed=assets/freewipe.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/freewipe.ico")
        .set("ProductName", "freewipe")
        .set("FileDescription", "freewipe — secure data wiping")
        .set("LegalCopyright", "MIT OR Apache-2.0");

    // When cross-compiling with MinGW, use the matching windres/ar.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
        && std::env::var("HOST").map(|h| !h.contains("windows")).unwrap_or(false)
    {
        res.set_windres_path("x86_64-w64-mingw32-windres");
        res.set_ar_path("x86_64-w64-mingw32-ar");
    }

    if let Err(e) = res.compile() {
        println!("cargo:warning=icon resource not embedded: {e}");
    }
}

//! Gives the Windows executable its program icon (Explorer, shortcuts, taskbar).

fn main() {
    println!("cargo:rerun-if-changed=assets/app_icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new().set_icon("assets/app_icon.ico").compile().expect("icon resource compiles");
    }
}

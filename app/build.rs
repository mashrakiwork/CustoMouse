// Embeds the app icon as a Win32 resource, so Explorer, Task Manager, the
// Start menu, and the Settings > Startup Apps list all show it for the exe
// itself -- not just for the window CustoMouse draws once it's running.
#[cfg(windows)]
fn main() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed app icon resource: {e}");
    }
}

#[cfg(not(windows))]
fn main() {}

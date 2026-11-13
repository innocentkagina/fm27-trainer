// Windows: icoon en versie-info in het .exe-bestand. Op andere platforms niets.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/app.ico");
        res.set("ProductName", "FM27 Coach");
        res.set("FileDescription", "FM27 Coach");
        if let Err(e) = res.compile() {
            println!("cargo:warning=Windows-icoon niet ingebouwd: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/icon/app.ico");
}

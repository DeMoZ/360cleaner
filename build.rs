// Значок и описание .exe в Windows.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "360Cleaner");
        res.set("ProductName", "360Cleaner");
        if let Err(e) = res.compile() {
            println!("cargo:warning=значок не добавлен: {e}");
        }
    }
}

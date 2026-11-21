// Значок и описание .exe в Windows; на Mac — среда выполнения clang для ONNX Runtime.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    // Часть ONNX Runtime для CoreML проверяет версию системы (`@available` → `__isPlatformVersionAtLeast`). Эта
    // функция — в libclang_rt.osx.a; обычная сборка её находит, а тесты в --release без неё не линкуются.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
        && let Ok(out) = std::process::Command::new("clang").arg("--print-runtime-dir").output()
        && out.status.success()
    {
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if std::path::Path::new(&dir).join("libclang_rt.osx.a").is_file() {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
    }
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

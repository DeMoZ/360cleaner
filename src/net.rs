//! Сеть через системный `curl` (есть в macOS и в Windows 10+): без своей реализации TLS в программе.
//! Нужна только для донатов и обновлений; обработка фото работает без сети.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Команда без окна консоли в Windows.
pub fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn curl() -> Command {
    let mut c = command(if cfg!(windows) { "curl.exe" } else { "/usr/bin/curl" });
    // GitHub API без User-Agent отвечает отказом.
    c.args(["-fsSL", "--proto", "=https", "-A", "360Cleaner"]).stdin(Stdio::null());
    c
}

/// Скачивает небольшой ответ (JSON) целиком.
pub fn get(url: &str, timeout_secs: u32) -> Result<Vec<u8>, String> {
    let out = curl()
        .args(["--max-time", &timeout_secs.to_string(), url])
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("curl exited with code {:?}", out.status.code()) } else { err })
    }
}

/// Скачивает файл на диск.
pub fn download(url: &str, to: &Path) -> Result<(), String> {
    let out = curl()
        .args(["--max-time", "900", "-o"])
        .arg(to)
        .arg(url)
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Открывает страницу в браузере. Только https://.
pub fn open_url(url: &str) -> std::io::Result<()> {
    if !is_https(url) {
        return Err(std::io::Error::other("only https:// links are allowed"));
    }
    if cfg!(windows) {
        command("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).spawn().map(|_| ())
    } else if cfg!(target_os = "macos") {
        command("/usr/bin/open").arg(url).spawn().map(|_| ())
    } else {
        command("xdg-open").arg(url).spawn().map(|_| ())
    }
}

pub fn is_https(url: &str) -> bool {
    url.len() > "https://".len()
        && url[..8].eq_ignore_ascii_case("https://")
        && !url[8..].starts_with('/')
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Показывает файл в Finder / Проводнике.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    if cfg!(windows) {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        command("explorer.exe").arg(arg).spawn().map(|_| ())
    } else if cfg!(target_os = "macos") {
        command("/usr/bin/open").arg("-R").arg(path).spawn().map(|_| ())
    } else {
        command("xdg-open").arg(path.parent().unwrap_or(path)).spawn().map(|_| ())
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

fn base_dir(var_win: &str, mac: &str, linux: &str) -> PathBuf {
    let home = home_dir();
    let dir = if cfg!(windows) {
        std::env::var_os(var_win).map(PathBuf::from).unwrap_or_else(|| home.join("AppData"))
    } else if cfg!(target_os = "macos") {
        home.join(mac)
    } else {
        home.join(linux)
    };
    dir.join("360Cleaner")
}

/// Настройки: `~/Library/Application Support/360Cleaner`, `%APPDATA%\360Cleaner`.
pub fn config_dir() -> PathBuf {
    base_dir("APPDATA", "Library/Application Support", ".config")
}

/// Кэш: `~/Library/Caches/360Cleaner`, `%LOCALAPPDATA%\360Cleaner`.
pub fn cache_dir() -> PathBuf {
    base_dir("LOCALAPPDATA", "Library/Caches", ".cache")
}

#[cfg(test)]
mod tests {
    use super::is_https;

    #[test]
    fn only_https_links() {
        assert!(is_https("https://pay.example/a"));
        assert!(is_https("HTTPS://pay.example/a"));
        assert!(!is_https("http://pay.example/a"));
        assert!(!is_https("https:///etc/passwd"));
        assert!(!is_https("javascript:alert(1)"));
        assert!(!is_https("https://a b"));
        assert!(!is_https("https://"));
    }
}

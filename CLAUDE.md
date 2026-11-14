# 360Cleaner

Убирает фотографа и людей с 360° панорамы (equirectangular 2:1) по двум кадрам с одной точки. Без нейросетей.
Отвечай пользователю по-русски. Описание для пользователя — [README.md](README.md).

## Устройство

Rust (edition 2024) + tao 0.37 / wry 0.57 (системный WebView: WKWebView на Mac, WebView2 в Windows) + Rayon.
Интерфейс — один файл `src/ui/index.html` (HTML/CSS/JS, встраивается через `include_str!`).

- `main.rs` — окно, обмен с WebView, состояние (`State` под `Mutex`), загрузка фото, рабочий поток расчёта,
  сохранение, донаты и обновления.
- `processor.rs` — обработка: разница ΔE в Lab → порог → открытие (убирает точки шума) → расширение кругом →
  два угловых кластера на окружности → мягкая маска `Alpha` → смешивание.
- `xmp.rs` — перенос APP1 (EXIF, XMP GPano) и APP2 (ICC) из первого фото в результат (после JFIF).
- `update.rs`, `donations.rs`, `net.rs`, `settings.rs` — как в AdbToolsGUI (см. ниже).

## Важно (на этом уже обожглись)

- **WebView живёт только в главном потоке.** Из рабочих потоков в окно — только через `Ui::send` →
  `EventLoopProxy` → `UserEvent::Js` → `evaluate_script` в цикле событий. Первая версия держала WebView в
  `thread_local` и звала его из рабочих потоков: там он пустой, и ни одно сообщение до окна не доходило.
- **Диалоги `rfd` — только в главном потоке** (на Mac иначе нельзя): IPC шлёт `UserEvent::PickFile` / `Save`.
- **Расчёт превью — один рабочий поток**: из очереди берётся последний запрос, результат сверяется с
  `State::generation` и выбрасывается, если фото или параметры успели поменяться.
- **Экспорт не пересчитывает маску в полном размере**, а растягивает `Alpha` превью (`blend_into`): файл совпадает
  с превью, и это быстро. Прежняя дилатация перебором на 8K с радиусом ×6 работала бы часами.
- Расширение маски — `dilate_mask_360` за O(w·h·r) через расстояния по строкам; тест сверяет с перебором.
- Удалённые Python-версии (PySide6 / OpenCV) лежат в истории git до коммита cd6040a, не возвращай их.

## Донаты и обновления

- Донаты: файл `360cleaner.json` из `DeMoZ/donations` (raw.githubusercontent → jsDelivr → кэш → копия
  `assets/donations.json`), формат schema 1 как в AdbToolsGUI / EnhanceCapCut / UniversalKM. Ссылки меняются в
  репозитории donations; копию в `assets/` держать такой же.
- Обновления (`update.rs`): GitHub API `releases/latest` репозитория `DeMoZ/360cleaner`, архив
  `360Cleaner-<v>-macos-arm64.zip` (`360Cleaner.app`, упакован `ditto`) или `360Cleaner-<v>-windows-x64.zip`
  (`360Cleaner.exe`), проверка SHA-256 из поля `digest`, замена скриптом после выхода, перезапуск; при неудаче
  старая копия стартует с `--update-failed`. В сборке `cargo run` (debug) обновление выключено.
  Адрес API можно подменить переменной `CLEANER360_UPDATES_URL`.
- Сеть — системный `curl`, ссылки открываются только `https://`.

## Проверки и релиз

- `cargo test` (медленный замер: `cargo test --release -- --ignored --nocapture timings`),
  `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — CI требует всё это.
- Windows с Mac: `cargo check --target x86_64-pc-windows-msvc`. На настоящей Windows программа не проверялась.
- Сборка Mac: `scripts/build-macos.sh` (только Apple Silicon). Значок: `scripts/make-icon.sh` → `assets/`.
- Релиз: `version` в `Cargo.toml` + `docs/releases/v<версия>.md` + тег `v<версия>` на main (CI сверяет тег с
  версией и выкладывает .dmg и оба .zip).
- Тексты интерфейса на русском.

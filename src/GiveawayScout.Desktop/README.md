# Giveaway Scout Desktop

Приложение Giveaway Scout на Tauri, Rust и React/TypeScript.

Основная платформа — **Windows x64**. Основная документация — [README в корне](../../README.md).

Текущее приложение использует Rust-ядро; импорт данных прежней WPF-версии сохранён. Доступны:

- каркас Tauri 2;
- React-интерфейс на русском и английском языках;
- первая типизированная команда React → Rust;
- строгая CSP и минимальные capabilities;
- SQLite-схема и одноразовый транзакционный импорт JSON из WPF-версии;
- фильтрация ложных записей Games for Farm `/user#settings` при импорте;
- Rust-парсеры Givee Club, Games for Farm и Zeros со стабильными ключами;
- отдельное включение Zeros (HTTP/WS) с предупреждением об одном подключении на IP; только публичный список Steam-раздач, без входа или получения ключей;
- HTTP-проверка с независимой обработкой ошибок источников, настраиваемыми тайм-аутом и повторными попытками;
- немедленная и периодическая проверка, команда «Проверить сейчас» и статус последней успешной проверки;
- история уведомлений из SQLite, отметка одной или всех записей прочитанными и безопасное открытие раздачи в браузере;
- локализованные системные уведомления с настраиваемым звуком, тестовой кнопкой, повтором неудачной доставки и открытием соответствующей раздачи по UUID;
- привлечение внимания к окну при непрочитанных уведомлениях и протокол `giveaway-scout://` для активации установленного приложения из Windows toast;
- отдельные настройки источников, интервала, сетевых запросов и языка с сохранением в SQLite и применением без перезапуска;
- отображение размера SQLite-данных и очистка старых раздач вместе с историей без удаления настроек;
- системный трей с открытием окна, немедленной проверкой и полным выходом;
- настраиваемые автозапуск и работа в трее после закрытия окна;
- защита от второго экземпляра с активацией уже работающего окна;
- собственный значок Giveaway Scout вместо стандартного значка Tauri;
- файловый журнал с ограничением размера и ротацией;
- frontend- и Rust-тесты основы приложения, миграции и мониторинга.

## Разработка

Требуются Node.js 24 LTS, pnpm 12 и Rust 1.99.1. Для Linux также нужны системные зависимости Tauri, включая WebKitGTK.

```bash
pnpm install
pnpm test
pnpm build
cd src-tauri
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cd ..
pnpm tauri dev
```

В Windows те же проверки запускаются из PowerShell, а NSIS-установщик собирается командой:

```powershell
.\scripts\build-windows.ps1
```

Скрипт запускается из корня репозитория, принудительно восстанавливает платформенные Windows-ссылки зависимостей frontend, выполняет Rust- и frontend-тесты, production-сборку интерфейса и создаёт NSIS Setup. Для повторной сборки без тестов можно передать `-SkipTests`. Windows-артефакты помещаются в отдельный `src-tauri/target-windows-x64`, чтобы сборки Windows и WSL/Linux не перезаписывали друг друга.

База создаётся в системном локальном каталоге данных приложения. В Windows при первом запуске новая версия читает `settings.json`, `tracking-state.json` и `notification-history.json` из `%LocalAppData%\GiveawayScout`. Импорт не удаляет и не изменяет исходные JSON-файлы.

Полная Linux-сборка Tauri дополнительно требует `pkg-config`, GTK 3, WebKitGTK 4.1, Ayatana AppIndicator и librsvg. Без них независимое Rust-ядро и frontend всё равно можно разрабатывать и тестировать отдельно.

Для Ubuntu/Debian зависимости устанавливаются командой:

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev pkg-config
```

После сборки через `pnpm tauri` на Linux x64 пакеты находятся в `src-tauri/target-linux-x64/release/bundle/deb` и `src-tauri/target-linux-x64/release/bundle/appimage` (на ARM64 — `target-linux-arm64`). В Linux база хранится в `~/.local/share/com.giveawayscout.desktop/giveaway-scout.db`, а журнал — в соседнем каталоге `logs`.

Экспериментальная Linux-упаковка выполняется на Linux из этого каталога: `pnpm tauri build --bundles deb,appimage`. Это не готовая поддерживаемая Linux-версия и не сборка Linux-пакетов из Windows PowerShell.

Windows-установщик находится в `src-tauri/target-windows-x64/release/bundle/nsis`. Данные Windows-версии хранятся в `%LocalAppData%\com.giveawayscout.desktop`: SQLite-база — `giveaway-scout.db`, журнал — `logs\Giveaway Scout.log`.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

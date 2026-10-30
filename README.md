# Giveaway Scout

Приложение для отслеживания раздач игр и розыгрышей на Givee Club, Games for Farm и Zeros. Сообщает о новых раздачах с помощью системных уведомлений.

[Скачать для Windows](https://github.com/gogolevmatvey/giveaway-scout/releases/latest)

## Возможности

- Уведомления о новых раздачах игр и розыгрышах с переходом на страницу раздачи.
- История уведомлений и отметки прочитанности.
- Фильтры Steam/Epic Games, выбор источников и интервала проверки.
- Русский и английский интерфейс.
- Трей, автозапуск и запуск свёрнутым.
- Автообновления.

## Источник Zeros

[Zeros](http://zeros.group/free/) предоставляет публичный список Steam-раздач через `ws://zeros.group:8863/`. Источник включается отдельно и по умолчанию выключен.

Соединение HTTP/WebSocket не шифруется. Сайт допускает одно подключение с внешнего IP: открытая вкладка или другое устройство той же сети могут мешать проверке. Программа кратковременно получает публичную конфигурацию и закрывает соединение.

## Статистика использования

Приложение автоматически отправляет статистику использования, чтобы оценивать количество активных пользователей. Передаются только случайный идентификатор установки и версия приложения.

## Разработка в Windows

Требуются Windows-версии Node.js 24.21.0, pnpm 12.10.1, Rust 1.99.1, Visual Studio Build Tools с C++/Windows SDK и Microsoft Edge WebView2 Runtime.

Из корня проекта:

```powershell
cd src\GiveawayScout.Desktop
pnpm install --frozen-lockfile
pnpm tauri dev
```

Интерфейс обновляется при сохранении файлов; Rust-часть пересобирается автоматически. Dev- и Release-профили данных разделены.

## Проверки и сборка

Из каталога `src/GiveawayScout.Desktop`:

```powershell
pnpm test
pnpm build
pnpm test:ui:browser
pnpm test:ui:native
```

Нативные UI-тесты используют отдельный профиль и контролируемые данные. Они не заменяют ручную проверку настоящих системных уведомлений, автозапуска и установки обновлений.

Проверки Rust-ядра и инструментов выпуска:

```powershell
cd src-tauri
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Внутренняя Windows x64-сборка из корня проекта:

```powershell
.\scripts\build-windows.ps1 -BuildLabel next
```

Portable EXE и NSIS-установщик появятся в `src/GiveawayScout.Desktop/src-tauri/target-windows-x64-next/release/`. Обычная внутренняя сборка не подключает канал обновлений.

## Лицензия

[MIT](LICENSE).

# Giveaway Scout Desktop

Исходники графического приложения на Tauri 2 и React/TypeScript и отдельного CLI на Rust. Оба приложения используют общие пакеты мониторинга, данных, системных операций и обновления. Описание возможностей и ссылка на скачивание находятся в [основном README](../../README.md).

## Каталоги

| Путь | Назначение |
| --- | --- |
| `src/` | Интерфейс, переводы и frontend-тесты |
| `src-tauri/src/` | Команды приложения, системные уведомления, трей и обновления |
| `src-tauri/crates/` | CLI, общее ядро, runtime, системные операции, обновление и инструменты выпуска |
| `src-tauri/installer/` | Ресурсы и тексты Windows-установщика |
| `scripts/` | Запуск инструментов разработки и нативных UI-тестов |
| `tests/ui/` | Автоматические проверки интерфейса |

## Запуск в разработке

Требования к окружению Windows перечислены в [основном README](../../README.md#разработка-в-windows). Из этого каталога:

```powershell
pnpm install --frozen-lockfile
pnpm tauri dev
```

## Проверки

Из этого каталога:

```powershell
pnpm test
pnpm build
pnpm test:ui:browser
cargo test --manifest-path src-tauri/Cargo.toml --workspace --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets --locked -- -D warnings
```

Нативные UI-тесты Windows запускаются командой `pnpm test:ui:native`. Для них также нужен .NET SDK, указанный в [global.json](../../global.json).

## Сборка для Windows

Из корня репозитория:

```powershell
.\scripts\build-windows.ps1 -BuildLabel next
```

Готовые EXE графического и консольного приложений и каталог `bundle/nsis` с установщиком находятся в `src/GiveawayScout.Desktop/src-tauri/target-windows-x64-next/release/`. Эта внутренняя сборка не подключает канал автообновлений. Публичный релиз распространяется через общий установщик с выбором GUI, CLI или обоих компонентов.

Для отдельной сборки CLI из этого каталога:

```powershell
cargo build --manifest-path src-tauri/Cargo.toml -p giveaway-scout-cli --release --locked
```

Эта команда не требует Node.js, frontend-сборки, Tauri или WebView2. Готовый `giveaway-scout-cli.exe` находится в `src-tauri/target/release/`, если переменная `CARGO_TARGET_DIR` не задаёт другой каталог. Отдельно собранный EXE может проверять обновления, но для установки обновления установленной копии используется её CLI.

Параметры сборок и проверок остальных платформ описаны в [конфигурации GitHub Actions](../../.github/workflows/desktop-candidate.yml).

## Лицензия

[MIT](../../LICENSE).

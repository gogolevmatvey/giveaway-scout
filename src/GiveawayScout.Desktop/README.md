# Giveaway Scout Desktop

Исходники приложения на Tauri 2, Rust и React/TypeScript. Описание возможностей и ссылка на скачивание находятся в [основном README](../../README.md).

## Каталоги

| Путь | Назначение |
| --- | --- |
| `src/` | Интерфейс, переводы и frontend-тесты |
| `src-tauri/src/` | Команды приложения, системные уведомления, трей и обновления |
| `src-tauri/crates/` | Мониторинг источников, хранение данных, уведомления и инструменты выпуска |
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

Готовый EXE и каталог `bundle/nsis` с установщиком находятся в `src/GiveawayScout.Desktop/src-tauri/target-windows-x64-next/release/`. Эта внутренняя сборка не подключает канал автообновлений.

Параметры сборок и проверок остальных платформ описаны в [конфигурации GitHub Actions](../../.github/workflows/desktop-candidate.yml).

## Лицензия

[MIT](../../LICENSE).

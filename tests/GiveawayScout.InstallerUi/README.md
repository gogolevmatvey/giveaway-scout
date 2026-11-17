# Windows installer component acceptance

This independent .NET harness inspects the actual NSIS wizard before installation. It starts its own setup process with an isolated `/D=` destination and workspace-local temporary files, navigates to the component page, checks it, and cancels. It never invokes the Install button or closes an existing Giveaway Scout process.

Run from the repository root with Windows PowerShell:

```powershell
scripts/check-installer-ui.ps1 -Setup "artifacts/setup-language-test.exe" -Language ru -OutputDirectory artifacts/installer-ui/run-ru
scripts/check-installer-ui.ps1 -Setup "artifacts/setup-language-test.exe" -Language en -OutputDirectory artifacts/installer-ui/run-en
scripts/check-installer-ui.ps1 -MeasureOnly -OutputDirectory artifacts/installer-ui/measurements
```

Each output directory must be new or empty. Setup and outputs must remain inside the workspace; output directories belong under `artifacts/`. The harness briefly moves the cursor over its own setup window, then restores the cursor and foreground window. Run it separately from other UI automation.

The production setup uses the Windows language. To exercise both languages without changing Windows settings, build a separate test-only setup with NSIS `displayLanguageSelector: true` and a test hook that includes the production hook and defines `MUI_LANGDLL_ALWAYSSHOW`. Keep this setup in `artifacts/`. The standard language dialog reads the saved installer language; saving it occurs on the installation page, which this harness never enters.

Acceptance checks cover full localized component labels, Russian guillemets, actual tree bounds, absence of a horizontal scrollbar, both hover descriptions, restoration of the placeholder, rendered text contrast, and selection persistence after Back/Next. The report includes actual window DPI, control texts, setup hash, GDI text measurements, and verification that installation registration and user environment values did not change. BMP images capture only the owned setup window, including the placeholder and each hover state.

GDI measurements at 96, 144, and 192 DPI are additional text/layout evidence. They do not represent real window tests at 100%, 150%, or 200%. The harness does not modify Windows scale or language settings. Real window acceptance reports only `GetDpiForWindow` for the display on which the setup actually ran.

On success, the output contains `acceptance.json` and `components.bmp`. A failed run attempts to write `failure.json` and `failure.bmp`. Measurement-only runs produce `measurements.json` and do not start setup.

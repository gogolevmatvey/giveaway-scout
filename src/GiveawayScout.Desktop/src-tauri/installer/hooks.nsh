; Override welcome and directory copy; keep the upstream installation workflow.
; Numeric language IDs work here because Tauri includes hooks before MUI_LANGUAGE.
LangString ScoutWelcomeText 1033 "Setup will install $(^NameDA) on your computer.$\r$\n$\r$\nBefore updating, quit Giveaway Scout using Quit in the tray menu. Other applications can remain open.$\r$\n$\r$\nClick Next to continue."
LangString ScoutWelcomeText 1049 "Эта программа установит $(^NameDA) на ваш компьютер.$\r$\n$\r$\nПеред обновлением завершите работу Giveaway Scout: выберите «Выйти» в меню значка в трее. Остальные приложения можно оставить открытыми.$\r$\n$\r$\nНажмите кнопку «Далее» для продолжения."
!define MUI_WELCOMEPAGE_TEXT "$(ScoutWelcomeText)"
LangString ScoutDirectoryText 1033 "Setup will install $(^NameDA) in the following folder. To install in a different folder, click Browse and select another folder. Click Next to continue."
LangString ScoutDirectoryText 1049 "Программа установит $(^NameDA) в указанную папку. Чтобы установить приложение в другую папку, нажмите кнопку «Обзор» и укажите её. Нажмите кнопку «Далее» для продолжения."
!define MUI_DIRECTORYPAGE_TEXT_TOP "$(ScoutDirectoryText)"

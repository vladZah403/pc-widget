# PC Monitor — виджет для рабочего стола Windows

Показывает загрузку CPU (с графиком за минуту), RAM, место и активность диска,
скорость сети, батарею и температуру CPU/GPU.

## Запуск
1. Один раз: `python -m pip install psutil`
2. Двойной клик по `start.bat` (или по `widget.pyw`).

## Управление
- Перетаскивание — левой кнопкой мыши за любое место.
- Правый клик — меню: поверх всех окон, прозрачность, сброс позиции, закрыть.
- Позиция и настройки сохраняются в `widget_config.json`.

## Температура
Windows не отдаёт настоящие температуры обычным программам, поэтому виджет
берёт их из LibreHardwareMonitor (https://github.com/LibreHardwareMonitor/LibreHardwareMonitor):
1. Скачай и запусти `LibreHardwareMonitor.exe` от имени администратора.
2. Включи `Options → Remote Web Server → Run` (порт 8085).
3. По желанию: `Options → Start Minimized`, `Minimize To Tray`, `Run On Windows Startup`.

Виджет сам подхватит данные в течение 3 секунд.

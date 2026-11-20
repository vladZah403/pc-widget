@echo off
rem Starts the PC Monitor widget without a console window
cd /d "%~dp0"
start "" pythonw widget.pyw

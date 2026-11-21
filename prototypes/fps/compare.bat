@echo off
rem Stage 0 check: FPS of PC Monitor 2.0 (Rust prototype) against 1.8 on the
rem same frames. Start a game, then run this; play for a minute.
rem For OpenGL / Vulkan / DirectX 9 games: right click - Run as administrator.
cd /d "%~dp0"
python compare_fps.py --seconds 60
pause

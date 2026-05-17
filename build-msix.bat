@echo off
mkdir dist 2>nul
"C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\makeappx.exe" pack /d "%~dp0msix_layout" /p "%~dp0dist\HeadsetBatteryWidget_1.7.1.0.msix" /o /v
if %errorlevel% neq 0 (
    echo FAILED
    pause
) else (
    echo SUCCESS: dist\HeadsetBatteryWidget_1.7.1.0.msix
)

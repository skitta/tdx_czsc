@echo off
rem Deploy: build (Rust service + bridge DLL) and copy artifacts to TDX T0002\dlls\
rem Usage : deploy_to_tdx.bat [TDX dir]   (default D:\new_tdx64)
set "DEST=%~1"
if "%DEST%"=="" set "DEST=D:\new_tdx64"
if not exist "%DEST%\T0002\dlls\" echo [ERR] not found: %DEST% & exit /b 1

rem 1) build Rust service (release)
cargo build --release --manifest-path "%~dp0rust_service\Cargo.toml"
if errorlevel 1 goto :fail

rem 2) build bridge DLL
cmake -S "%~dp0bridge_dll" -B "%~dp0bridge_dll\build" -A x64
if errorlevel 1 goto :fail
cmake --build "%~dp0bridge_dll\build" --config Release
if errorlevel 1 goto :fail

rem 3) copy artifacts
copy /Y "%~dp0rust_service\target\release\tdx_czsc_service.exe"  "%DEST%\T0002\dlls\" >nul
copy /Y "%~dp0bridge_dll\build\Release\tdx_czsc_bridge.dll"      "%DEST%\T0002\dlls\" >nul
copy /Y "%~dp0bridge_dll\build\Release\libprotobuf.dll"          "%DEST%\T0002\dlls\" >nul
copy /Y "%~dp0bridge_dll\build\Release\abseil_dll.dll"           "%DEST%\T0002\dlls\" >nul
if errorlevel 1 echo [ERR] copy failed (TDX running?) & exit /b 1
echo Deployed to %DEST%\T0002\dlls
pause
exit /b 0

:fail
echo [ERR] build failed
pause
exit /b 1
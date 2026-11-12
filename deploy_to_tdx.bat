@echo off
setlocal
rem Deploy: build (Rust service + bridge DLL) and copy artifacts to TDX T0002\dlls\
rem Usage : deploy_to_tdx.bat [TDX dir]
rem 本机路径写在 .build\windows.env（KEY=VALUE，已 gitignore）；命令行参数优先。

rem 读取本机配置；已存在的环境变量优先于文件
if exist "%~dp0.build\windows.env" for /f "usebackq eol=# tokens=1,* delims==" %%A in ("%~dp0.build\windows.env") do if not "%%A"=="" if not defined %%A set "%%A=%%B"

set "DEST=%~1"
if "%DEST%"=="" set "DEST=%TDX_DIR%"
if "%DEST%"=="" echo [ERR] 未指定通达信目录：传入参数，或在 .build\windows.env 设置 TDX_DIR= & exit /b 1
if not exist "%DEST%\T0002\dlls\" echo [ERR] not found: %DEST% & exit /b 1

rem 构建输出目录：可用 CARGO_TARGET_DIR / TDX_CZSC_DLL_BUILD_DIR 覆盖
set "CARGO_TARGET=%CARGO_TARGET_DIR%"
if not defined CARGO_TARGET set "CARGO_TARGET=%~dp0rust_service\target"
set "DLL_BUILD=%TDX_CZSC_DLL_BUILD_DIR%"
if not defined DLL_BUILD set "DLL_BUILD=%~dp0bridge_dll\build"

rem 1) build Rust service (release)
cargo build --release --manifest-path "%~dp0rust_service\Cargo.toml"
if errorlevel 1 goto :fail

rem 2) build bridge DLL
cmake -S "%~dp0bridge_dll" -B "%DLL_BUILD%" -A x64
if errorlevel 1 goto :fail
cmake --build "%DLL_BUILD%" --config Release
if errorlevel 1 goto :fail

rem 3) copy artifacts
copy /Y "%CARGO_TARGET%\release\tdx_czsc_service.exe"  "%DEST%\T0002\dlls\" >nul
copy /Y "%DLL_BUILD%\Release\tdx_czsc_bridge.dll"      "%DEST%\T0002\dlls\" >nul
copy /Y "%DLL_BUILD%\Release\libprotobuf.dll"          "%DEST%\T0002\dlls\" >nul
copy /Y "%DLL_BUILD%\Release\abseil_dll.dll"           "%DEST%\T0002\dlls\" >nul
if errorlevel 1 echo [ERR] copy failed (TDX running?) & exit /b 1
echo Deployed to %DEST%\T0002\dlls
pause
exit /b 0

:fail
echo [ERR] build failed
pause
exit /b 1

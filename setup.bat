@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

echo CustoMouse setup
echo ================
echo Builds a release exe from source and stages it in dist\, next to a
echo copy of the bundled packs. Nothing else needs to be downloaded by
echo hand: every crate CustoMouse depends on, including the vector-trace
echo upscaling library, is plain code fetched by cargo during the build
echo below, not a separate model or binary.
echo.

where cargo >nul 2>nul
if errorlevel 1 (
    echo [FAILED] Rust is not installed, or cargo is not on PATH.
    echo Install it from https://rustup.rs ^(choose the default MSVC toolchain^),
    echo open a new terminal so PATH picks it up, then run setup.bat again.
    exit /b 1
)

echo Using:
cargo --version
rustc --version
echo.

echo Building custo-mouse in release mode. The first build fetches every
echo dependency from crates.io and compiles them all, so it can take a
echo few minutes; later runs are incremental and much faster.
echo.

cargo build --release -p custo-mouse --target-dir target
if errorlevel 1 (
    echo.
    echo [FAILED] The build did not complete. Two common causes on a fresh machine:
    echo   1. Rust's MSVC linker needs the Visual Studio C++ build tools. If the
    echo      error above mentions "link.exe" or "linker not found", install the
    echo      "Desktop development with C++" workload from:
    echo      https://visualstudio.microsoft.com/visual-cpp-build-tools/
    echo   2. A dependency failed to download. Check your internet connection
    echo      and try again.
    exit /b 1
)

if not exist dist mkdir dist
copy /y target\release\custo-mouse.exe dist\custo-mouse.exe >nul
if errorlevel 1 (
    echo [FAILED] Could not copy the built exe into dist\.
    exit /b 1
)

if exist dist\packs rmdir /s /q dist\packs
xcopy /y /e /i /q packs dist\packs >nul

echo.
echo [OK] dist\custo-mouse.exe is ready to run.
endlocal

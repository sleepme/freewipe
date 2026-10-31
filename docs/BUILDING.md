# Building freewipe

Requires Rust 1.80+ ([rustup.rs](https://rustup.rs)).

freewipe is a **Windows** program. The wiping engine, the GUI and the Explorer integration use Win32 APIs. On Linux and macOS only a CLI development build (portable I/O backend, used by the tests) compiles; it is not meant for real wiping.

## Windows (native, recommended)

1. Install Rust with the `x86_64-pc-windows-msvc` toolchain (default) and **Visual Studio Build Tools** ("Desktop development with C++").
2. Build:

```powershell
cargo build --release                         # freewipe.exe + freewipe-gui.exe
cargo build --release --no-default-features   # CLI only (smaller dependency tree)
cargo build --profile shell -p freewipe-shell # Explorer extension: target\shell\freewipe_shell.dll
cargo test --workspace
```

Output: `target\release\freewipe.exe`, `target\release\freewipe-gui.exe`, `target\shell\freewipe_shell.dll`. Copy them together with the `locales\` folder into one directory.

The icon is embedded by `build.rs` when a resource compiler (`rc.exe` from the Windows SDK) is available; otherwise the build continues without it.

## Linux → Windows (cross-compile, MinGW-w64)

```bash
# Debian/Ubuntu
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu

cargo build --release --target x86_64-pc-windows-gnu
cargo build --profile shell --target x86_64-pc-windows-gnu -p freewipe-shell
```

Output is in `target/x86_64-pc-windows-gnu/…`. The linker is configured in `.cargo/config.toml`. Test the result on real Windows, not under Wine.

## Linux / macOS (development only)

```bash
cargo build --no-default-features    # CLI, portable backend
cargo test --no-default-features
```

Windows-only commands (Explorer integration, scheduler, lock, GUI) report that they are Windows-only. Do not use these builds to wipe real data.

## Release (maintainers)

```bash
# bump version in Cargo.toml (and shell/Cargo.toml), update CHANGELOG.md
git commit -am "Release v1.0.1"
git tag v1.0.1 && git push origin main v1.0.1
```

The *Release* workflow builds on `windows-latest`, creates `freewipe-v1.0.1-windows-x64.zip` plus a SHA-256 file and attaches them to a **draft** GitHub release. Edit the notes (template: `.github/RELEASE_TEMPLATE.md`) and publish.

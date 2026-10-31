### Downloads
| File | Description |
|---|---|
| `freewipe-vX.Y.Z-windows-x64.zip` | CLI (`freewipe.exe`), desktop app (`freewipe-gui.exe`), Explorer extension, language packs |
| `*.sha256` | SHA-256 checksum of the archive |

Unpack the whole archive into one folder (e.g. `C:\Program Files\freewipe\`) and keep `locales\` next to the executables.

**Verify:** `Get-FileHash .\freewipe-vX.Y.Z-windows-x64.zip -Algorithm SHA256` must match the `.sha256` file.

> The executables are not code-signed yet, so Windows SmartScreen may warn on first run. Right-click the file → *Properties* → **Unblock**, or build from source (see [docs/BUILDING.md](https://github.com/sleepme/freewipe/blob/main/docs/BUILDING.md)).

### Support the project
If freewipe is useful to you, you can support its development: [ko-fi.com/tovld](https://ko-fi.com/tovld).

Full changelog: [CHANGELOG.md](https://github.com/sleepme/freewipe/blob/main/CHANGELOG.md)

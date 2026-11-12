# Windows

## Rule: one .exe, nothing installed

`ithub-agent.exe` and `ithub-server.exe` import only DLLs that ship with Windows.
Both builds below enforce this and fail otherwise.

| | CI (`.github/workflows/windows.yml`) | Cross from Linux (`scripts/build-windows.sh`) |
|---|---|---|
| Rust target | `x86_64-pc-windows-msvc` | `x86_64-pc-windows-gnu` (mingw-w64) |
| C libs | vcpkg `x64-windows-static` | vcpkg `x64-mingw-static` |
| C runtime | `+crt-static`: no vcruntime140.dll, no ucrt DLLs | `+crt-static`; imports msvcrt.dll, which is part of Windows |
| Check | `dumpbin /dependents` | `objdump -p` |

`+crt-static` is set per target in `.cargo/config.toml`, not through RUSTFLAGS, so
host build scripts and proc macros aren't affected. Always build with an explicit `--target`.

Measured from the mingw cross build (2026-11-11): the agent is 9.8 MB and imports
advapi32, d3d11, dxgi, gdi32, kernel32, msvcrt, ntdll, ole32, oleaut32, propsys,
shell32, user32, ws2_32, bcryptprimitives and two `api-ms-win-core/shcore` API sets.
The server is 3.2 MB and imports only kernel32, msvcrt, ntdll, ws2_32 and bcryptprimitives.

### Changes needed in voxl

None. voxl's `build.rs` calls `vcpkg::Config::new().find_package(..)`, which reads
`VCPKGRS_TRIPLET`. The vcpkg crate also requires `crt-static` with a `*-static` triplet,
and `.cargo/config.toml` supplies it. voxl's own CI uses `x64-windows-static-md`,
which links the CRT dynamically and so breaks the rule. ithub's CI uses
`x64-windows-static` instead. With mingw, libyuv's C++ runtime (libstdc++, libgcc,
winpthread) is linked statically by Rust's self-contained mingw linking. The import
check confirms this.

## Running as a service (login screen, UAC, any user)

A service runs in session 0, which has no display, so it can't capture or inject
input itself. Like MeshCentral, the agent is one exe that runs in two modes:

```
ithub-agent.exe service          # SYSTEM, session 0: tunnel to the server, owns the helper
ithub-agent.exe helper --port N  # SYSTEM, in the console session: voxl itself
```

### Service (session 0)

- Registered with the SCM (`windows-service` crate), `SERVICE_AUTO_START`, as LocalSystem.
- Keeps the control socket to the server, as on Linux and macOS, and pipes streams to
  `127.0.0.1:N`, where the helper listens.
- Accepts `SERVICE_ACCEPT_SESSIONCHANGE` and handles `SERVICE_CONTROL_SESSIONCHANGE`
  (`WTS_CONSOLE_CONNECT`, `WTS_SESSION_LOGON`, `WTS_SESSION_LOGOFF`,
  `WTS_CONSOLE_DISCONNECT`). On each event it checks `WTSGetActiveConsoleSessionId()`.
  If the session changed, it kills the helper and starts a new one in the new session.
- Starts the helper in the console session as SYSTEM, not as the user. Only SYSTEM
  can open the Winlogon desktop (lock screen, Ctrl+Alt+Del, UAC secure desktop):
  1. `OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS)`
  2. `DuplicateTokenEx(.., SecurityImpersonation, TokenPrimary)`
  3. `SetTokenInformation(dup, TokenSessionId, &console_session)`. This needs
     SeTcbPrivilege, which LocalSystem has.
  4. `CreateEnvironmentBlock`; then `CreateProcessAsUserW(dup, exe, "helper --port N
     --token T", .., CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
     lpDesktop = "winsta0\\default")`.
  5. Wait on the process handle and restart it with backoff if it exits. Use a job
     object (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) so the helper dies with the service.
- `WTSQueryUserToken` is the variant for running *as the logged-in user*. Keep it for
  later features (run-as-user commands, the user's own clipboard and mapped drives)
  and don't use it for the screen helper: a user token can't capture the lock screen or UAC.
- The service generates voxl's password `T` and passes it on the command line.
  Anything in the same session can read that, so later pass it through an inherited
  anonymous pipe or the environment block instead. The helper binds only to 127.0.0.1.

### Helper (console session)

This is voxl's `run()` as it is today. Much of voxl's Windows code is reusable unchanged:

- `win::follow_input_desktop()` (`src/win/mod.rs`) already calls
  `OpenInputDesktop` + `SetThreadDesktop`. Its doc comment says it is meant for running
  as SYSTEM to follow the lock screen and UAC prompts.
- Capture (`src/win/capture.rs`): when DXGI duplication fails with
  `DXGI_ERROR_ACCESS_LOST`, which happens on every desktop switch, the capture thread
  calls `follow_input_desktop()` and reopens duplication in a loop. Switching between
  Default and Winlogon is therefore already handled. If the resolution differs, the
  pipeline is rebuilt.
- Input (`src/win/input.rs`): if `SendInput` is blocked by UIPI or the desktop has
  changed, it calls `follow_input_desktop()` and retries. As SYSTEM this reaches
  elevated windows and the secure desktop.

Gaps to fill in a voxl fork (`vendor/voxl`) or in the agent:

1. **Ctrl+Alt+Del**: injected keystrokes can't produce the Secure Attention Sequence.
   Call `SendSAS(FALSE)` from `sas.dll` in the service, triggered by a control message
   from the server. Set `HKLM\...\Policies\System\SoftwareSASGeneration = 1` (services)
   at install.
2. **Clipboard and cursor threads** (`src/win/desktop.rs`) don't call
   `follow_input_desktop()`. On the Winlogon desktop the cursor shape and clipboard go
   stale. That's harmless, but call it in those loops too for consistent behavior.
3. **Audio as SYSTEM**: WASAPI loopback in session N as SYSTEM captures the default
   render device of that session, so it should work, but this is untested.
4. **Fast user switching or RDP**: only the console session is followed. An RDP
   session would need its own helper chosen with `WTSEnumerateSessions`, which is out of scope for now.

### Install

`ithub-agent.exe install --server wss://host --key K` copies itself to
`%ProgramFiles%\ithub\`, writes the config to `%ProgramData%\ithub\agent.toml` (ACL:
SYSTEM and Administrators only), registers the service, sets `SoftwareSASGeneration`,
and starts it. `uninstall` reverses each of these steps.

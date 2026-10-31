//! Test client that behaves like Explorer invoking a freewipe menu item:
//! `shell_client <action-index> <path>...` creates the `IExplorerCommand`
//! handler registered by `freewipe shell install` (in-process, from
//! freewipe_shell.dll), builds an `IShellItemArray` of the paths and calls
//! `Invoke`. Used to test the shell extension under Wine; Windows only.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
#[allow(unsafe_code)]
fn main() -> windows::core::Result<()> {
    use windows::core::{GUID, HSTRING};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{IExplorerCommand, ILCreateFromPathW, ILFree, SHCreateShellItemArrayFromIDLists};

    let mut args = std::env::args().skip(1);
    let index: u128 = args.next().and_then(|a| a.parse().ok()).unwrap_or(0);
    let paths: Vec<String> = args.collect();
    let clsid = GUID::from_u128(freewipe::shell::HANDLER_CLSID_BASE + index);
    // SAFETY: test code; standard COM usage on the main thread.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let cmd: IExplorerCommand = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)?;
        let pidls: Vec<*mut ITEMIDLIST> = paths.iter().map(|p| ILCreateFromPathW(&HSTRING::from(p.as_str()))).collect();
        let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        let items = SHCreateShellItemArrayFromIDLists(&consts)?;
        println!("state = {}", cmd.GetState(&items, true)?);
        let t = std::time::Instant::now();
        cmd.Invoke(&items, None)?;
        println!("invoked {} item(s) in {:?}", paths.len(), t.elapsed());
        for p in pidls {
            ILFree(Some(p));
        }
        drop(cmd);
        CoUninitialize();
    }
    Ok(())
}

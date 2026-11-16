//! windows-rs projects nullable COM interface outputs as Result<Interface>.
//! Adapt only E_NOTIMPL sentinels to the UIA-required S_OK/null at the ABI boundary.
use std::{ffi::c_void, ptr, sync::OnceLock};
use windows::{
    core::{Interface, HRESULT},
    Win32::{Foundation::E_NOTIMPL, UI::Accessibility::*},
};

static SIMPLE: OnceLock<IRawElementProviderSimple_Vtbl> = OnceLock::new();
static SIMPLE_NULLABLE: OnceLock<IRawElementProviderSimple_Vtbl> = OnceLock::new();
static FRAGMENT: OnceLock<IRawElementProviderFragment_Vtbl> = OnceLock::new();
static FRAGMENT_NULLABLE: OnceLock<IRawElementProviderFragment_Vtbl> = OnceLock::new();
static ROOT: OnceLock<IRawElementProviderFragmentRoot_Vtbl> = OnceLock::new();
static ROOT_NULLABLE: OnceLock<IRawElementProviderFragmentRoot_Vtbl> = OnceLock::new();

fn nullable(result: HRESULT) -> HRESULT {
    if result == E_NOTIMPL {
        HRESULT(0)
    } else {
        result
    }
}
unsafe extern "system" fn pattern(
    this: *mut c_void,
    id: UIA_PATTERN_ID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return windows::Win32::Foundation::E_POINTER;
    }
    *out = ptr::null_mut();
    nullable((SIMPLE.get().unwrap().GetPatternProvider)(this, id, out))
}
unsafe extern "system" fn host(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return windows::Win32::Foundation::E_POINTER;
    }
    *out = ptr::null_mut();
    nullable((SIMPLE.get().unwrap().HostRawElementProvider)(this, out))
}
unsafe extern "system" fn navigate(
    this: *mut c_void,
    direction: NavigateDirection,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return windows::Win32::Foundation::E_POINTER;
    }
    *out = ptr::null_mut();
    nullable((FRAGMENT.get().unwrap().Navigate)(this, direction, out))
}
unsafe extern "system" fn focus(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return windows::Win32::Foundation::E_POINTER;
    }
    *out = ptr::null_mut();
    nullable((ROOT.get().unwrap().GetFocus)(this, out))
}
unsafe extern "system" fn point(
    this: *mut c_void,
    x: f64,
    y: f64,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return windows::Win32::Foundation::E_POINTER;
    }
    *out = ptr::null_mut();
    nullable((ROOT.get().unwrap().ElementProviderFromPoint)(
        this, x, y, out,
    ))
}

pub(super) unsafe fn install(provider: &IRawElementProviderSimple) {
    // Each freshly constructed object is still private here. COM guarantees that
    // an interface starts with its vtable pointer. Tables retain the generated
    // identity/refcount methods and are immutable once installed.
    let original = SIMPLE.get_or_init(|| ptr::read(Interface::vtable(provider)));
    let table = SIMPLE_NULLABLE.get_or_init(|| {
        let mut table = ptr::read(original);
        table.GetPatternProvider = pattern;
        table.HostRawElementProvider = host;
        table
    });
    *provider
        .as_raw()
        .cast::<*const IRawElementProviderSimple_Vtbl>() = table;
    let fragment: IRawElementProviderFragment = provider.cast().expect("provider fragment");
    let original = FRAGMENT.get_or_init(|| ptr::read(Interface::vtable(&fragment)));
    let table = FRAGMENT_NULLABLE.get_or_init(|| {
        let mut table = ptr::read(original);
        table.Navigate = navigate;
        table
    });
    *fragment
        .as_raw()
        .cast::<*const IRawElementProviderFragment_Vtbl>() = table;
    let root: IRawElementProviderFragmentRoot = provider.cast().expect("provider root");
    let original = ROOT.get_or_init(|| ptr::read(Interface::vtable(&root)));
    let table = ROOT_NULLABLE.get_or_init(|| {
        let mut table = ptr::read(original);
        table.GetFocus = focus;
        table.ElementProviderFromPoint = point;
        table
    });
    *root
        .as_raw()
        .cast::<*const IRawElementProviderFragmentRoot_Vtbl>() = table;
}

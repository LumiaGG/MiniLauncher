use std::path::PathBuf;
use windows_sys::Win32::Foundation::{HWND, S_OK};
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};

/// 弹出原生文件夹选择框，允许用户自由指定安装路径
pub fn pick_folder(owner: HWND, default_path: Option<&str>) -> Option<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let res = pick_folder_modern(owner, default_path);
        CoUninitialize();
        res
    }
}

unsafe fn pick_folder_modern(owner: HWND, _default_path: Option<&str>) -> Option<PathBuf> {
    // CLSID_FileOpenDialog: DC1C5A9C-E88A-4DDE-A5A1-60F82A20AEF7
    const CLSID_FILE_OPEN_DIALOG: windows_sys::core::GUID = windows_sys::core::GUID {
        data1: 0xDC1C5A9C,
        data2: 0xE88A,
        data3: 0x4DDE,
        data4: [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7],
    };
    // IID_IFileOpenDialog: D57C7288-D4AD-4768-BE02-9D969532D960
    const IID_IFILE_OPEN_DIALOG: windows_sys::core::GUID = windows_sys::core::GUID {
        data1: 0xD57C7288,
        data2: 0xD4AD,
        data3: 0x4768,
        data4: [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60],
    };

    let mut dialog: *mut std::ffi::c_void = std::ptr::null_mut();
    let hr = CoCreateInstance(
        &CLSID_FILE_OPEN_DIALOG,
        std::ptr::null_mut(),
        CLSCTX_INPROC_SERVER,
        &IID_IFILE_OPEN_DIALOG,
        &mut dialog,
    );

    if hr != S_OK || dialog.is_null() {
        return None;
    }

    // IFileOpenDialog vtable layout
    // IUnknown: 0: QueryInterface, 1: AddRef, 2: Release
    // IModalWindow: 3: Show
    // IFileDialog:
    //  4: SetFileTypes, 5: SetFileTypeIndex, 6: GetFileTypeIndex, 7: Advise, 8: Unadvise,
    //  9: SetOptions, 10: GetOptions, 11: SetDefaultFolder, 12: SetFolder, 13: GetFolder,
    //  14: GetCurrentSelection, 15: SetFileName, 16: GetFileName, 17: SetTitle, 18: SetOkButtonLabel,
    //  19: SetFileNameLabel, 20: GetResult, 21: AddPlace, 22: SetDefaultExtension, 23: Close,
    //  24: SetClientGuid, 25: ClearClientData, 26: SetFilter
    // IFileOpenDialog:
    //  27: GetResults, 28: GetSelectedItems
    type Vtbl = *const usize;
    let vtbl = *(dialog as *const Vtbl);

    // GetOptions (slot 10)
    let get_options: unsafe extern "system" fn(*mut std::ffi::c_void, *mut u32) -> i32 =
        std::mem::transmute(*vtbl.add(10));
    // SetOptions (slot 9)
    let set_options: unsafe extern "system" fn(*mut std::ffi::c_void, u32) -> i32 =
        std::mem::transmute(*vtbl.add(9));
    // SetTitle (slot 17)
    let set_title: unsafe extern "system" fn(*mut std::ffi::c_void, *const u16) -> i32 =
        std::mem::transmute(*vtbl.add(17));
    // Show (slot 3)
    let show: unsafe extern "system" fn(*mut std::ffi::c_void, HWND) -> i32 =
        std::mem::transmute(*vtbl.add(3));
    // GetResult (slot 20)
    let get_result: unsafe extern "system" fn(
        *mut std::ffi::c_void,
        *mut *mut std::ffi::c_void,
    ) -> i32 = std::mem::transmute(*vtbl.add(20));
    // Release (slot 2)
    let release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
        std::mem::transmute(*vtbl.add(2));

    let mut options = 0u32;
    get_options(dialog, &mut options);
    // FOS_PICKFOLDERS = 0x20, FOS_FORCEFILESYSTEM = 0x40
    set_options(dialog, options | 0x20 | 0x40);

    let title: Vec<u16> = "选择 MiniLauncher 安装目录\0".encode_utf16().collect();
    set_title(dialog, title.as_ptr());

    let hr_show = show(dialog, owner);
    if hr_show != S_OK {
        release(dialog);
        return None;
    }

    let mut item: *mut std::ffi::c_void = std::ptr::null_mut();
    let hr_res = get_result(dialog, &mut item);
    release(dialog);

    if hr_res != S_OK || item.is_null() {
        return None;
    }

    // IShellItem vtable:
    // IUnknown: 0: QueryInterface, 1: AddRef, 2: Release
    // 3: BindToHandler, 4: GetParent, 5: GetDisplayName, 6: GetAttributes, 7: Compare
    let item_vtbl = *(item as *const Vtbl);
    let get_display_name: unsafe extern "system" fn(
        *mut std::ffi::c_void,
        u32,
        *mut *mut u16,
    ) -> i32 = std::mem::transmute(*item_vtbl.add(5));
    let item_release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
        std::mem::transmute(*item_vtbl.add(2));

    let mut path_ptr: *mut u16 = std::ptr::null_mut();
    // SIGDN_FILESYSPATH = 0x80058000
    let hr_name = get_display_name(item, 0x80058000, &mut path_ptr);
    item_release(item);

    if hr_name == S_OK && !path_ptr.is_null() {
        let mut len = 0;
        while *path_ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(path_ptr, len);
        let path_str = String::from_utf16_lossy(slice);
        CoTaskMemFree(path_ptr as _);
        Some(PathBuf::from(path_str))
    } else {
        None
    }
}

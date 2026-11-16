//! Enumerate Windows' application catalog, including packaged/Store apps.
use crate::result::{Action, Entry};
use windows::{
    core::Result,
    Win32::{
        System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED},
        UI::Shell::{
            BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, SHGetKnownFolderItem,
            KF_FLAG_DEFAULT, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
        },
    },
};

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

unsafe fn display_name(item: &IShellItem, kind: SIGDN) -> Result<String> {
    let value = item.GetDisplayName(kind)?;
    let text = value.to_string();
    CoTaskMemFree(Some(value.0.cast()));
    Ok(text?)
}

pub(super) fn entries() -> Result<Vec<Entry>> {
    unsafe {
        // Indexing runs on a worker. Balance both S_OK and S_FALSE, and leave
        // another caller's COM initialization alone if its apartment differs.
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _apartment = if initialized.is_ok() {
            Some(Apartment)
        } else if initialized == windows::Win32::Foundation::RPC_E_CHANGED_MODE {
            None
        } else {
            return Err(initialized.into());
        };
        let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
        let children: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
        let mut entries = Vec::new();
        loop {
            let mut items = [None];
            let mut fetched = 0;
            children.Next(&mut items, Some(&mut fetched))?;
            let Some(item) = items[0].take().filter(|_| fetched == 1) else {
                break;
            };
            // A stale registration must not prevent the remaining apps from
            // being indexed. Let Shell resolve each app's own launch identity.
            let (Ok(title), Ok(id)) = (
                display_name(&item, SIGDN_NORMALDISPLAY),
                display_name(&item, SIGDN_PARENTRELATIVEPARSING),
            ) else {
                continue;
            };
            if title.is_empty() || id.is_empty() {
                continue;
            }
            let target = format!("shell:AppsFolder\\{id}");
            entries.push(Entry::new(
                title,
                target.clone(),
                Action::Open(target),
                "应用",
            ));
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the current user's installed Windows application catalog"]
    fn installed_shell_apps_are_indexed_and_searchable() {
        use windows::{core::PCWSTR, Win32::UI::Shell::SHCreateItemFromParsingName};

        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
        let _apartment = Apartment;
        let apps = entries().expect("enumerate AppsFolder");
        assert!(!apps.is_empty());
        let index = crate::app_index::build_index(&crate::config::Settings::default());
        for app in &apps {
            let Action::Open(target) = &app.action else {
                panic!("expected a Shell launch target");
            };
            let target: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
            let resolved: IShellItem = unsafe {
                SHCreateItemFromParsingName(PCWSTR(target.as_ptr()), None)
                    .expect("resolve Shell launch target")
            };
            assert_eq!(
                unsafe { display_name(&resolved, SIGDN_NORMALDISPLAY).unwrap() },
                app.title
            );
            if app.title.to_lowercase().contains("uninstall") || app.title.contains("卸载") {
                continue;
            }
            assert!(
                index
                    .iter()
                    .any(|entry| entry.title.eq_ignore_ascii_case(&app.title)),
                "not indexed: {}",
                app.title
            );
            // This input is reserved for the launcher's own settings command.
            if app.title == "设置" {
                continue;
            }
            let results = crate::search::query(
                &index,
                &app.title,
                &crate::config::History::new(),
                index.len(),
            );
            assert!(
                results
                    .iter()
                    .any(|result| result.title.eq_ignore_ascii_case(&app.title)),
                "missing {}",
                app.title
            );
        }
        println!("Verified {} installed Shell applications", apps.len());
    }
}

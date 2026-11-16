//! UI Automation sees immutable snapshots; actions are posted to the window thread.
use std::{
    ptr::null_mut,
    sync::{Arc, RwLock},
};
use windows::{
    core::{implement, Error, IUnknown, Interface, Result, BOOL, HRESULT},
    Win32::{
        Foundation::{E_NOTIMPL, E_OUTOFMEMORY, HWND, LPARAM, WPARAM},
        System::{
            Com::SAFEARRAY,
            Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement},
            Variant::{VARIANT, VT_I4, VT_UNKNOWN},
        },
        UI::Accessibility::*,
    },
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub(crate) const ACTION: u32 = WM_APP + 21;
#[derive(Clone)]
pub(crate) struct Node {
    pub id: String,
    pub name: String,
    pub help: String,
    pub bounds: [f64; 4],
    pub actionable: bool,
}
struct Snapshot {
    hwnd: usize,
    menu: bool,
    alive: bool,
    name: String,
    status: String,
    revision: u64,
    nodes: Vec<Node>,
    selected: usize,
    visible: bool,
    focused: bool,
    bounds: [f64; 4],
    on_screen: Vec<usize>,
}
pub(crate) struct Tree {
    data: Arc<RwLock<Snapshot>>,
}
impl Tree {
    pub fn new(hwnd: windows_sys::Win32::Foundation::HWND, menu: bool) -> Self {
        Self {
            data: Arc::new(RwLock::new(Snapshot {
                hwnd: hwnd as usize,
                menu,
                alive: true,
                name: if menu { "操作菜单" } else { "搜索结果" }.into(),
                status: String::new(),
                revision: 0,
                nodes: Vec::new(),
                selected: 0,
                visible: false,
                focused: false,
                bounds: [0.; 4],
                on_screen: Vec::new(),
            })),
        }
    }
    pub fn nodes(&self, nodes: Vec<Node>, status: String) {
        let listening = unsafe { UiaClientsAreListening().as_bool() };
        let mut data = self.data.write().unwrap_or_else(|e| e.into_inner());
        let changed = listening
            && (data.nodes.len() != nodes.len()
                || data
                    .nodes
                    .iter()
                    .zip(&nodes)
                    .any(|(a, b)| a.id != b.id || a.name != b.name || a.help != b.help));
        let status_changed = listening && data.status != status;
        data.nodes = nodes;
        data.status = status;
        data.revision = data.revision.wrapping_add(1);
        data.on_screen.clear();
        drop(data);
        if changed {
            let provider = make(&self.data, None);
            let _ = unsafe {
                UiaRaiseStructureChangedEvent(
                    &provider,
                    StructureChangeType_ChildrenInvalidated,
                    null_mut(),
                    0,
                )
            };
        }
        if status_changed {
            let provider = make(&self.data, None);
            let _ = unsafe { UiaRaiseAutomationEvent(&provider, UIA_LiveRegionChangedEventId) };
        }
    }
    pub fn status(&self, status: &str) {
        let mut data = self.data.write().unwrap_or_else(|e| e.into_inner());
        if data.status != status {
            data.status = status.to_owned();
        }
    }
    pub fn view(
        &self,
        selected: usize,
        visible: bool,
        focused: bool,
        bounds: [f64; 4],
        rows: Vec<(usize, [f64; 4])>,
    ) {
        let changed;
        {
            let mut data = self.data.write().unwrap_or_else(|e| e.into_inner());
            changed = data.selected != selected || data.focused != focused;
            data.selected = selected;
            data.visible = visible;
            data.focused = focused;
            data.bounds = bounds;
            for index in std::mem::take(&mut data.on_screen) {
                if let Some(node) = data.nodes.get_mut(index) {
                    node.bounds = [0.; 4];
                }
            }
            for (index, rect) in rows {
                if let Some(node) = data.nodes.get_mut(index) {
                    node.bounds = rect;
                    data.on_screen.push(index);
                }
            }
        }
        if changed && visible && unsafe { UiaClientsAreListening().as_bool() } {
            let data = self.data.read().unwrap_or_else(|e| e.into_inner());
            let id = data.nodes.get(selected).map(|n| n.id.clone());
            drop(data);
            if id.is_some() {
                let provider = make(&self.data, id);
                let _ = unsafe {
                    UiaRaiseAutomationEvent(&provider, UIA_SelectionItem_ElementSelectedEventId)
                };
            }
        }
    }
    pub unsafe fn get_object(&self, wparam: usize, lparam: isize) -> Option<isize> {
        if lparam as i32 != UiaRootObjectId {
            return None;
        }
        let hwnd = self.data.read().unwrap_or_else(|e| e.into_inner()).hwnd;
        Some(
            UiaReturnRawElementProvider(
                HWND(hwnd as _),
                WPARAM(wparam),
                LPARAM(lparam),
                &make(&self.data, None),
            )
            .0,
        )
    }
    pub fn resolve(&self, wparam: usize, revision: isize) -> Option<(Option<String>, bool)> {
        let data = self.data.read().unwrap_or_else(|e| e.into_inner());
        if !data.alive || !data.visible || data.revision != revision as u64 {
            return None;
        }
        if wparam == usize::MAX {
            return Some((None, false));
        }
        data.nodes
            .get(wparam / 2)
            .map(|node| (Some(node.id.clone()), wparam & 1 != 0))
    }
    pub fn hide(&self) {
        let mut data = self.data.write().unwrap_or_else(|e| e.into_inner());
        data.visible = false;
        data.focused = false;
        data.nodes = Vec::new();
        data.on_screen = Vec::new();
        data.status = String::new();
        data.revision = data.revision.wrapping_add(1);
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        self.data.write().unwrap_or_else(|e| e.into_inner()).alive = false;
    }
}

fn unavailable() -> Error {
    Error::from_hresult(HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32))
}
fn unsupported() -> Error {
    Error::from_hresult(E_NOTIMPL)
}
fn hash(id: &str) -> u64 {
    id.bytes().fold(0xcbf29ce484222325, |value, b| {
        (value ^ b as u64).wrapping_mul(0x100000001b3)
    })
}
fn make(data: &Arc<RwLock<Snapshot>>, id: Option<String>) -> IRawElementProviderSimple {
    let provider: IRawElementProviderSimple = Provider {
        data: data.clone(),
        id,
    }
    .into();
    unsafe {
        super::nullable::install(&provider);
    }
    provider
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    ISelectionProvider,
    ISelectionItemProvider,
    IInvokeProvider
)]
struct Provider {
    data: Arc<RwLock<Snapshot>>,
    id: Option<String>,
}
impl Provider_Impl {
    fn read(&self) -> Result<std::sync::RwLockReadGuard<'_, Snapshot>> {
        let data = self.data.read().map_err(|_| unavailable())?;
        if !data.alive
            || self
                .id
                .as_ref()
                .is_some_and(|id| !data.nodes.iter().any(|n| &n.id == id))
        {
            return Err(unavailable());
        }
        Ok(data)
    }
    fn node<'a>(&self, data: &'a Snapshot) -> Option<(usize, &'a Node)> {
        self.id
            .as_ref()
            .and_then(|id| data.nodes.iter().enumerate().find(|(_, n)| &n.id == id))
    }
    fn post(&self, invoke: bool) -> Result<()> {
        let data = self.read()?;
        if !data.visible {
            return Err(unavailable());
        }
        if invoke && self.node(&data).is_some_and(|(_, node)| !node.actionable) {
            return Err(unsupported());
        }
        let argument = self
            .node(&data)
            .map_or(usize::MAX, |(i, _)| i * 2 + usize::from(invoke));
        if unsafe { PostMessageW(data.hwnd as _, ACTION, argument, data.revision as isize) } == 0 {
            return Err(unavailable());
        }
        Ok(())
    }
}
#[allow(non_snake_case)]
impl IRawElementProviderSimple_Impl for Provider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }
    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        let actionable = {
            let data = self.read()?;
            self.node(&data).is_none_or(|(_, node)| node.actionable)
        };
        let provider = make(&self.data, self.id.clone());
        if self.id.is_none() && pattern == UIA_SelectionPatternId {
            return provider.cast::<ISelectionProvider>()?.cast();
        }
        if self.id.is_some() && actionable && pattern == UIA_InvokePatternId {
            return provider.cast::<IInvokeProvider>()?.cast();
        }
        if self.id.is_some() && pattern == UIA_SelectionItemPatternId {
            return provider.cast::<ISelectionItemProvider>()?.cast();
        }
        Err(unsupported())
    }
    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let data = self.read()?;
        let node = self.node(&data);
        Ok(if property == UIA_NamePropertyId {
            VARIANT::from(node.map_or(data.name.as_str(), |(_, n)| n.name.as_str()))
        } else if property == UIA_HelpTextPropertyId {
            VARIANT::from(node.map_or("Tab 浏览文本区；Enter 执行结果", |(_, n)| {
                n.help.as_str()
            }))
        } else if property == UIA_ItemStatusPropertyId {
            VARIANT::from(data.status.as_str())
        } else if property == UIA_ControlTypePropertyId {
            VARIANT::from(if data.menu {
                if node.is_some() {
                    UIA_MenuItemControlTypeId.0
                } else {
                    UIA_MenuControlTypeId.0
                }
            } else if node.is_some() {
                UIA_ListItemControlTypeId.0
            } else {
                UIA_ListControlTypeId.0
            })
        } else if property == UIA_IsEnabledPropertyId {
            VARIANT::from(node.is_none_or(|(_, node)| node.actionable))
        } else if property == UIA_IsKeyboardFocusablePropertyId
            || property == UIA_IsControlElementPropertyId
            || property == UIA_IsContentElementPropertyId
        {
            VARIANT::from(true)
        } else if property == UIA_LiveSettingPropertyId {
            VARIANT::from(1i32)
        } else if property == UIA_HasKeyboardFocusPropertyId {
            VARIANT::from(data.focused && node.is_some_and(|(i, _)| i == data.selected))
        } else if property == UIA_IsOffscreenPropertyId {
            VARIANT::from(
                !data.visible || node.is_some_and(|(_, n)| n.bounds[2] <= 0. || n.bounds[3] <= 0.),
            )
        } else if property == UIA_AutomationIdPropertyId {
            VARIANT::from(
                format!(
                    "mini-launcher.{:x}",
                    hash(self.id.as_deref().unwrap_or("results"))
                )
                .as_str(),
            )
        } else if property == UIA_ProviderDescriptionPropertyId {
            VARIANT::from("MiniLauncher native result provider")
        } else {
            VARIANT::default()
        })
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        let data = self.read()?;
        if self.id.is_some() {
            return Err(unsupported());
        }
        unsafe { UiaHostProviderFromHwnd(HWND(data.hwnd as _)) }
    }
}
#[allow(non_snake_case)]
impl IRawElementProviderFragment_Impl for Provider_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let data = self.read()?;
        let current = self.node(&data);
        let target = if direction == NavigateDirection_Parent && current.is_some() {
            None
        } else {
            let index = if current.is_none() && direction == NavigateDirection_FirstChild {
                Some(0)
            } else if current.is_none() && direction == NavigateDirection_LastChild {
                data.nodes.len().checked_sub(1)
            } else if direction == NavigateDirection_NextSibling {
                current.map(|(i, _)| i + 1)
            } else if direction == NavigateDirection_PreviousSibling {
                current.and_then(|(i, _)| i.checked_sub(1))
            } else {
                return Err(unsupported());
            };
            Some(
                data.nodes
                    .get(index.ok_or_else(unsupported)?)
                    .ok_or_else(unsupported)?
                    .id
                    .clone(),
            )
        };
        make(&self.data, target).cast()
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        drop(self.read()?);
        let id = hash(self.id.as_deref().unwrap_or("root"));
        unsafe {
            let array = SafeArrayCreateVector(VT_I4, 0, 3);
            if array.is_null() {
                return Err(E_OUTOFMEMORY.into());
            }
            for (index, value) in [UiaAppendRuntimeId as i32, id as i32, (id >> 32) as i32]
                .iter()
                .enumerate()
            {
                if let Err(error) =
                    SafeArrayPutElement(array, &(index as i32), (value as *const i32).cast())
                {
                    let _ = SafeArrayDestroy(array);
                    return Err(error);
                }
            }
            Ok(array)
        }
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let data = self.read()?;
        let r = self.node(&data).map_or(data.bounds, |(_, n)| n.bounds);
        Ok(UiaRect {
            left: r[0],
            top: r[1],
            width: r[2],
            height: r[3],
        })
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        drop(self.read()?);
        Ok(null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        self.post(false)
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        drop(self.read()?);
        make(&self.data, None).cast()
    }
}
#[allow(non_snake_case)]
impl IRawElementProviderFragmentRoot_Impl for Provider_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let data = self.read()?;
        if !data.visible
            || x < data.bounds[0]
            || y < data.bounds[1]
            || x >= data.bounds[0] + data.bounds[2]
            || y >= data.bounds[1] + data.bounds[3]
        {
            return Err(unsupported());
        }
        let id = data
            .nodes
            .iter()
            .find(|n| {
                let r = n.bounds;
                r[2] > 0.
                    && r[3] > 0.
                    && x >= r[0]
                    && x < r[0] + r[2]
                    && y >= r[1]
                    && y < r[1] + r[3]
            })
            .map(|n| n.id.clone());
        make(&self.data, id).cast()
    }
    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let data = self.read()?;
        if !data.focused {
            return Err(unsupported());
        }
        make(
            &self.data,
            data.nodes.get(data.selected).map(|n| n.id.clone()),
        )
        .cast()
    }
}
#[allow(non_snake_case)]
impl IInvokeProvider_Impl for Provider_Impl {
    fn Invoke(&self) -> Result<()> {
        if self.id.is_none() {
            Err(unsupported())
        } else {
            self.post(true)
        }
    }
}
#[allow(non_snake_case)]
impl ISelectionItemProvider_Impl for Provider_Impl {
    fn Select(&self) -> Result<()> {
        self.post(false)
    }
    fn AddToSelection(&self) -> Result<()> {
        self.post(false)
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        Err(unsupported())
    }
    fn IsSelected(&self) -> Result<BOOL> {
        let data = self.read()?;
        Ok(self
            .node(&data)
            .is_some_and(|(i, _)| i == data.selected)
            .into())
    }
    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        drop(self.read()?);
        Ok(make(&self.data, None))
    }
}
#[allow(non_snake_case)]
impl ISelectionProvider_Impl for Provider_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let data = self.read()?;
        let id = data.nodes.get(data.selected).map(|n| n.id.clone());
        unsafe {
            let array = SafeArrayCreateVector(VT_UNKNOWN, 0, u32::from(id.is_some()));
            if array.is_null() {
                return Err(E_OUTOFMEMORY.into());
            }
            if let Some(id) = id {
                let provider = make(&self.data, Some(id));
                if let Err(error) = SafeArrayPutElement(array, &0, provider.as_raw().cast()) {
                    let _ = SafeArrayDestroy(array);
                    return Err(error);
                }
            }
            Ok(array)
        }
    }
    fn CanSelectMultiple(&self) -> Result<BOOL> {
        Ok(false.into())
    }
    fn IsSelectionRequired(&self) -> Result<BOOL> {
        Ok(true.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::BSTR;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    #[test]
    fn provider_exposes_names_roles_selection_actions_and_nullable_navigation() {
        unsafe {
            let window = CreateWindowExW(
                0,
                super::super::super::wide("STATIC").as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                200,
                120,
                null_mut(),
                null_mut(),
                null_mut(),
                std::ptr::null(),
            );
            assert!(!window.is_null());
            let tree = Tree::new(window, false);
            tree.nodes(
                vec![Node {
                    id: "stable".into(),
                    name: "词典结果".into(),
                    help: "说明".into(),
                    bounds: [0.; 4],
                    actionable: true,
                }],
                "已完成".into(),
            );
            tree.view(
                0,
                true,
                false,
                [0., 0., 200., 120.],
                vec![(0, [0., 0., 200., 50.])],
            );
            let root = make(&tree.data, None);
            assert_eq!(
                i32::try_from(&root.GetPropertyValue(UIA_ControlTypePropertyId).unwrap()).unwrap(),
                UIA_ListControlTypeId.0
            );
            let fragment: IRawElementProviderFragment = root.cast().unwrap();
            let child = fragment.Navigate(NavigateDirection_FirstChild).unwrap();
            let simple: IRawElementProviderSimple = child.cast().unwrap();
            assert_eq!(
                BSTR::try_from(&simple.GetPropertyValue(UIA_NamePropertyId).unwrap())
                    .unwrap()
                    .to_string(),
                "词典结果"
            );
            let selection: ISelectionItemProvider = simple.cast().unwrap();
            assert!(selection.IsSelected().unwrap().as_bool());
            let selected: ISelectionProvider = root.cast().unwrap();
            let array = selected.GetSelection().unwrap();
            SafeArrayDestroy(array).unwrap();
            let array = child.GetRuntimeId().unwrap();
            SafeArrayDestroy(array).unwrap();
            let mut out = std::ptr::dangling_mut::<c_void>();
            let status = (Interface::vtable(&child).Navigate)(
                child.as_raw(),
                NavigateDirection_NextSibling,
                &mut out,
            );
            assert_eq!(status, HRESULT(0));
            assert!(out.is_null());
            let mut out = std::ptr::dangling_mut::<c_void>();
            assert_eq!(
                (Interface::vtable(&root).GetPatternProvider)(
                    root.as_raw(),
                    UIA_ValuePatternId,
                    &mut out
                ),
                HRESULT(0)
            );
            assert!(out.is_null());
            let invoke: IInvokeProvider = simple.cast().unwrap();
            invoke.Invoke().unwrap();
            let mut message = std::mem::zeroed();
            assert_ne!(
                PeekMessageW(&mut message, window, ACTION, ACTION, PM_REMOVE),
                0
            );
            assert_eq!(
                tree.resolve(message.wParam, message.lParam),
                Some((Some("stable".into()), true))
            );
            tree.nodes(
                vec![Node {
                    id: "stable".into(),
                    name: "加载中".into(),
                    help: String::new(),
                    bounds: [0.; 4],
                    actionable: false,
                }],
                String::new(),
            );
            let mut out = std::ptr::dangling_mut::<c_void>();
            assert_eq!(
                (Interface::vtable(&simple).GetPatternProvider)(
                    simple.as_raw(),
                    UIA_InvokePatternId,
                    &mut out
                ),
                HRESULT(0)
            );
            assert!(out.is_null());
            assert!(
                !bool::try_from(&simple.GetPropertyValue(UIA_IsEnabledPropertyId).unwrap())
                    .unwrap()
            );
            assert!(invoke.Invoke().is_err());
            tree.nodes(Vec::new(), String::new());
            assert!(tree.resolve(message.wParam, message.lParam).is_none());
            assert!(simple.GetPropertyValue(UIA_NamePropertyId).is_err());
            assert_eq!(IsWindowVisible(window), 0);
            DestroyWindow(window);
        }
    }
    use std::ffi::c_void;
}

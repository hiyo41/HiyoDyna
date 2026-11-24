//! UI Automation fragments for controls drawn into the layered Island.
use crate::canvas::Rect;
const UNAVAILABLE: windows::core::HRESULT =
    windows::core::HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32);
use std::ptr::null_mut;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::*;
use windows::core::{IUnknown, IUnknownImpl, Interface, Result, implement};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WM_APP, WM_GETOBJECT, WM_KILLFOCUS, WM_SETFOCUS,
};

pub(crate) const MESSAGE: u32 = WM_APP + 41;

#[derive(Clone, Debug)]
pub(crate) enum Action {
    Click(f32, f32),
    Music,
    Settings,
    Seek(f64),
    Focus,
}

#[derive(Clone)]
pub(crate) struct Node {
    pub id: u32,
    pub name: String,
    pub rect: Rect,
    pub enabled: bool,
    pub action: Action,
    pub range: Option<(f64, f64)>,
}
impl Node {
    pub fn button(id: u32, name: impl Into<String>, rect: Rect, enabled: bool) -> Self {
        Self {
            id,
            name: name.into(),
            rect,
            enabled,
            action: Action::Click(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5),
            range: None,
        }
    }
}

#[derive(Default)]
struct Data {
    alive: bool,
    hidden: bool,
    nodes: Vec<Node>,
    name: String,
    origin: (f32, f32),
    scale: f32,
    native_focused: bool,
    focused: Option<u32>,
    pending_focus: Option<Option<u32>>,
    actions: Vec<Action>,
}
impl Data {
    // The outer Option means an event is needed; the inner None identifies the root.
    fn focus(&mut self, id: Option<u32>) -> Option<Option<u32>> {
        if !self.native_focused {
            self.pending_focus = Some(id);
            return None;
        }
        if self.focused == id {
            return None;
        }
        self.focused = id;
        Some(id)
    }

    fn native_focus(&mut self, focused: bool) -> Option<Option<u32>> {
        if !focused || self.hidden {
            self.native_focused = false;
            self.focused = None;
            self.pending_focus = None;
            self.actions
                .retain(|action| !matches!(action, Action::Focus));
            return None;
        }
        if self.native_focused {
            return None;
        }
        self.native_focused = true;
        self.focused = self.pending_focus.take().flatten();
        Some(self.focused)
    }
}

fn raise_focus(hwnd: usize, data: &Arc<Mutex<Data>>, target: Option<Option<u32>>) {
    let Some(id) = target else { return };
    let provider: IRawElementProviderSimple = Element {
        hwnd,
        data: data.clone(),
        id,
    }
    .into();
    // UIA can call back into the provider synchronously. Never hold Data's lock here.
    let _ = unsafe { UiaRaiseAutomationEvent(&provider, UIA_AutomationFocusChangedEventId) };
}

pub(crate) struct Host {
    hwnd: usize,
    data: Arc<Mutex<Data>>,
}
impl Host {
    pub fn new(hwnd: windows_sys::Win32::Foundation::HWND) -> Self {
        let data = Arc::new(Mutex::new(Data {
            alive: true,
            scale: 1.0,
            native_focused: unsafe { GetFocus() == hwnd },
            ..Default::default()
        }));
        unsafe {
            SetWindowSubclass(hwnd, Some(callback), 41, Arc::as_ptr(&data) as usize);
        }
        Self {
            hwnd: hwnd as usize,
            data,
        }
    }
    pub fn update(&self, name: String, nodes: Vec<Node>, scale: f32) {
        let target = if let Ok(mut data) = self.data.lock() {
            data.nodes = nodes;
            data.name = name;
            data.scale = scale;
            if let Some(Some(id)) = data.pending_focus
                && !data.nodes.iter().any(|n| n.id == id && n.enabled)
            {
                data.pending_focus = None;
            }
            if data
                .focused
                .is_some_and(|id| !data.nodes.iter().any(|n| n.id == id && n.enabled))
            {
                data.focus(None)
            } else {
                None
            }
        } else {
            None
        };
        raise_focus(self.hwnd, &self.data, target);
    }
    pub fn set_hidden(&self, hidden: bool) {
        if let Ok(mut data) = self.data.lock() {
            data.hidden = hidden;
            if hidden {
                data.native_focus(false);
                data.actions.clear();
            }
        }
    }
    pub fn origin(&self, x: i32, y: i32) {
        if let Ok(mut data) = self.data.lock() {
            data.origin = (x as f32, y as f32);
        }
    }
    pub fn take_actions(&self) -> Vec<Action> {
        self.data
            .lock()
            .map(|mut d| std::mem::take(&mut d.actions))
            .unwrap_or_default()
    }
    pub fn focus_next(&self, backwards: bool) {
        let target = if let Ok(mut d) = self.data.lock() {
            if d.hidden || !d.native_focused {
                return;
            }
            let nodes: Vec<_> = d.nodes.iter().filter(|n| n.enabled).map(|n| n.id).collect();
            if nodes.is_empty() {
                return;
            }
            let current = d.focused.and_then(|id| nodes.iter().position(|n| *n == id));
            let index = match (current, backwards) {
                (Some(i), true) => (i + nodes.len() - 1) % nodes.len(),
                (Some(i), false) => (i + 1) % nodes.len(),
                (None, true) => nodes.len() - 1,
                (None, false) => 0,
            };
            d.focus(Some(nodes[index]))
        } else {
            None
        };
        raise_focus(self.hwnd, &self.data, target);
    }
    pub fn activate(&self) {
        if let Ok(mut d) = self.data.lock() {
            if d.hidden {
                return;
            }
            let action = d
                .nodes
                .iter()
                .find(|n| Some(n.id) == d.focused && n.enabled)
                .map(|n| n.action.clone());
            if let Some(action) = action {
                d.actions.push(action);
                unsafe {
                    PostMessageW(self.hwnd as _, MESSAGE, 0, 0);
                }
            }
        }
    }
    pub fn focus_rect(&self) -> Option<Rect> {
        let d = self.data.lock().ok()?;
        if d.hidden {
            return None;
        }
        d.nodes
            .iter()
            .find(|n| Some(n.id) == d.focused)
            .map(|n| n.rect)
    }
    pub fn clear_focus(&self) {
        let target = if let Ok(mut d) = self.data.lock() {
            d.pending_focus = None;
            if d.native_focused {
                d.focus(None)
            } else {
                None
            }
        } else {
            None
        };
        raise_focus(self.hwnd, &self.data, target);
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Ok(mut d) = self.data.lock() {
            d.alive = false;
            d.nodes.clear();
        }
        unsafe {
            RemoveWindowSubclass(self.hwnd as _, Some(callback), 41);
        }
    }
}

unsafe extern "system" fn callback(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    data: usize,
) -> isize {
    if (message == WM_GETOBJECT && lparam as i32 == UiaRootObjectId)
        || message == WM_SETFOCUS
        || message == WM_KILLFOCUS
    {
        let ptr = data as *const Mutex<Data>;
        // Host keeps this allocation alive until the subclass is removed on the UI thread.
        let data = unsafe {
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        };
        if message != WM_GETOBJECT {
            let target = data
                .lock()
                .ok()
                .and_then(|mut d| d.native_focus(message == WM_SETFOCUS));
            raise_focus(hwnd as usize, &data, target);
            return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        }
        let provider: IRawElementProviderSimple = Element {
            hwnd: hwnd as usize,
            data,
            id: None,
        }
        .into();
        return unsafe {
            UiaReturnRawElementProvider(HWND(hwnd), WPARAM(wparam), LPARAM(lparam), &provider)
        }
        .0;
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider,
    IRangeValueProvider
)]
struct Element {
    hwnd: usize,
    data: Arc<Mutex<Data>>,
    id: Option<u32>,
}
impl Element {
    fn node(&self) -> Result<Node> {
        let d = self.data.lock().map_err(|_| UNAVAILABLE)?;
        if !d.alive || d.hidden {
            return Err(UNAVAILABLE.into());
        }
        d.nodes
            .iter()
            .find(|n| Some(n.id) == self.id)
            .cloned()
            .ok_or_else(|| UNAVAILABLE.into())
    }
    fn fragment(&self, id: Option<u32>) -> IRawElementProviderFragment {
        Element {
            hwnd: self.hwnd,
            data: self.data.clone(),
            id,
        }
        .into()
    }
    fn send(&self, action: Action) -> Result<()> {
        let mut d = self.data.lock().map_err(|_| UNAVAILABLE)?;
        if !d.alive || d.hidden {
            return Err(UNAVAILABLE.into());
        }
        if d.actions.len() >= 16 {
            // A full queue means the island is not draining actions; report
            // the failure so assistive tools know the activation was dropped
            // instead of seeing a silent no-op success.
            return Err(windows::core::HRESULT(UIA_E_INVALIDOPERATION as i32).into());
        }
        d.actions.push(action);
        unsafe {
            PostMessageW(self.hwnd as _, MESSAGE, 0, 0);
        }
        Ok(())
    }
}
#[allow(non_snake_case)]
impl IRawElementProviderSimple_Impl for Element_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        if self.id.is_none() {
            return Err(windows::core::Error::empty());
        }
        let node = self.node()?;
        if id == UIA_InvokePatternId && node.range.is_none() {
            return self.to_interface::<IInvokeProvider>().cast();
        }
        if id == UIA_RangeValuePatternId && node.range.is_some() {
            return self.to_interface::<IRangeValueProvider>().cast();
        }
        Err(windows::core::Error::empty())
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        if self.id.is_none() {
            let d = self.data.lock().map_err(|_| UNAVAILABLE)?;
            return Ok(if id == UIA_NamePropertyId {
                VARIANT::from(d.name.as_str())
            } else if id == UIA_ControlTypePropertyId {
                VARIANT::from(UIA_PaneControlTypeId.0)
            } else if id == UIA_HasKeyboardFocusPropertyId {
                VARIANT::from(!d.hidden && d.native_focused && d.focused.is_none())
            } else if id == UIA_IsKeyboardFocusablePropertyId || id == UIA_IsEnabledPropertyId {
                VARIANT::from(d.alive && !d.hidden)
            } else if id == UIA_IsOffscreenPropertyId {
                VARIANT::from(d.hidden)
            } else {
                VARIANT::default()
            });
        }
        let node = self.node()?;
        Ok(if id == UIA_NamePropertyId {
            VARIANT::from(node.name.as_str())
        } else if id == UIA_ControlTypePropertyId {
            VARIANT::from(if node.range.is_some() {
                UIA_SliderControlTypeId.0
            } else {
                UIA_ButtonControlTypeId.0
            })
        } else if id == UIA_IsEnabledPropertyId {
            VARIANT::from(node.enabled)
        } else if id == UIA_IsKeyboardFocusablePropertyId
            || id == UIA_IsControlElementPropertyId
            || id == UIA_IsContentElementPropertyId
        {
            VARIANT::from(true)
        } else if id == UIA_HasKeyboardFocusPropertyId {
            VARIANT::from(
                self.data
                    .lock()
                    .is_ok_and(|d| d.native_focused && d.focused == self.id),
            )
        } else if id == UIA_AutomationIdPropertyId {
            VARIANT::from(format!("island-{}", node.id).as_str())
        } else {
            VARIANT::default()
        })
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        if self.id.is_none() {
            unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as _)) }
        } else {
            Err(windows::core::Error::empty())
        }
    }
}
#[allow(non_snake_case)]
impl IRawElementProviderFragment_Impl for Element_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let d = self.data.lock().map_err(|_| UNAVAILABLE)?;
        if !d.alive || d.hidden {
            return Err(UNAVAILABLE.into());
        }
        let target = if self.id.is_none() {
            if direction == NavigateDirection_FirstChild {
                d.nodes.first().map(|n| n.id)
            } else if direction == NavigateDirection_LastChild {
                d.nodes.last().map(|n| n.id)
            } else {
                None
            }
        } else if direction == NavigateDirection_Parent {
            return Ok(self.fragment(None));
        } else if let Some(index) = d.nodes.iter().position(|n| Some(n.id) == self.id) {
            if direction == NavigateDirection_NextSibling {
                d.nodes.get(index + 1).map(|n| n.id)
            } else if direction == NavigateDirection_PreviousSibling {
                index
                    .checked_sub(1)
                    .and_then(|i| d.nodes.get(i))
                    .map(|n| n.id)
            } else {
                None
            }
        } else {
            None
        };
        target
            .map(|id| self.fragment(Some(id)))
            .ok_or_else(windows::core::Error::empty)
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        let Some(id) = self.id else {
            return Ok(null_mut());
        };
        unsafe {
            let array = SafeArrayCreateVector(VT_I4, 0, 2);
            if array.is_null() {
                return Err(E_NOTIMPL.into());
            }
            for (index, value) in [UiaAppendRuntimeId as i32, id as i32]
                .into_iter()
                .enumerate()
            {
                SafeArrayPutElement(array, &(index as i32), (&value as *const i32).cast())?;
            }
            Ok(array)
        }
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let node = self.node().ok();
        let d = self.data.lock().map_err(|_| UNAVAILABLE)?;
        // The fragment root has no node of its own; reporting a zero rect
        // would make magnifiers and screen readers treat the whole island as
        // a single invisible point. Its extent is the union of the nodes.
        let rect = match (self.id, node) {
            (None, _) if !d.nodes.is_empty() => {
                let mut left = f32::MAX;
                let mut top = f32::MAX;
                let mut right = f32::MIN;
                let mut bottom = f32::MIN;
                for node in &d.nodes {
                    left = left.min(node.rect.x);
                    top = top.min(node.rect.y);
                    right = right.max(node.rect.x + node.rect.w);
                    bottom = bottom.max(node.rect.y + node.rect.h);
                }
                Rect::new(left, top, (right - left).max(0.0), (bottom - top).max(0.0))
            }
            (_, node) => node.map_or(Rect::default(), |n| n.rect),
        };
        Ok(UiaRect {
            left: f64::from(d.origin.0 + rect.x * d.scale),
            top: f64::from(d.origin.1 + rect.y * d.scale),
            width: f64::from(rect.w * d.scale),
            height: f64::from(rect.h * d.scale),
        })
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        let target = {
            let mut d = self.data.lock().map_err(|_| UNAVAILABLE)?;
            if !d.alive || d.hidden {
                return Err(UNAVAILABLE.into());
            }
            if let Some(id) = self.id {
                let node = d.nodes.iter().find(|n| n.id == id).ok_or(UNAVAILABLE)?;
                if !node.enabled {
                    return Err(windows::core::HRESULT(UIA_E_ELEMENTNOTENABLED as i32).into());
                }
            }
            // Queue the native request atomically with its virtual target, so the UI
            // thread cannot receive WM_SETFOCUS before the target is available.
            if !d
                .actions
                .iter()
                .any(|action| matches!(action, Action::Focus))
            {
                if d.actions.len() >= 16 {
                    return Err(windows::core::HRESULT(UIA_E_INVALIDOPERATION as i32).into());
                }
                d.actions.push(Action::Focus);
            }
            d.focus(self.id)
        };
        unsafe { PostMessageW(self.hwnd as _, MESSAGE, 0, 0) };
        raise_focus(self.hwnd, &self.data, target);
        Ok(())
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Element {
            hwnd: self.hwnd,
            data: self.data.clone(),
            id: None,
        }
        .into())
    }
}
#[allow(non_snake_case)]
impl IRawElementProviderFragmentRoot_Impl for Element_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let d = self.data.lock().map_err(|_| UNAVAILABLE)?;
        if !d.alive || d.hidden {
            return Err(UNAVAILABLE.into());
        }
        let id = d
            .nodes
            .iter()
            .find(|n| {
                n.rect.contains(
                    (x as f32 - d.origin.0) / d.scale,
                    (y as f32 - d.origin.1) / d.scale,
                )
            })
            .map(|n| n.id);
        Ok(self.fragment(id))
    }
    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let d = self.data.lock().map_err(|_| UNAVAILABLE)?;
        if !d.alive || d.hidden {
            return Err(UNAVAILABLE.into());
        }
        if !d.native_focused {
            return Err(windows::core::Error::empty());
        }
        Ok(self.fragment(d.focused))
    }
}
#[allow(non_snake_case)]
impl IInvokeProvider_Impl for Element_Impl {
    fn Invoke(&self) -> Result<()> {
        let node = self.node()?;
        if !node.enabled {
            return Err(windows::core::HRESULT(UIA_E_ELEMENTNOTENABLED as i32).into());
        }
        self.send(node.action)
    }
}
#[allow(non_snake_case)]
impl IRangeValueProvider_Impl for Element_Impl {
    fn SetValue(&self, value: f64) -> Result<()> {
        let node = self.node()?;
        let (_, max) = node.range.ok_or(E_INVALIDARG)?;
        if !node.enabled || !value.is_finite() || !(0.0..=max).contains(&value) {
            return Err(E_INVALIDARG.into());
        }
        self.send(Action::Seek(value))
    }
    fn Value(&self) -> Result<f64> {
        Ok(self.node()?.range.ok_or(E_INVALIDARG)?.0)
    }
    fn IsReadOnly(&self) -> Result<windows::core::BOOL> {
        Ok((!self.node()?.enabled).into())
    }
    fn Maximum(&self) -> Result<f64> {
        Ok(self.node()?.range.ok_or(E_INVALIDARG)?.1)
    }
    fn Minimum(&self) -> Result<f64> {
        Ok(0.0)
    }
    fn LargeChange(&self) -> Result<f64> {
        Ok(10.0)
    }
    fn SmallChange(&self) -> Result<f64> {
        Ok(5.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessible_focus_waits_for_native_focus() {
        let data = Arc::new(Mutex::new(Data {
            alive: true,
            nodes: vec![Node::button(1, "Play", Rect::default(), true)],
            ..Default::default()
        }));
        let provider: IRawElementProviderFragment = Element {
            hwnd: 0,
            data: data.clone(),
            id: Some(1),
        }
        .into();
        unsafe { provider.SetFocus() }.unwrap();
        let d = data.lock().unwrap();
        assert_eq!(
            d.focused, None,
            "a posted request has not acquired native focus"
        );
        assert!(matches!(d.actions.as_slice(), [Action::Focus]));
    }

    #[test]
    fn accessible_root_has_no_focused_fragment_without_native_focus() {
        let provider: IRawElementProviderFragmentRoot = Element {
            hwnd: 0,
            data: Arc::new(Mutex::new(Data {
                alive: true,
                focused: Some(1),
                ..Default::default()
            })),
            id: None,
        }
        .into();
        assert!(unsafe { provider.GetFocus() }.is_err());
    }

    #[test]
    fn focus_notifications_follow_native_focus_and_suppress_duplicates() {
        let mut d = Data::default();
        assert_eq!(d.focus(Some(1)), None);
        assert_eq!(d.focused, None);
        assert_eq!(d.native_focus(true), Some(Some(1)));
        assert_eq!(d.focused, Some(1));
        assert_eq!(d.native_focus(true), None);
        assert_eq!(d.focus(Some(1)), None);
        assert_eq!(d.focus(Some(2)), Some(Some(2)));
        assert_eq!(d.focus(None), Some(None));
        assert_eq!(d.focus(None), None);
    }

    #[test]
    fn native_focus_loss_clears_virtual_focus_and_pending_requests() {
        let mut d = Data::default();
        d.focus(Some(1));
        d.native_focus(true);
        d.actions = vec![Action::Focus, Action::Music];
        assert_eq!(d.native_focus(false), None);
        assert_eq!(d.focused, None);
        assert_eq!(d.pending_focus, None);
        assert!(matches!(d.actions.as_slice(), [Action::Music]));
        assert_eq!(d.native_focus(true), Some(None));
        d.native_focus(false);
        d.focus(Some(2));
        d.native_focus(false);
        assert_eq!(d.native_focus(true), Some(None));
    }

    #[test]
    fn accessible_focus_properties_require_native_ownership() {
        let data = Arc::new(Mutex::new(Data {
            alive: true,
            focused: Some(1),
            nodes: vec![Node::button(1, "Play", Rect::default(), true)],
            ..Default::default()
        }));
        let child: IRawElementProviderSimple = Element {
            hwnd: 0,
            data: data.clone(),
            id: Some(1),
        }
        .into();
        let root: IRawElementProviderSimple = Element {
            hwnd: 0,
            data: data.clone(),
            id: None,
        }
        .into();
        let focused = |provider: &IRawElementProviderSimple| {
            bool::try_from(
                &unsafe { provider.GetPropertyValue(UIA_HasKeyboardFocusPropertyId) }.unwrap(),
            )
            .unwrap()
        };
        assert!(!focused(&child));
        assert!(!focused(&root));
        data.lock().unwrap().native_focused = true;
        assert!(focused(&child));
        assert!(!focused(&root));
        data.lock().unwrap().focus(None);
        assert!(!focused(&child));
        assert!(focused(&root));
        data.lock().unwrap().native_focus(false);
        assert!(!focused(&child));
        assert!(!focused(&root));
    }

    #[test]
    fn accessible_focus_rejects_unavailable_and_disabled_targets() {
        let data = Arc::new(Mutex::new(Data {
            alive: true,
            nodes: vec![Node::button(1, "Play", Rect::default(), false)],
            ..Default::default()
        }));
        let provider: IRawElementProviderFragment = Element {
            hwnd: 0,
            data: data.clone(),
            id: Some(1),
        }
        .into();
        assert!(unsafe { provider.SetFocus() }.is_err());
        data.lock().unwrap().nodes.clear();
        assert!(unsafe { provider.SetFocus() }.is_err());
        let mut d = data.lock().unwrap();
        assert_eq!(d.pending_focus, None);
        assert!(d.actions.is_empty());
        d.alive = false;
        drop(d);
        assert!(unsafe { provider.SetFocus() }.is_err());
    }

    #[test]
    fn updating_nodes_discards_removed_pending_focus() {
        let host = Host {
            hwnd: 0,
            data: Arc::new(Mutex::new(Data {
                alive: true,
                nodes: vec![Node::button(1, "Play", Rect::default(), true)],
                ..Default::default()
            })),
        };
        host.data.lock().unwrap().focus(Some(1));
        host.update("Island".into(), vec![], 1.0);
        assert_eq!(host.data.lock().unwrap().native_focus(true), Some(None));
    }

    #[test]
    fn hiding_discards_queued_actions_and_rejects_cached_providers_until_restored() {
        let host = Host {
            hwnd: 0,
            data: Arc::new(Mutex::new(Data {
                alive: true,
                nodes: vec![Node::button(1, "Play", Rect::default(), true)],
                native_focused: true,
                focused: Some(1),
                pending_focus: Some(Some(1)),
                actions: vec![Action::Focus, Action::Seek(20.0)],
                ..Default::default()
            })),
        };
        let root: IRawElementProviderFragment = Element {
            hwnd: 0,
            data: host.data.clone(),
            id: None,
        }
        .into();
        let child: IInvokeProvider = Element {
            hwnd: 0,
            data: host.data.clone(),
            id: Some(1),
        }
        .into();
        host.set_hidden(true);
        assert!(host.take_actions().is_empty());
        assert!(host.focus_rect().is_none());
        assert!(unsafe { root.SetFocus() }.is_err());
        assert!(unsafe { child.Invoke() }.is_err());
        assert_eq!(host.data.lock().unwrap().native_focus(true), None);
        host.focus_next(false);
        host.activate();
        assert!(host.take_actions().is_empty());
        assert_eq!(host.data.lock().unwrap().pending_focus, None);
        host.set_hidden(false);
        unsafe { child.Invoke() }.unwrap();
        assert!(matches!(
            host.take_actions().as_slice(),
            [Action::Click(_, _)]
        ));
        unsafe { root.SetFocus() }.unwrap();
        assert!(matches!(host.take_actions().as_slice(), [Action::Focus]));
    }

    #[test]
    fn accessible_transport_enforces_disabled_state_and_queues_enabled_action() {
        let data = Arc::new(Mutex::new(Data {
            alive: true,
            nodes: vec![Node::button(
                1,
                "Next track",
                Rect::new(10.0, 10.0, 40.0, 40.0),
                false,
            )],
            ..Default::default()
        }));
        let provider: IInvokeProvider = Element {
            hwnd: 0,
            data: data.clone(),
            id: Some(1),
        }
        .into();
        assert!(unsafe { provider.Invoke() }.is_err());
        data.lock().unwrap().nodes[0].enabled = true;
        unsafe { provider.Invoke() }.unwrap();
        assert_eq!(data.lock().unwrap().actions.len(), 1);
        data.lock().unwrap().alive = false;
        assert!(unsafe { provider.Invoke() }.is_err());
    }

    #[test]
    fn accessible_seek_rejects_nonfinite_and_out_of_range_values() {
        let mut node = Node::button(7, "Playback position", Rect::default(), true);
        node.range = Some((10.0, 200.0));
        let data = Arc::new(Mutex::new(Data {
            alive: true,
            nodes: vec![node],
            ..Default::default()
        }));
        let provider: IRangeValueProvider = Element {
            hwnd: 0,
            data: data.clone(),
            id: Some(7),
        }
        .into();
        assert!(unsafe { provider.SetValue(f64::NAN) }.is_err());
        assert!(unsafe { provider.SetValue(201.0) }.is_err());
        unsafe { provider.SetValue(50.0) }.unwrap();
        assert!(matches!(
            data.lock().unwrap().actions.first(),
            Some(Action::Seek(50.0))
        ));
    }
}

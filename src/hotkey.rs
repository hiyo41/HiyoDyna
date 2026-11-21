#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shortcut {
    pub modifiers: u32,
    pub key: u32,
}

impl Default for Shortcut {
    fn default() -> Self {
        Self {
            modifiers: 3,
            key: 0x44,
        }
    }
}

impl Shortcut {
    pub fn parse(text: &str) -> Option<Self> {
        let (modifiers, key) = text.split_once(':')?;
        let shortcut = Self {
            modifiers: modifiers.parse().ok()?,
            key: key.parse().ok()?,
        };
        shortcut.is_valid().then_some(shortcut)
    }
    pub fn serialize(self) -> String {
        format!("{}:{}", self.modifiers, self.key)
    }
    pub fn label(self) -> String {
        let mut parts = Vec::new();
        for (mask, name) in [(2, "Ctrl"), (1, "Alt"), (4, "Shift"), (8, "Win")] {
            if self.modifiers & mask != 0 {
                parts.push(name.to_owned());
            }
        }
        let key = match self.key {
            0x30..=0x39 | 0x41..=0x5a => char::from_u32(self.key).unwrap_or('?').to_string(),
            0x70..=0x87 => format!("F{}", self.key - 0x6f),
            0x20 => "Space".into(),
            0x21 => "Page Up".into(),
            0x22 => "Page Down".into(),
            0x23 => "End".into(),
            0x24 => "Home".into(),
            0x25 => "Left".into(),
            0x26 => "Up".into(),
            0x27 => "Right".into(),
            0x28 => "Down".into(),
            0x2d => "Insert".into(),
            0x2e => "Delete".into(),
            _ => "?".into(),
        };
        parts.push(key);
        parts.join("+")
    }
    pub fn is_valid(self) -> bool {
        self.modifiers != 0
            && self.modifiers & !15 == 0
            && matches!(self.key, 0x20..=0x28 | 0x2d..=0x2e | 0x30..=0x39 | 0x41..=0x5a | 0x70..=0x87)
    }
    pub fn from_key(key: u32) -> Option<Self> {
        #[cfg(windows)]
        let modifiers = unsafe {
            #[link(name = "user32")]
            unsafe extern "system" {
                fn GetKeyState(key: i32) -> i16;
            }
            u32::from(GetKeyState(0x12) < 0)
                | (u32::from(GetKeyState(0x11) < 0) << 1)
                | (u32::from(GetKeyState(0x10) < 0) << 2)
                | (u32::from(GetKeyState(0x5b) < 0 || GetKeyState(0x5c) < 0) << 3)
        };
        #[cfg(not(windows))]
        let modifiers = 0;
        let shortcut = Self { modifiers, key };
        shortcut.is_valid().then_some(shortcut)
    }
}

pub(crate) fn registration(
    current: Option<(i32, Shortcut)>,
    requested: Shortcut,
    mut register: impl FnMut(i32, Shortcut) -> bool,
) -> Option<(i32, Shortcut)> {
    if !requested.is_valid() {
        return None;
    }
    if current.is_some_and(|(_, shortcut)| shortcut == requested) {
        return current;
    }
    let id = if current.is_some_and(|(id, _)| id == 73) {
        74
    } else {
        73
    };
    register(id, requested).then_some((id, requested))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcut_defaults_and_saved_navigation_bindings_are_readable() {
        let shortcut = Shortcut::default();
        assert_eq!(shortcut.label(), "Ctrl+Alt+D");
        assert_eq!(Shortcut::parse(&shortcut.serialize()), Some(shortcut));
        assert_eq!(Shortcut::parse("6:37").unwrap().label(), "Ctrl+Shift+Left");
        assert_eq!(Shortcut::parse("9:123").unwrap().label(), "Alt+Win+F12");
    }
    #[test]
    fn shortcut_rejects_missing_unknown_and_modifier_only_combinations() {
        for invalid in [
            "0:68", "3:17", "3:91", "16:68", "3:0", "3:255", "garbage", "3:-1",
        ] {
            assert!(Shortcut::parse(invalid).is_none(), "{invalid}");
        }
        assert!(Shortcut::default().is_valid());
    }
    #[test]
    fn failed_rebinding_keeps_old_registration_and_success_uses_a_different_id() {
        let old = Some((73, Shortcut::default()));
        let requested = Shortcut {
            modifiers: 6,
            key: 0x25,
        };
        let mut current = old;
        let failed = registration(current, requested, |_, _| false);
        if let Some(next) = failed {
            current = Some(next);
        }
        assert_eq!(current, old);
        let next = registration(current, requested, |id, key| {
            assert_eq!(id, 74);
            assert_eq!(key, requested);
            true
        })
        .unwrap();
        assert_eq!(next, (74, requested));
        assert_eq!(
            registration(Some(next), requested, |_, _| panic!(
                "unchanged bindings need no re-registration"
            )),
            Some(next)
        );
    }
}

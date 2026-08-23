use crate::register::Registers;
use helix_core::unicode::width::UnicodeWidthStr;
use std::{borrow::Cow, fmt::Write};

/// Info 框位置(JS set_keymap_hint 可指定;默认右下角)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InfoPosition {
    #[default]
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
    Center,
}

#[derive(Debug)]
/// Info box used in editor. Rendering logic will be in other crate.
pub struct Info {
    /// Title shown at top.
    pub title: Cow<'static, str>,
    /// Text body, should contain newlines.
    pub text: String,
    /// Body width.
    pub width: u16,
    /// Body height.
    pub height: u16,
    /// 渲染位置(全屏坐标基准;默认右下角)
    pub position: InfoPosition,
}

impl Info {
    pub fn new<T, K, V>(title: T, body: &[(K, V)]) -> Self
    where
        T: Into<Cow<'static, str>>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let title = title.into();
        if body.is_empty() {
            return Self {
                height: 1,
                width: title.len() as u16,
                text: "".to_string(),
                title,
                position: InfoPosition::default(),
            };
        }

        let item_width = body
            .iter()
            .map(|(item, _)| item.as_ref().width())
            .max()
            .unwrap();
        let mut text = String::new();

        for (item, desc) in body {
            let _ = writeln!(
                text,
                "{:width$}  {}",
                item.as_ref(),
                desc.as_ref(),
                width = item_width
            );
        }

        Self {
            title,
            width: text.lines().map(|l| l.width()).max().unwrap() as u16,
            height: body.len() as u16,
            text,
            position: InfoPosition::default(),
        }
    }

    pub fn from_registers(title: impl Into<Cow<'static, str>>, registers: &Registers) -> Self {
        let body: Vec<_> = registers
            .iter_preview()
            .map(|(ch, preview)| (ch.to_string(), preview))
            .collect();

        let mut infobox = Self::new(title, &body);
        infobox.width = 30; // copied content could be very long
        infobox
    }
}

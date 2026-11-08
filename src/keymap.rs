//! Browser key codes and text to USB HID keyboard usages (US layout).

/// A modifier's bit in the report's first byte, for a `KeyboardEvent.code`.
pub fn modifier(code: &str) -> Option<u8> {
    Some(match code {
        "ControlLeft" => 0x01,
        "ShiftLeft" => 0x02,
        "AltLeft" => 0x04,
        "MetaLeft" | "OSLeft" => 0x08,
        "ControlRight" => 0x10,
        "ShiftRight" => 0x20,
        "AltRight" => 0x40,
        "MetaRight" | "OSRight" => 0x80,
        _ => return None,
    })
}

/// The usage for a `KeyboardEvent.code` that is not a modifier. Physical keys, so the
/// target's own layout decides what character a key makes, as on a real keyboard.
pub fn usage(code: &str) -> Option<u8> {
    if let Some(l) = code.strip_prefix("Key") {
        let c = l.as_bytes();
        if c.len() == 1 && c[0].is_ascii_uppercase() {
            return Some(0x04 + (c[0] - b'A'));
        }
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return match d.as_bytes() {
            [b'0'] => Some(0x27),
            [n @ b'1'..=b'9'] => Some(0x1E + (n - b'1')),
            _ => None,
        };
    }
    if let Some(d) = code.strip_prefix("Numpad") {
        match d.as_bytes() {
            [b'0'] => return Some(0x62),
            [n @ b'1'..=b'9'] => return Some(0x59 + (n - b'1')),
            _ => {}
        }
    }
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=12).contains(&n).then(|| 0x3A + n - 1);
    }
    Some(match code {
        "Enter" => 0x28,
        "Escape" => 0x29,
        "Backspace" => 0x2A,
        "Tab" => 0x2B,
        "Space" => 0x2C,
        "Minus" => 0x2D,
        "Equal" => 0x2E,
        "BracketLeft" => 0x2F,
        "BracketRight" => 0x30,
        "Backslash" => 0x31,
        "IntlHash" => 0x32,
        "Semicolon" => 0x33,
        "Quote" => 0x34,
        "Backquote" => 0x35,
        "Comma" => 0x36,
        "Period" => 0x37,
        "Slash" => 0x38,
        "CapsLock" => 0x39,
        "PrintScreen" => 0x46,
        "ScrollLock" => 0x47,
        "Pause" => 0x48,
        "Insert" => 0x49,
        "Home" => 0x4A,
        "PageUp" => 0x4B,
        "Delete" => 0x4C,
        "End" => 0x4D,
        "PageDown" => 0x4E,
        "ArrowRight" => 0x4F,
        "ArrowLeft" => 0x50,
        "ArrowDown" => 0x51,
        "ArrowUp" => 0x52,
        "NumLock" => 0x53,
        "NumpadDivide" => 0x54,
        "NumpadMultiply" => 0x55,
        "NumpadSubtract" => 0x56,
        "NumpadAdd" => 0x57,
        "NumpadEnter" => 0x58,
        "NumpadDecimal" => 0x63,
        "IntlBackslash" => 0x64,
        "ContextMenu" => 0x65,
        _ => return None,
    })
}

/// The key (and whether Shift is held) that types `c` on a US layout.
pub fn char_key(c: char) -> Option<(u8, bool)> {
    let plain = |u| Some((u, false));
    let shift = |u| Some((u, true));
    match c {
        'a'..='z' => plain(0x04 + (c as u8 - b'a')),
        'A'..='Z' => shift(0x04 + (c as u8 - b'A')),
        '1'..='9' => plain(0x1E + (c as u8 - b'1')),
        '0' => plain(0x27),
        '!' => shift(0x1E),
        '@' => shift(0x1F),
        '#' => shift(0x20),
        '$' => shift(0x21),
        '%' => shift(0x22),
        '^' => shift(0x23),
        '&' => shift(0x24),
        '*' => shift(0x25),
        '(' => shift(0x26),
        ')' => shift(0x27),
        '\n' => plain(0x28),
        '\t' => plain(0x2B),
        ' ' => plain(0x2C),
        '-' => plain(0x2D),
        '_' => shift(0x2D),
        '=' => plain(0x2E),
        '+' => shift(0x2E),
        '[' => plain(0x2F),
        '{' => shift(0x2F),
        ']' => plain(0x30),
        '}' => shift(0x30),
        '\\' => plain(0x31),
        '|' => shift(0x31),
        ';' => plain(0x33),
        ':' => shift(0x33),
        '\'' => plain(0x34),
        '"' => shift(0x34),
        '`' => plain(0x35),
        '~' => shift(0x35),
        ',' => plain(0x36),
        '<' => shift(0x36),
        '.' => plain(0x37),
        '>' => shift(0x37),
        '/' => plain(0x38),
        '?' => shift(0x38),
        _ => None,
    }
}

/// What a key makes on a US layout, for the demo screen: the inverse of [`char_key`].
pub fn key_char(usage: u8, shifted: bool) -> Option<char> {
    // Every printable ASCII character has exactly one key, so search the table.
    (' '..='~').chain(['\n', '\t']).find(|&c| char_key(c) == Some((usage, shifted)))
}

/// Target keyboard layouts Paste text knows.
pub const LAYOUTS: &[&str] = &["us", "uk", "de", "fr", "es"];

pub const SHIFT: u8 = 0x02;
/// Right Alt, which is AltGr on the layouts that have one.
pub const ALTGR: u8 = 0x40;

/// A key a layout defines differently from US: usage, then what it types plain, with
/// Shift and with AltGr. None is nothing, or a dead key, which Paste text does not use.
type Row = (u8, Option<char>, Option<char>, Option<char>);

const fn k(u: u8, n: char, s: char) -> Row {
    (u, Some(n), Some(s), None)
}
const fn ka(u: u8, n: char, s: char, a: char) -> Row {
    (u, Some(n), Some(s), Some(a))
}
/// A key that types nothing Paste text can use (dead keys, or absent on that keyboard).
const fn none(u: u8) -> Row {
    (u, None, None, None)
}

/// The keys `layout` changes from US. ISO keyboards have the extra key by the left Shift
/// (0x64) and the one by Enter (0x32) instead of US backslash (0x31).
const UK: &[Row] = &[
    k(0x1F, '2', '"'),
    k(0x20, '3', '£'),
    ka(0x21, '4', '$', '€'),
    k(0x34, '\'', '@'),
    k(0x32, '#', '~'),
    none(0x31),
    k(0x64, '\\', '|'),
    ka(0x35, '`', '¬', '¦'),
];

const DE: &[Row] = &[
    k(0x1C, 'z', 'Z'),
    k(0x1D, 'y', 'Y'),
    ka(0x14, 'q', 'Q', '@'),
    ka(0x08, 'e', 'E', '€'),
    ka(0x10, 'm', 'M', 'µ'),
    k(0x1E, '1', '!'),
    ka(0x1F, '2', '"', '²'),
    ka(0x20, '3', '§', '³'),
    k(0x21, '4', '$'),
    k(0x22, '5', '%'),
    k(0x23, '6', '&'),
    ka(0x24, '7', '/', '{'),
    ka(0x25, '8', '(', '['),
    ka(0x26, '9', ')', ']'),
    ka(0x27, '0', '=', '}'),
    ka(0x2D, 'ß', '?', '\\'),
    none(0x2E),
    k(0x2F, 'ü', 'Ü'),
    ka(0x30, '+', '*', '~'),
    none(0x31),
    k(0x32, '#', '\''),
    k(0x33, 'ö', 'Ö'),
    k(0x34, 'ä', 'Ä'),
    (0x35, None, Some('°'), None),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    ka(0x64, '<', '>', '|'),
];

const FR: &[Row] = &[
    k(0x14, 'a', 'A'),
    k(0x04, 'q', 'Q'),
    k(0x1A, 'z', 'Z'),
    k(0x1D, 'w', 'W'),
    k(0x33, 'm', 'M'),
    k(0x10, ',', '?'),
    ka(0x08, 'e', 'E', '€'),
    k(0x1E, '&', '1'),
    k(0x1F, 'é', '2'),
    ka(0x20, '"', '3', '#'),
    ka(0x21, '\'', '4', '{'),
    ka(0x22, '(', '5', '['),
    ka(0x23, '-', '6', '|'),
    k(0x24, 'è', '7'),
    ka(0x25, '_', '8', '\\'),
    ka(0x26, 'ç', '9', '^'),
    ka(0x27, 'à', '0', '@'),
    ka(0x2D, ')', '°', ']'),
    ka(0x2E, '=', '+', '}'),
    none(0x2F),
    ka(0x30, '$', '£', '¤'),
    none(0x31),
    k(0x32, '*', 'µ'),
    k(0x34, 'ù', '%'),
    (0x35, Some('²'), None, None),
    k(0x36, ';', '.'),
    k(0x37, ':', '/'),
    k(0x38, '!', '§'),
    k(0x64, '<', '>'),
];

const ES: &[Row] = &[
    ka(0x08, 'e', 'E', '€'),
    ka(0x1E, '1', '!', '|'),
    ka(0x1F, '2', '"', '@'),
    ka(0x20, '3', '·', '#'),
    ka(0x21, '4', '$', '~'),
    k(0x22, '5', '%'),
    ka(0x23, '6', '&', '¬'),
    k(0x24, '7', '/'),
    k(0x25, '8', '('),
    k(0x26, '9', ')'),
    k(0x27, '0', '='),
    k(0x2D, '\'', '?'),
    k(0x2E, '¡', '¿'),
    (0x2F, None, None, Some('[')),
    ka(0x30, '+', '*', ']'),
    none(0x31),
    ka(0x32, 'ç', 'Ç', '}'),
    k(0x33, 'ñ', 'Ñ'),
    (0x34, None, None, Some('{')),
    ka(0x35, 'º', 'ª', '\\'),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    k(0x64, '<', '>'),
];

fn overrides(layout: &str) -> &'static [Row] {
    match layout {
        "uk" => UK,
        "de" => DE,
        "fr" => FR,
        "es" => ES,
        _ => &[],
    }
}

/// The key and modifiers that type `c` on a target set to `layout`.
pub fn char_key_in(c: char, layout: &str) -> Option<(u8, u8)> {
    let rows = overrides(layout);
    for &(u, n, s, a) in rows {
        if n == Some(c) {
            return Some((u, 0));
        }
        if s == Some(c) {
            return Some((u, SHIFT));
        }
        if a == Some(c) {
            return Some((u, ALTGR));
        }
    }
    // Otherwise where US has it, unless the layout put something else on that key.
    let (u, shifted) = char_key(c)?;
    if rows.iter().any(|r| r.0 == u) {
        return None;
    }
    Some((u, if shifted { SHIFT } else { 0 }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_named_keys() {
        assert_eq!(usage("KeyA"), Some(0x04));
        assert_eq!(usage("KeyZ"), Some(0x1D));
        assert_eq!(usage("Digit1"), Some(0x1E));
        assert_eq!(usage("Digit0"), Some(0x27));
        assert_eq!(usage("F1"), Some(0x3A));
        assert_eq!(usage("F12"), Some(0x45));
        assert_eq!(usage("F13"), None);
        assert_eq!(usage("Numpad1"), Some(0x59));
        assert_eq!(usage("Numpad0"), Some(0x62));
        assert_eq!(usage("NumpadEnter"), Some(0x58));
        assert_eq!(usage("ArrowUp"), Some(0x52));
        assert_eq!(usage("Keyboard"), None);
        assert_eq!(modifier("ShiftRight"), Some(0x20));
        assert_eq!(usage("ShiftRight"), None);
    }

    #[test]
    fn layouts_put_characters_where_those_keyboards_have_them() {
        assert_eq!(char_key_in('@', "us"), Some((0x1F, SHIFT)));
        assert_eq!(char_key_in('"', "uk"), Some((0x1F, SHIFT)));
        assert_eq!(char_key_in('@', "uk"), Some((0x34, SHIFT)));
        assert_eq!(char_key_in('£', "uk"), Some((0x20, SHIFT)));
        assert_eq!(char_key_in('z', "de"), Some((0x1C, 0)));
        assert_eq!(char_key_in('Y', "de"), Some((0x1D, SHIFT)));
        assert_eq!(char_key_in('@', "de"), Some((0x14, ALTGR)));
        assert_eq!(char_key_in('ß', "de"), Some((0x2D, 0)));
        assert_eq!(char_key_in('a', "fr"), Some((0x14, 0)));
        assert_eq!(char_key_in('1', "fr"), Some((0x1E, SHIFT)));
        assert_eq!(char_key_in('m', "fr"), Some((0x33, 0)));
        assert_eq!(char_key_in('ñ', "es"), Some((0x33, 0)));
        assert_eq!(char_key_in('@', "es"), Some((0x1F, ALTGR)));
        assert_eq!(char_key_in('b', "de"), Some((0x05, 0)), "letters not moved stay put");
        assert_eq!(char_key_in('\\', "de"), Some((0x2D, ALTGR)), "not the US backslash key");
        assert_eq!(char_key_in('é', "us"), None);
    }

    /// No layout makes two keys type the same character, and every character a layout
    /// can type comes back to the key it was found on.
    #[test]
    fn layouts_are_consistent() {
        for layout in LAYOUTS {
            let mut seen = std::collections::HashMap::new();
            let rows = overrides(layout);
            let mut chars: Vec<char> = rows.iter().flat_map(|r| [r.1, r.2, r.3]).flatten().collect();
            chars.extend((' '..='~').chain(['\n', '\t']).filter(|&c| {
                char_key(c).is_some_and(|(u, _)| !rows.iter().any(|r| r.0 == u))
            }));
            for c in chars {
                let key = char_key_in(c, layout).unwrap_or_else(|| panic!("{layout}: {c:?} has no key"));
                if let Some(other) = seen.insert(c, key) {
                    assert_eq!(other, key, "{layout}: {c:?} on two keys");
                }
            }
            for c in ['a', 'Z', '0', ' ', '\n', '.', '-'] {
                assert!(char_key_in(c, layout).is_some(), "{layout} types {c:?}");
            }
        }
    }

    #[test]
    fn every_printable_character_round_trips() {
        for c in (' '..='~').chain(['\n', '\t']) {
            let (u, s) = char_key(c).unwrap_or_else(|| panic!("no key for {c:?}"));
            assert_eq!(key_char(u, s), Some(c));
        }
    }
}

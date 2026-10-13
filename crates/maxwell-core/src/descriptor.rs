//! Report lengths from a raw HID report descriptor, for platforms whose HID API
//! does not report them (Windows gets them from `HidP_GetCaps`).

/// The longest input and output report in the top-level collections on
/// `usage_page`, in bytes including the report ID byte, as Windows counts them.
pub fn report_sizes(descriptor: &[u8], usage_page: u16) -> Option<(usize, usize)> {
    #[derive(Clone, Copy, Default)]
    struct Globals {
        usage_page: u32,
        report_size: u32,
        report_count: u32,
        report_id: u8,
    }
    let mut globals = Globals::default();
    let mut stack = Vec::new();
    let mut depth = 0usize;
    let mut inside = false;
    // (is_output, report id) -> bits
    let mut bits: Vec<((bool, u8), u32)> = Vec::new();
    let mut i = 0;
    while i < descriptor.len() {
        let prefix = descriptor[i];
        if prefix == 0xFE {
            // Long item: size, tag, data.
            let size = usize::from(*descriptor.get(i + 1)?);
            i += 3 + size;
            continue;
        }
        let size = [0, 1, 2, 4][usize::from(prefix & 3)];
        let data = descriptor.get(i + 1..i + 1 + size)?;
        let value = data.iter().rev().fold(0u32, |value, &byte| value << 8 | u32::from(byte));
        i += 1 + size;
        match (prefix >> 2) & 3 {
            // Main items.
            0 => match prefix >> 4 {
                0x8 | 0x9 if inside => {
                    let key = (prefix >> 4 == 0x9, globals.report_id);
                    let added = globals.report_size.checked_mul(globals.report_count)?;
                    match bits.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, total)) => *total = total.checked_add(added)?,
                        None => bits.push((key, added)),
                    }
                }
                0xA => {
                    if depth == 0 {
                        inside = globals.usage_page == u32::from(usage_page);
                    }
                    depth += 1;
                }
                0xC => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        inside = false;
                    }
                }
                _ => {}
            },
            // Global items.
            1 => match prefix >> 4 {
                0x0 => globals.usage_page = value,
                0x7 => globals.report_size = value,
                0x8 => globals.report_id = u8::try_from(value).ok()?,
                0x9 => globals.report_count = value,
                0xA => stack.push(globals),
                0xB => globals = stack.pop()?,
                _ => {}
            },
            _ => {}
        }
    }
    let longest = |output: bool| {
        bits.iter().filter(|((o, _), _)| *o == output).map(|(_, bits)| bits.div_ceil(8) as usize + 1).max()
    };
    Some((longest(false)?, longest(true)?))
}

#[cfg(test)]
mod tests {
    use super::report_sizes;

    // Vendor page 0xFF13: input report 7 and output report 6, 61 bytes each.
    // A second collection on another page must not count.
    const MAXWELL_LIKE: &[u8] = &[
        0x06, 0x13, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x07, 0x75, 0x08, 0x95, 0x3D, 0x81, 0x02, 0x85, 0x06, 0x95,
        0x3D, 0x91, 0x02, 0xC0, 0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x01, 0x75, 0x08, 0x95, 0xFF, 0x81, 0x02,
        0x91, 0x02, 0xC0,
    ];

    #[test]
    fn counts_only_the_requested_page() {
        assert_eq!(report_sizes(MAXWELL_LIKE, 0xFF13), Some((62, 62)));
        assert_eq!(report_sizes(MAXWELL_LIKE, 0x000C), Some((256, 256)));
    }

    #[test]
    fn missing_page_or_truncated_descriptor_gives_none() {
        assert_eq!(report_sizes(MAXWELL_LIKE, 0xFF00), None);
        assert_eq!(report_sizes(&MAXWELL_LIKE[..2], 0xFF13), None);
    }

    #[test]
    fn push_and_pop_restore_globals() {
        let descriptor = [
            0x06, 0x13, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x07, 0x75, 0x08, 0x95, 0x10, 0xA4, 0x95, 0x3D, 0x81, 0x02,
            0xB4, 0x91, 0x02, 0xC0,
        ];
        assert_eq!(report_sizes(&descriptor, 0xFF13), Some((62, 17)));
    }
}

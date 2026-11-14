//! A compiled Windows resource file (`.res`) holding one icon group, built from an `.ico` file (ADR-130).
//!
//! The MSVC linker (`link.exe`, and `lld-link`) accepts `.res` files as input and embeds their resources, so
//! `build.rs` needs neither a resource compiler nor a dependency. Shared with the integration tests
//! (`tests/it/windows_icon.rs`), which parse the result back.
//!
//! The format (Microsoft, "Resource File Formats"): an empty 32-byte entry that marks a 32-bit resource file, then
//! one entry per resource: `DataSize`, `HeaderSize`, `TYPE`, `NAME`, `DataVersion`, `MemoryFlags`, `LanguageId`,
//! `Version`, `Characteristics`, the data, padding to 4 bytes. Every image of the `.ico` becomes an `RT_ICON`
//! resource (ids 1..=n, the image bytes unchanged: a BMP DIB or a PNG), and an `RT_GROUP_ICON` resource (id 1) is
//! the `.ico` directory with each 4-byte file offset replaced by the 2-byte id of its `RT_ICON`. Windows shows an
//! executable's first icon group as its icon.

/// `RT_ICON`: one image of an icon.
pub(crate) const RT_ICON: u16 = 3;
/// `RT_GROUP_ICON`: the directory of an icon's images.
pub(crate) const RT_GROUP_ICON: u16 = 14;
/// The id of the one icon group.
pub(crate) const GROUP_ID: u16 = 1;
/// Memory flags of an icon image as the resource compiler writes them (movable, discardable).
const FLAGS_ICON: u16 = 0x1010;
/// Memory flags of an icon group as the resource compiler writes them (movable, pure, discardable).
const FLAGS_GROUP: u16 = 0x1030;
/// English (United States), the resource compiler's default language.
pub(crate) const LANG_EN_US: u16 = 0x0409;

/// Reads little-endian fields from the front of a byte slice.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let (head, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or("the icon file is truncated")?;
        self.0 = rest;
        Ok(*head)
    }

    fn u16(&mut self) -> Result<u16, String> {
        self.bytes().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, String> {
        self.bytes().map(u32::from_le_bytes)
    }
}

/// Appends one resource entry (ordinal type and name) and its data, padded to a 4-byte boundary.
fn entry(
    res: &mut Vec<u8>,
    kind: u16,
    name: u16,
    flags: u16,
    lang: u16,
    data: &[u8],
) -> Result<(), String> {
    let size = u32::try_from(data.len()).map_err(|_| "an icon image is larger than 4 GiB")?;
    res.extend_from_slice(&size.to_le_bytes());
    res.extend_from_slice(&32_u32.to_le_bytes()); // HeaderSize: the fields up to Characteristics
    for word in [0xFFFF, kind, 0xFFFF, name] {
        res.extend_from_slice(&u16::to_le_bytes(word)); // 0xFFFF: an ordinal follows
    }
    res.extend_from_slice(&0_u32.to_le_bytes()); // DataVersion
    res.extend_from_slice(&flags.to_le_bytes());
    res.extend_from_slice(&lang.to_le_bytes());
    res.extend_from_slice(&0_u32.to_le_bytes()); // Version
    res.extend_from_slice(&0_u32.to_le_bytes()); // Characteristics
    res.extend_from_slice(data);
    res.resize(res.len().next_multiple_of(4), 0);
    Ok(())
}

/// Compiles an `.ico` file into a `.res` file with the `RT_ICON` resources and one `RT_GROUP_ICON`.
pub(crate) fn compile_ico(ico: &[u8]) -> Result<Vec<u8>, String> {
    let mut dir = Reader(ico);
    let (reserved, kind, count) = (dir.u16()?, dir.u16()?, dir.u16()?);
    if reserved != 0 || kind != 1 || count == 0 {
        return Err(
            "not an icon file (ICONDIR reserved = 0, type = 1, at least one image)".to_owned(),
        );
    }
    let mut res = Vec::new();
    res.try_reserve(ico.len().saturating_add(4096))
        .map_err(|e| e.to_string())?;
    entry(&mut res, 0, 0, 0, 0, &[])?; // the empty entry that marks a 32-bit resource file
    let mut group = Vec::new();
    for word in [0, 1, count] {
        group.extend_from_slice(&u16::to_le_bytes(word));
    }
    for id in 1..=count {
        let [width, height, colours, _reserved] = dir.bytes()?;
        let (planes, bit_count, size, offset) = (dir.u16()?, dir.u16()?, dir.u32()?, dir.u32()?);
        let start =
            usize::try_from(offset).map_err(|_| "an image offset does not fit in memory")?;
        let len = usize::try_from(size).map_err(|_| "an image size does not fit in memory")?;
        let image = ico
            .get(start..)
            .and_then(|rest| rest.get(..len))
            .ok_or("an icon image lies outside the file")?;
        entry(&mut res, RT_ICON, id, FLAGS_ICON, LANG_EN_US, image)?;
        group.extend_from_slice(&[width, height, colours, 0]);
        group.extend_from_slice(&planes.to_le_bytes());
        group.extend_from_slice(&bit_count.to_le_bytes());
        group.extend_from_slice(&size.to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    entry(
        &mut res,
        RT_GROUP_ICON,
        GROUP_ID,
        FLAGS_GROUP,
        LANG_EN_US,
        &group,
    )?;
    Ok(res)
}

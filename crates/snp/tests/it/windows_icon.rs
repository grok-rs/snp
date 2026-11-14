//! snp's Windows icon (ADR-130): `build.rs` compiles `assets/snp.ico` into a resource file that the MSVC linker
//! embeds in `snp.exe`. The resource file is checked on every platform; on Windows the built `snp.exe` must carry
//! every image of the icon in its `.rsrc` section.
//!
//! Owner: WP-37.

#[path = "../../build/winres.rs"]
mod winres;

const ICO: &[u8] = include_bytes!("../../assets/snp.ico");

/// One image of an `.ico` file: its directory entry and its bytes.
#[derive(Debug)]
struct IcoImage<'a> {
    width: u8,
    height: u8,
    planes: u16,
    bit_count: u16,
    data: &'a [u8],
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn usz(v: u32) -> usize {
    usize::try_from(v).unwrap()
}

fn ico_images(ico: &[u8]) -> Vec<IcoImage<'_>> {
    assert_eq!((u16_at(ico, 0), u16_at(ico, 2)), (0, 1), "ICONDIR");
    (0..usize::from(u16_at(ico, 4)))
        .map(|i| {
            let e = 6 + 16 * i;
            let (size, offset) = (usz(u32_at(ico, e + 8)), usz(u32_at(ico, e + 12)));
            IcoImage {
                width: ico[e],
                height: ico[e + 1],
                planes: u16_at(ico, e + 4),
                bit_count: u16_at(ico, e + 6),
                data: &ico[offset..offset + size],
            }
        })
        .collect()
}

/// One entry of a resource file with an ordinal type and name.
#[derive(Debug)]
struct ResEntry<'a> {
    kind: u16,
    name: u16,
    lang: u16,
    data: &'a [u8],
}

fn res_entries(res: &[u8]) -> Vec<ResEntry<'_>> {
    let mut entries = Vec::new();
    let mut at = 0;
    while at < res.len() {
        let size = usz(u32_at(res, at));
        assert_eq!(u32_at(res, at + 4), 32, "header size of the entry at {at}");
        assert_eq!(
            (u16_at(res, at + 8), u16_at(res, at + 12)),
            (0xFFFF, 0xFFFF),
            "ordinal type and name at {at}"
        );
        entries.push(ResEntry {
            kind: u16_at(res, at + 10),
            name: u16_at(res, at + 14),
            lang: u16_at(res, at + 22),
            data: &res[at + 32..at + 32 + size],
        });
        at = (at + 32 + size).next_multiple_of(4);
    }
    assert_eq!(at, res.len(), "the last entry ends the file");
    entries
}

#[test]
fn icon_has_the_sizes_windows_asks_for() {
    let sizes: Vec<u32> = ico_images(ICO)
        .iter()
        .map(|i| {
            if i.width == 0 {
                256
            } else {
                u32::from(i.width)
            }
        })
        .collect();
    assert_eq!(sizes, [16, 20, 24, 32, 40, 48, 64, 128, 256]);
    for image in ico_images(ICO) {
        assert_eq!(
            (image.width, image.planes, image.bit_count),
            (image.height, 1, 32),
            "{image:?}"
        );
        let png = image.data.starts_with(b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            png,
            image.width == 0 || image.width > 64,
            "PNG above 64 px, 32-bit BMP up to 64 px"
        );
    }
}

#[test]
fn icon_res_holds_every_image_and_one_group() {
    let images = ico_images(ICO);
    let res = winres::compile_ico(ICO).unwrap();
    let entries = res_entries(&res);
    let (first, rest) = entries.split_first().unwrap();
    assert_eq!(
        (first.kind, first.name, first.data.len()),
        (0, 0, 0),
        "the 32-bit resource file marker"
    );

    let icons: Vec<&ResEntry<'_>> = rest.iter().filter(|e| e.kind == winres::RT_ICON).collect();
    assert_eq!(icons.len(), images.len());
    for (n, (icon, image)) in icons.iter().zip(&images).enumerate() {
        assert_eq!(
            usize::from(icon.name),
            n + 1,
            "RT_ICON ids are 1..=n in directory order"
        );
        assert_eq!(icon.lang, winres::LANG_EN_US);
        assert_eq!(
            icon.data,
            image.data,
            "image {} is embedded unchanged",
            n + 1
        );
    }

    let groups: Vec<&ResEntry<'_>> = rest
        .iter()
        .filter(|e| e.kind == winres::RT_GROUP_ICON)
        .collect();
    assert_eq!(groups.len(), 1);
    let group = groups[0];
    assert_eq!(
        (group.name, group.lang),
        (winres::GROUP_ID, winres::LANG_EN_US)
    );
    let dir = group.data;
    assert_eq!(
        (u16_at(dir, 0), u16_at(dir, 2), usize::from(u16_at(dir, 4))),
        (0, 1, images.len())
    );
    assert_eq!(
        dir.len(),
        6 + 14 * images.len(),
        "GRPICONDIR + one 14-byte GRPICONDIRENTRY per image"
    );
    for (n, image) in images.iter().enumerate() {
        let e = 6 + 14 * n;
        assert_eq!(
            (dir[e], dir[e + 1], dir[e + 3]),
            (image.width, image.height, 0)
        );
        assert_eq!(
            (u16_at(dir, e + 4), u16_at(dir, e + 6)),
            (image.planes, image.bit_count)
        );
        assert_eq!(usz(u32_at(dir, e + 8)), image.data.len());
        assert_eq!(
            usize::from(u16_at(dir, e + 12)),
            n + 1,
            "entry {n} names its RT_ICON"
        );
    }
    assert_eq!(entries.len(), 2 + images.len());
}

#[test]
fn malformed_icons_are_errors() {
    assert!(winres::compile_ico(b"").is_err(), "empty");
    assert!(
        winres::compile_ico(&ICO[..6 + 16 * 9]).is_err(),
        "images outside the file"
    );
    assert!(
        winres::compile_ico(&ICO[..10]).is_err(),
        "truncated directory"
    );
    let mut cursor = ICO.to_vec();
    cursor[2] = 2; // type 2 is a cursor file
    assert!(
        winres::compile_ico(&cursor).is_err(),
        "a cursor is not an icon"
    );
    assert!(
        winres::compile_ico(&[0, 0, 1, 0, 0, 0]).is_err(),
        "no image"
    );
}

/// The bytes of a PE file's `.rsrc` section.
#[cfg(windows)]
fn rsrc(exe: &[u8]) -> &[u8] {
    let pe = usz(u32_at(exe, 0x3C));
    assert_eq!(&exe[pe..pe + 4], b"PE\0\0");
    let sections = usize::from(u16_at(exe, pe + 6));
    let table = pe + 24 + usize::from(u16_at(exe, pe + 20));
    (0..sections)
        .map(|i| table + 40 * i)
        .find(|&s| &exe[s..s + 8] == b".rsrc\0\0\0")
        .map(|s| {
            let (size, start) = (usz(u32_at(exe, s + 16)), usz(u32_at(exe, s + 20)));
            &exe[start..start + size]
        })
        .expect("snp.exe has a .rsrc section")
}

#[cfg(windows)]
#[test]
fn snp_exe_carries_every_icon_image() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_snp")).unwrap();
    let rsrc = rsrc(&exe);
    for image in ico_images(ICO) {
        let found = rsrc.windows(image.data.len()).any(|w| w == image.data);
        assert!(
            found,
            "the {}px image is in snp.exe's resources",
            image.width
        );
    }
}

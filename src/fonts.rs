//! The font families installed on this computer, for the font pickers in the settings.
//!
//! Scanning reads a few tables of every family's regular face, so it runs once, on a thread of
//! its own, the first time the settings open.

use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

use fontique::{Collection, CollectionOptions, FontInfo};
use read_fonts::{FontRef, TableProvider, tables::cmap::PlatformId};
use rune::Waker;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Family {
    pub name: String,
    /// Every glyph has the same advance, so the family suits code and tables.
    pub mono: bool,
}

/// The installed families that set text, sorted by name.
#[derive(Debug, Default)]
pub struct Installed {
    pub families: Vec<Family>,
}

static INSTALLED: OnceLock<Installed> = OnceLock::new();
static SCANNING: AtomicBool = AtomicBool::new(false);

/// The families, once a scan finished.
pub fn installed() -> Option<&'static Installed> {
    if cfg!(test) {
        return Some(test_fonts());
    }
    INSTALLED.get()
}

/// Starts the scan unless it ran or is running; `done` wakes the app when it finishes.
pub fn scan_in_background(done: Option<Waker>) {
    if cfg!(test) || INSTALLED.get().is_some() || SCANNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("font-scan".into())
        .spawn(move || {
            let _ = INSTALLED.set(scan());
            if let Some(done) = done {
                done();
            }
        });
    if spawned.is_err() {
        // The pickers keep offering the families the OS ships; a later opening tries again.
        SCANNING.store(false, Ordering::SeqCst);
    }
}

/// Every installed family that has Latin letters, which leaves out symbol and icon fonts and the
/// OS's hidden families.
pub fn scan() -> Installed {
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    let ids: Vec<_> = collection.family_ids().collect();
    let mut families: Vec<Family> = Vec::new();
    for id in ids {
        let Some(family) = collection.family(id) else {
            continue;
        };
        let name = family.name().trim();
        if hidden(name) {
            continue;
        }
        let Some(mono) = family.default_font().and_then(spacing) else {
            continue;
        };
        families.push(Family {
            name: name.to_string(),
            mono,
        });
    }
    families.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    families.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    Installed { families }
}

/// Families the OS keeps for itself: `.SF NS`, `.AppleSystemUIFont`, the glyphs of last resort,
/// and macOS's `System Font`, which can't be asked for by that name.
fn hidden(name: &str) -> bool {
    name.is_empty()
        || name.starts_with('.')
        || ["LastResort", "System Font"]
            .iter()
            .any(|os| name.eq_ignore_ascii_case(os))
}

/// Whether `font` is monospaced, or None when it can't be read or doesn't set Latin text:
/// symbol fonts (a Windows symbol cmap, or a pictorial PANOSE family, as Wingdings and Zapf
/// Dingbats have) and fonts that map letters to the missing glyph (math and size variants).
fn spacing(font: &FontInfo) -> Option<bool> {
    const SYMBOL_ENCODING: (PlatformId, u16) = (PlatformId::Windows, 0);
    const PANOSE_PICTORIAL: u8 = 5;
    let data = font.load(None)?;
    let font = FontRef::from_index(data.as_ref(), font.index()).ok()?;
    let cmap = font.cmap().ok()?;
    let symbol = cmap
        .encoding_records()
        .iter()
        .any(|record| (record.platform_id(), record.encoding_id()) == SYMBOL_ENCODING);
    let pictorial = font
        .os2()
        .is_ok_and(|os2| os2.panose_10()[0] == PANOSE_PICTORIAL);
    if symbol || pictorial {
        return None;
    }
    let hmtx = font.hmtx().ok()?;
    let advances = ['i', 'M', 'W', '0', 'a']
        .into_iter()
        .map(|c| {
            let glyph = cmap.map_codepoint(c).filter(|glyph| glyph.to_u32() != 0)?;
            hmtx.advance(glyph)
        })
        .collect::<Option<Vec<u16>>>()?;
    let fixed_pitch = font.post().is_ok_and(|post| post.is_fixed_pitch() != 0);
    let same_widths = advances[0] > 0 && advances.iter().all(|&advance| advance == advances[0]);
    Some(fixed_pitch || same_widths)
}

/// A fixed list, so tests don't depend on the fonts of the machine they run on.
fn test_fonts() -> &'static Installed {
    static FONTS: OnceLock<Installed> = OnceLock::new();
    FONTS.get_or_init(|| {
        let family = |name: &str, mono: bool| Family {
            name: name.to_string(),
            mono,
        };
        Installed {
            families: vec![
                family("Andale Mono", true),
                family("Avenir Next", false),
                family("Georgia", false),
                family("Helvetica Neue", false),
                family("Menlo", true),
                family("Monaco", true),
                family("Verdana", false),
            ],
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{hidden, scan};

    #[test]
    fn the_os_families_are_hidden() {
        assert!(hidden(".SF NS"));
        assert!(hidden("LastResort"));
        assert!(hidden("System Font"));
        assert!(hidden(""));
        assert!(!hidden("Menlo"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_scan_finds_the_fonts_macos_ships_and_tells_monospace_apart() {
        let installed = scan();
        let mono = |name: &str| {
            installed
                .families
                .iter()
                .find(|family| family.name == name)
                .map(|family| family.mono)
        };
        assert_eq!(mono("Menlo"), Some(true));
        assert_eq!(mono("Courier New"), Some(true));
        assert_eq!(mono("Helvetica Neue"), Some(false));
        assert_eq!(mono("Georgia"), Some(false));
        for symbols in ["Wingdings", "Webdings", "Zapf Dingbats", "STIXSizeOneSym"] {
            assert_eq!(mono(symbols), None, "{symbols} doesn't set text");
        }
        assert!(
            installed
                .families
                .iter()
                .all(|family| !family.name.starts_with('.')),
            "hidden families are left out"
        );
        assert!(
            installed
                .families
                .windows(2)
                .all(|pair| pair[0].name.to_lowercase() <= pair[1].name.to_lowercase()),
            "sorted by name"
        );
    }
}
